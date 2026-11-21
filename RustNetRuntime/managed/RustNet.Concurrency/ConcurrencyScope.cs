namespace RustNet.Concurrency;

/// <summary>
/// Structured concurrency: work started in a scope finishes before the scope
/// does.
/// </summary>
/// <remarks>
/// <para>
/// A scope owns the tasks started through it. <see cref="JoinAsync"/> — or
/// <c>await using</c>, which calls it — waits for every one of them, including
/// tasks started by other tasks while the join was waiting.
/// </para>
/// <para>
/// The first child to fail cancels the scope's token, so its siblings stop
/// early instead of finishing work nobody will use, and the join throws an
/// <see cref="AggregateException"/> of every failure. A child that ends with
/// <see cref="OperationCanceledException"/> because the scope was cancelled is
/// not a failure: it did what it was asked.
/// </para>
/// <para>
/// Cancellation flows down. A scope made with a parent token, or by
/// <see cref="CreateChild"/>, is cancelled when its parent is; cancelling a
/// child does not cancel its parent.
/// </para>
/// </remarks>
public sealed class ConcurrencyScope : IAsyncDisposable
{
    private readonly CancellationTokenSource _cancellation;
    private readonly object _gate = new object();
    private readonly List<Task> _watchers = new List<Task>();
    private readonly List<Exception> _failures = new List<Exception>();
    private bool _joined;

    public ConcurrencyScope() : this(CancellationToken.None) { }

    public ConcurrencyScope(CancellationToken parent)
    {
        _cancellation = CancellationTokenSource.CreateLinkedTokenSource(parent);
    }

    /// <summary>Cancelled when the scope is, which children should observe.</summary>
    public CancellationToken Token => _cancellation.Token;

    public bool IsCancellationRequested => _cancellation.IsCancellationRequested;

    /// <summary>Failures recorded so far, in the order they were observed.</summary>
    public IReadOnlyList<Exception> Failures
    {
        get { lock (_gate) return _failures.ToArray(); }
    }

    /// <summary>Asks every child to stop.</summary>
    public void Cancel() => _cancellation.Cancel();

    /// <summary>A scope whose token is cancelled with this one's.</summary>
    public ConcurrencyScope CreateChild() => new ConcurrencyScope(Token);

    /// <summary>Starts a child on the thread pool.</summary>
    public Task Run(Func<CancellationToken, Task> body)
    {
        if (body == null) throw new ArgumentNullException(nameof(body));
        Task task = Task.Run(() => body(Token));
        Track(task);
        return task;
    }

    /// <summary>Starts a child that produces a value.</summary>
    public Task<T> Run<T>(Func<CancellationToken, Task<T>> body)
    {
        if (body == null) throw new ArgumentNullException(nameof(body));
        Task<T> task = Task.Run(() => body(Token));
        Track(task);
        return task;
    }

    /// <summary>Starts a synchronous child.</summary>
    public Task Run(Action<CancellationToken> body)
    {
        if (body == null) throw new ArgumentNullException(nameof(body));
        Task task = Task.Run(() => body(Token));
        Track(task);
        return task;
    }

    private void Track(Task task)
    {
        lock (_gate)
        {
            if (_joined)
                throw new InvalidOperationException("The scope has already been joined.");
            _watchers.Add(Observe(task));
        }
    }

    /// <summary>
    /// Waits for the child, and records how it ended. Never faults itself, so
    /// that waiting on every watcher is waiting on every child.
    /// </summary>
    private async Task Observe(Task task)
    {
        try
        {
            await task.ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (_cancellation.IsCancellationRequested)
        {
            // Asked to stop, and stopped.
        }
        catch (Exception error)
        {
            lock (_gate) _failures.Add(error);
            _cancellation.Cancel();
        }
    }

    /// <summary>
    /// Waits for every child, then throws an <see cref="AggregateException"/>
    /// of their failures if there were any. After this no more children can be
    /// started.
    /// </summary>
    public async Task JoinAsync()
    {
        while (true)
        {
            Task[] pending;
            lock (_gate)
            {
                pending = _watchers.ToArray();
                _watchers.Clear();
                if (pending.Length == 0)
                {
                    // Nothing outstanding and nothing new: done. Closed under the
                    // same lock that `Track` takes, so a child cannot slip in
                    // between the last check and the close.
                    _joined = true;
                    break;
                }
            }
            await Task.WhenAll(pending).ConfigureAwait(false);
        }

        Exception[] failures;
        lock (_gate) failures = _failures.ToArray();
        if (failures.Length > 0)
            throw new AggregateException("One or more tasks in the scope failed.", failures);
    }

    public async ValueTask DisposeAsync()
    {
        try
        {
            await JoinAsync().ConfigureAwait(false);
        }
        finally
        {
            _cancellation.Dispose();
        }
    }
}
