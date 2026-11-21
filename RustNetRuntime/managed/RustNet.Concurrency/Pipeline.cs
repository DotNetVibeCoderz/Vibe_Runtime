using System.Threading.Channels;

namespace RustNet.Concurrency;

/// <summary>Starts a pipeline.</summary>
/// <remarks>
/// <para>
/// A pipeline is stages joined by bounded channels. Each stage runs as soon as
/// it is attached, and a full channel makes the stage before it wait — so a
/// slow stage slows its producer instead of buffering without limit.
/// </para>
/// <para>
/// <b>Order is kept</b>, even by a stage with several workers: items leave a
/// stage in the order they entered it. A stage that throws completes its
/// output with the exception, every later stage stops, and the exception comes
/// out of whatever finally reads the pipeline.
/// </para>
/// </remarks>
public static class Pipeline
{
    /// <summary>The default room between two stages.</summary>
    public const int DefaultCapacity = 16;

    /// <summary>A pipeline fed from a sequence.</summary>
    public static Stage<T> From<T>(IEnumerable<T> source, int capacity = DefaultCapacity, CancellationToken cancellationToken = default)
    {
        if (source == null) throw new ArgumentNullException(nameof(source));
        Channel<T> output = Bounded<T>(capacity);
        _ = Task.Run(async () =>
        {
            try
            {
                foreach (T item in source)
                    await output.Writer.WriteAsync(item, cancellationToken).ConfigureAwait(false);
                output.Writer.TryComplete();
            }
            catch (Exception error)
            {
                output.Writer.TryComplete(error);
            }
        });
        return new Stage<T>(output.Reader, capacity, cancellationToken);
    }

    /// <summary>A pipeline fed from a channel someone else writes.</summary>
    public static Stage<T> From<T>(ChannelReader<T> source, int capacity = DefaultCapacity, CancellationToken cancellationToken = default)
    {
        if (source == null) throw new ArgumentNullException(nameof(source));
        return new Stage<T>(source, capacity, cancellationToken);
    }

    internal static Channel<T> Bounded<T>(int capacity) =>
        Channel.CreateBounded<T>(new BoundedChannelOptions(capacity < 1 ? 1 : capacity)
        {
            FullMode = BoundedChannelFullMode.Wait,
        });
}

/// <summary>The output of one stage, to which the next is attached.</summary>
public sealed class Stage<T>
{
    private readonly ChannelReader<T> _reader;
    private readonly int _capacity;
    private readonly CancellationToken _cancellation;

    internal Stage(ChannelReader<T> reader, int capacity, CancellationToken cancellation)
    {
        _reader = reader;
        _capacity = capacity;
        _cancellation = cancellation;
    }

    /// <summary>The stage's output, for reading it directly.</summary>
    public ChannelReader<T> Reader => _reader;

    /// <summary>Transforms each item.</summary>
    /// <param name="parallelism">Items transformed at once. Output order is kept.</param>
    public Stage<TOut> Select<TOut>(Func<T, TOut> transform, int parallelism = 1)
    {
        if (transform == null) throw new ArgumentNullException(nameof(transform));
        return SelectAsync<TOut>(item => Task.FromResult(transform(item)), parallelism);
    }

    /// <summary>Transforms each item asynchronously.</summary>
    /// <param name="parallelism">Items transformed at once. Output order is kept.</param>
    public Stage<TOut> SelectAsync<TOut>(Func<T, Task<TOut>> transform, int parallelism = 1)
    {
        if (transform == null) throw new ArgumentNullException(nameof(transform));
        if (parallelism < 1) throw new ArgumentOutOfRangeException(nameof(parallelism));
        Channel<TOut> output = Pipeline.Bounded<TOut>(_capacity);

        if (parallelism == 1)
        {
            _ = Task.Run(async () =>
            {
                try
                {
                    while (await _reader.WaitToReadAsync(_cancellation).ConfigureAwait(false))
                        while (_reader.TryRead(out T item))
                            await output.Writer.WriteAsync(await transform(item).ConfigureAwait(false), _cancellation).ConfigureAwait(false);
                    await _reader.Completion.ConfigureAwait(false);
                    output.Writer.TryComplete();
                }
                catch (Exception error)
                {
                    output.Writer.TryComplete(error);
                }
            });
            return Next(output.Reader);
        }

        // Several at once, order kept: each item's work starts as a task, and
        // the tasks themselves queue in input order. A second loop awaits them
        // in that order. The queue between the two is what bounds how many run
        // at once.
        Channel<Task<TOut>> inFlight = Pipeline.Bounded<Task<TOut>>(parallelism);
        _ = Task.Run(async () =>
        {
            try
            {
                while (await _reader.WaitToReadAsync(_cancellation).ConfigureAwait(false))
                {
                    while (_reader.TryRead(out T item))
                    {
                        T captured = item;
                        Task<TOut> work = Task.Run(() => transform(captured));
                        await inFlight.Writer.WriteAsync(work, _cancellation).ConfigureAwait(false);
                    }
                }
                await _reader.Completion.ConfigureAwait(false);
                inFlight.Writer.TryComplete();
            }
            catch (Exception error)
            {
                inFlight.Writer.TryComplete(error);
            }
        });
        _ = Task.Run(async () =>
        {
            try
            {
                while (await inFlight.Reader.WaitToReadAsync(_cancellation).ConfigureAwait(false))
                    while (inFlight.Reader.TryRead(out Task<TOut> work))
                        await output.Writer.WriteAsync(await work.ConfigureAwait(false), _cancellation).ConfigureAwait(false);
                await inFlight.Reader.Completion.ConfigureAwait(false);
                output.Writer.TryComplete();
            }
            catch (Exception error)
            {
                output.Writer.TryComplete(error);
            }
        });
        return Next(output.Reader);
    }

    /// <summary>Keeps the items that match.</summary>
    public Stage<T> Where(Func<T, bool> predicate)
    {
        if (predicate == null) throw new ArgumentNullException(nameof(predicate));
        Channel<T> output = Pipeline.Bounded<T>(_capacity);
        _ = Task.Run(async () =>
        {
            try
            {
                while (await _reader.WaitToReadAsync(_cancellation).ConfigureAwait(false))
                    while (_reader.TryRead(out T item))
                        if (predicate(item))
                            await output.Writer.WriteAsync(item, _cancellation).ConfigureAwait(false);
                await _reader.Completion.ConfigureAwait(false);
                output.Writer.TryComplete();
            }
            catch (Exception error)
            {
                output.Writer.TryComplete(error);
            }
        });
        return Next(output.Reader);
    }

    /// <summary>Groups items into arrays of <paramref name="size"/>; the last may be shorter.</summary>
    public Stage<T[]> Batch(int size)
    {
        if (size < 1) throw new ArgumentOutOfRangeException(nameof(size));
        Channel<T[]> output = Pipeline.Bounded<T[]>(_capacity);
        _ = Task.Run(async () =>
        {
            try
            {
                var batch = new List<T>(size);
                while (await _reader.WaitToReadAsync(_cancellation).ConfigureAwait(false))
                {
                    while (_reader.TryRead(out T item))
                    {
                        batch.Add(item);
                        if (batch.Count == size)
                        {
                            await output.Writer.WriteAsync(batch.ToArray(), _cancellation).ConfigureAwait(false);
                            batch.Clear();
                        }
                    }
                }
                await _reader.Completion.ConfigureAwait(false);
                if (batch.Count > 0)
                    await output.Writer.WriteAsync(batch.ToArray(), _cancellation).ConfigureAwait(false);
                output.Writer.TryComplete();
            }
            catch (Exception error)
            {
                output.Writer.TryComplete(error);
            }
        });
        return Next(output.Reader);
    }

    /// <summary>Runs an action on each item, in order, and waits for the end.</summary>
    public async Task ForEachAsync(Action<T> action)
    {
        if (action == null) throw new ArgumentNullException(nameof(action));
        while (await _reader.WaitToReadAsync(_cancellation).ConfigureAwait(false))
            while (_reader.TryRead(out T item))
                action(item);
        // A stage that failed completed its output with the exception, and
        // this is where it comes out.
        await _reader.Completion.ConfigureAwait(false);
    }

    /// <summary>Every item, in order, once the pipeline ends.</summary>
    public async Task<List<T>> ToListAsync()
    {
        var items = new List<T>();
        await ForEachAsync(items.Add).ConfigureAwait(false);
        return items;
    }

    private Stage<TNext> Next<TNext>(ChannelReader<TNext> reader) => new Stage<TNext>(reader, _capacity, _cancellation);
}
