using System.Threading.Channels;

namespace RustNet.Concurrency;

/// <summary>
/// Waits on several channels at once and handles whichever has a message —
/// Go's <c>select</c>, over <see cref="ChannelReader{T}"/>.
/// </summary>
/// <remarks>
/// <para>
/// Cases are tried in the order they were added, so when several channels are
/// ready the earlier case wins. That is a priority, stated: Go picks at random,
/// and a program that relies on either is relying on something it should not.
/// </para>
/// <para>
/// A completed channel's case is retired. <see cref="SelectAsync"/> returns
/// <c>false</c> once every case is retired and there is no default, which is
/// how <see cref="RunAsync"/> knows to stop.
/// </para>
/// </remarks>
public sealed class Selector
{
    private abstract class Case
    {
        public bool Retired;
        public abstract bool TryTake(out Func<Task> run);
        public abstract Task<bool> WaitAsync(CancellationToken cancellationToken);
    }

    private sealed class Case<T> : Case
    {
        private readonly ChannelReader<T> _reader;
        private readonly Func<T, Task> _handler;

        public Case(ChannelReader<T> reader, Func<T, Task> handler)
        {
            _reader = reader;
            _handler = handler;
        }

        public override bool TryTake(out Func<Task> run)
        {
            if (_reader.TryRead(out T item))
            {
                run = () => _handler(item);
                return true;
            }
            run = null;
            return false;
        }

        public override Task<bool> WaitAsync(CancellationToken cancellationToken) =>
            _reader.WaitToReadAsync(cancellationToken).AsTask();
    }

    private readonly List<Case> _cases = new List<Case>();
    private Func<Task> _default;

    /// <summary>Handles a message from <paramref name="reader"/>.</summary>
    public Selector On<T>(ChannelReader<T> reader, Func<T, Task> handler)
    {
        _cases.Add(new Case<T>(reader ?? throw new ArgumentNullException(nameof(reader)),
            handler ?? throw new ArgumentNullException(nameof(handler))));
        return this;
    }

    /// <summary>Handles a message from <paramref name="reader"/> synchronously.</summary>
    public Selector On<T>(ChannelReader<T> reader, Action<T> handler)
    {
        if (handler == null) throw new ArgumentNullException(nameof(handler));
        return On<T>(reader, item => { handler(item); return Task.CompletedTask; });
    }

    /// <summary>Runs instead of waiting when no case is ready.</summary>
    public Selector Default(Action handler)
    {
        if (handler == null) throw new ArgumentNullException(nameof(handler));
        _default = () => { handler(); return Task.CompletedTask; };
        return this;
    }

    /// <summary>
    /// Handles exactly one message, or runs the default. False when every
    /// channel has completed and there is no default to run.
    /// </summary>
    public async Task<bool> SelectAsync(CancellationToken cancellationToken = default)
    {
        while (true)
        {
            foreach (Case c in _cases)
            {
                if (!c.Retired && c.TryTake(out Func<Task> run))
                {
                    await run().ConfigureAwait(false);
                    return true;
                }
            }

            if (_default != null)
            {
                await _default().ConfigureAwait(false);
                return true;
            }

            var waits = new List<Task<bool>>();
            var owners = new List<Case>();
            foreach (Case c in _cases)
            {
                if (c.Retired) continue;
                waits.Add(c.WaitAsync(cancellationToken));
                owners.Add(c);
            }
            if (waits.Count == 0) return false;

            Task<bool> first = await Task.WhenAny(waits).ConfigureAwait(false);
            // A wait that answers false means its channel is complete and empty.
            // Every finished wait is checked, not only the first, so a channel
            // that closed at the same moment is retired on this pass.
            for (int i = 0; i < waits.Count; i++)
            {
                if (waits[i].IsCompleted && !await waits[i].ConfigureAwait(false))
                    owners[i].Retired = true;
            }
            cancellationToken.ThrowIfCancellationRequested();
            _ = first;
        }
    }

    /// <summary>Selects until every channel has completed.</summary>
    public async Task RunAsync(CancellationToken cancellationToken = default)
    {
        if (_default != null)
            throw new InvalidOperationException("A selector with a default never runs out; call SelectAsync in a loop.");
        while (await SelectAsync(cancellationToken).ConfigureAwait(false)) { }
    }
}

/// <summary>Shorthand for one-off selects.</summary>
public static class Select
{
    /// <summary>
    /// The next message from whichever reader has one, with its index. Readers
    /// earlier in the list win ties. Throws <see cref="ChannelClosedException"/>
    /// when every reader has completed.
    /// </summary>
    public static async Task<(int Index, T Item)> ReceiveAnyAsync<T>(
        IReadOnlyList<ChannelReader<T>> readers,
        CancellationToken cancellationToken = default)
    {
        if (readers == null) throw new ArgumentNullException(nameof(readers));
        int index = -1;
        T received = default;
        var selector = new Selector();
        for (int i = 0; i < readers.Count; i++)
        {
            int n = i;
            selector.On<T>(readers[i], item => { index = n; received = item; });
        }
        if (!await selector.SelectAsync(cancellationToken).ConfigureAwait(false))
            throw new ChannelClosedException();
        return (index, received);
    }
}
