using System.Threading.Channels;

namespace RustNet.Concurrency;

/// <summary>
/// Where an actor's messages wait. A channel with one reader — the actor —
/// and any number of writers.
/// </summary>
/// <remarks>
/// Unbounded by default. A bounded mailbox makes <see cref="SendAsync"/> wait
/// while it is full, which is back-pressure; <see cref="Post"/> on a full
/// bounded mailbox returns <c>false</c> rather than waiting.
/// </remarks>
public sealed class Mailbox<T>
{
    private readonly Channel<T> _channel;

    public Mailbox() : this(0) { }

    /// <param name="capacity">Zero or less for unbounded.</param>
    public Mailbox(int capacity)
    {
        Capacity = capacity > 0 ? capacity : 0;
        _channel = capacity > 0
            ? Channel.CreateBounded<T>(new BoundedChannelOptions(capacity)
            {
                FullMode = BoundedChannelFullMode.Wait,
                SingleReader = true,
            })
            : Channel.CreateUnbounded<T>(new UnboundedChannelOptions { SingleReader = true });
    }

    /// <summary>Zero for an unbounded mailbox.</summary>
    public int Capacity { get; }

    /// <summary>Queues a message without waiting. False if full or closed.</summary>
    public bool Post(T message) => _channel.Writer.TryWrite(message);

    /// <summary>Queues a message, waiting for room in a bounded mailbox.</summary>
    public ValueTask SendAsync(T message, CancellationToken cancellationToken = default) =>
        _channel.Writer.WriteAsync(message, cancellationToken);

    /// <summary>No more messages will be accepted; queued ones are still delivered.</summary>
    public bool Complete() => _channel.Writer.TryComplete();

    internal ChannelReader<T> Reader => _channel.Reader;
}

/// <summary>What a supervisor does with an actor whose handler threw.</summary>
public enum SupervisorDirective
{
    /// <summary>Drop the message and carry on with the same actor and its state.</summary>
    Resume,
    /// <summary>Replace the actor with a fresh one from its factory, losing its state.</summary>
    Restart,
    /// <summary>Stop the actor. Its completion succeeds; queued messages are dropped.</summary>
    Stop,
    /// <summary>Stop the actor and fault its completion with the exception.</summary>
    Escalate,
}

/// <summary>
/// How an actor's failures are handled: a decision per exception, and a limit
/// on restarts so that an actor failing on every message is stopped rather
/// than restarted forever.
/// </summary>
public sealed class SupervisorStrategy
{
    private readonly Func<Exception, SupervisorDirective> _decider;

    public SupervisorStrategy(Func<Exception, SupervisorDirective> decider, int maxRestarts = 10)
    {
        _decider = decider ?? throw new ArgumentNullException(nameof(decider));
        MaxRestarts = maxRestarts;
    }

    /// <summary>Restarts beyond this escalate instead.</summary>
    public int MaxRestarts { get; }

    public SupervisorDirective Decide(Exception error) => _decider(error);

    /// <summary>Restart on any exception, up to ten times.</summary>
    public static SupervisorStrategy RestartOnFailure { get; } =
        new SupervisorStrategy(_ => SupervisorDirective.Restart);

    /// <summary>Fault the actor's completion on the first exception.</summary>
    public static SupervisorStrategy EscalateOnFailure { get; } =
        new SupervisorStrategy(_ => SupervisorDirective.Escalate, 0);

    /// <summary>Ignore the failing message and carry on.</summary>
    public static SupervisorStrategy ResumeOnFailure { get; } =
        new SupervisorStrategy(_ => SupervisorDirective.Resume);
}

/// <summary>
/// An actor: state that only its own message loop touches, so it needs no lock.
/// </summary>
/// <remarks>
/// Messages are handled one at a time, in the order they were queued. A
/// handler may <c>await</c>; the next message waits for it.
/// </remarks>
public abstract class Actor<T>
{
    /// <summary>The actor's own reference, for replying or stopping itself.</summary>
    protected ActorRef<T> Self { get; private set; }

    internal void Attach(ActorRef<T> self) => Self = self;

    /// <summary>Handles one message.</summary>
    protected internal abstract Task ReceiveAsync(T message);

    /// <summary>Called before the first message, and after every restart.</summary>
    protected internal virtual void OnStarted() { }

    /// <summary>Called when the actor stops or is replaced by a restart.</summary>
    protected internal virtual void OnStopped() { }
}

/// <summary>
/// How the rest of a program reaches an actor. The actor itself is never
/// handed out, which is what keeps its state single-threaded.
/// </summary>
public sealed class ActorRef<T>
{
    private readonly Func<Actor<T>> _factory;
    private readonly SupervisorStrategy _strategy;
    private readonly Mailbox<T> _mailbox;
    private readonly CancellationTokenSource _stop = new CancellationTokenSource();
    private int _restarts;

    internal ActorRef(string name, Func<Actor<T>> factory, SupervisorStrategy strategy, int capacity)
    {
        Name = name;
        _factory = factory;
        _strategy = strategy;
        _mailbox = new Mailbox<T>(capacity);
        Completion = Task.Run(RunAsync);
    }

    public string Name { get; }

    /// <summary>Completes when the actor stops; faults if it escalated.</summary>
    public Task Completion { get; }

    /// <summary>How many times the actor has been replaced after a failure.</summary>
    public int Restarts => Volatile.Read(ref _restarts);

    public bool Post(T message) => _mailbox.Post(message);

    public ValueTask SendAsync(T message, CancellationToken cancellationToken = default) =>
        _mailbox.SendAsync(message, cancellationToken);

    /// <summary>
    /// Sends a message that carries its own reply slot, and waits for the
    /// answer. The actor completes the source it was given.
    /// </summary>
    public async Task<TReply> AskAsync<TReply>(Func<TaskCompletionSource<TReply>, T> build)
    {
        var reply = new TaskCompletionSource<TReply>(TaskCreationOptions.RunContinuationsAsynchronously);
        await SendAsync(build(reply)).ConfigureAwait(false);
        return await reply.Task.ConfigureAwait(false);
    }

    /// <summary>Stops accepting messages; the actor handles what is queued, then stops.</summary>
    public void Complete() => _mailbox.Complete();

    /// <summary>Stops the actor now; queued messages are dropped.</summary>
    public void Stop()
    {
        _mailbox.Complete();
        _stop.Cancel();
    }

    private Actor<T> Start()
    {
        Actor<T> actor = _factory();
        actor.Attach(this);
        actor.OnStarted();
        return actor;
    }

    private async Task RunAsync()
    {
        Actor<T> actor = Start();
        ChannelReader<T> reader = _mailbox.Reader;
        try
        {
            while (await reader.WaitToReadAsync().ConfigureAwait(false))
            {
                while (!_stop.IsCancellationRequested && reader.TryRead(out T message))
                {
                    try
                    {
                        await actor.ReceiveAsync(message).ConfigureAwait(false);
                    }
                    catch (Exception error)
                    {
                        SupervisorDirective directive = _strategy.Decide(error);
                        if (directive == SupervisorDirective.Restart && _restarts >= _strategy.MaxRestarts)
                            directive = SupervisorDirective.Escalate;
                        switch (directive)
                        {
                            case SupervisorDirective.Resume:
                                break;
                            case SupervisorDirective.Restart:
                                actor.OnStopped();
                                Interlocked.Increment(ref _restarts);
                                actor = Start();
                                break;
                            case SupervisorDirective.Stop:
                                _mailbox.Complete();
                                return;
                            default:
                                _mailbox.Complete();
                                throw new ActorFailedException(Name, error);
                        }
                    }
                }
                if (_stop.IsCancellationRequested) return;
            }
        }
        finally
        {
            actor.OnStopped();
        }
    }
}

/// <summary>An actor's completion, when its supervisor escalated.</summary>
public sealed class ActorFailedException : Exception
{
    public ActorFailedException(string actor, Exception inner)
        : base("Actor '" + actor + "' failed: " + inner.Message, inner)
    {
        Actor = actor;
    }

    public string Actor { get; }
}

/// <summary>Starts actors.</summary>
public static class Actors
{
    /// <summary>
    /// Starts an actor made by <paramref name="factory"/>. The factory is called
    /// again on every restart, so a restarted actor starts from fresh state.
    /// </summary>
    public static ActorRef<T> Spawn<T>(
        string name,
        Func<Actor<T>> factory,
        SupervisorStrategy strategy = null,
        int capacity = 0)
    {
        if (factory == null) throw new ArgumentNullException(nameof(factory));
        return new ActorRef<T>(name, factory, strategy ?? SupervisorStrategy.RestartOnFailure, capacity);
    }
}
