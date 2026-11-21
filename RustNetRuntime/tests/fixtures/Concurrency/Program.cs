// The Concurrency fixture: RustNet.Concurrency, exercised the same way under
// `dotnet` and under RustCLR. The two must print the same bytes.
//
// Every value printed is one the program determines — a sum, a count, a sorted
// list — never an order that depends on which thread ran first.

using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;
using RustNet.Concurrency;

static class Program
{
    static int checks;
    static int failures;

    static void Check(string label, bool ok)
    {
        checks++;
        if (!ok)
        {
            failures++;
            Console.WriteLine("FAIL " + label);
        }
    }

    static void CheckEq<T>(string label, T actual, T expected)
    {
        checks++;
        if (!EqualityComparer<T>.Default.Equals(actual, expected))
        {
            failures++;
            Console.WriteLine("FAIL " + label + ": expected " + expected + ", got " + actual);
        }
        else
        {
            Console.WriteLine(label + " = " + actual);
        }
    }

    static async Task Main()
    {
        await Scopes();
        await ActorsWork();
        await Supervision();
        await Selects();
        await Pipelines();
        Console.WriteLine("checks=" + checks + " failures=" + failures);
    }

    // -- ConcurrencyScope -----------------------------------------------------

    static async Task Scopes()
    {
        int done = 0;
        await using (var scope = new ConcurrencyScope())
        {
            for (int i = 0; i < 10; i++)
                scope.Run(async ct => { await Task.Delay(5); Interlocked.Increment(ref done); });
        }
        CheckEq("scope waits for every child", done, 10);

        // A child that starts a child while the scope is joining.
        int nested = 0;
        var outer = new ConcurrencyScope();
        outer.Run(async ct =>
        {
            await Task.Delay(10);
            outer.Run(async ct2 => { await Task.Delay(10); Interlocked.Increment(ref nested); });
            Interlocked.Increment(ref nested);
        });
        await outer.JoinAsync();
        CheckEq("children started during the join are joined too", nested, 2);

        var sum = new ConcurrencyScope();
        Task<int>[] parts = Enumerable.Range(1, 5).Select(n => sum.Run(async ct => { await Task.Yield(); return n * n; })).ToArray();
        await sum.JoinAsync();
        CheckEq("Run<T> hands back each result", parts.Sum(p => p.Result), 55);

        // The first failure cancels the siblings.
        bool siblingSawCancel = false;
        string caught = "none";
        try
        {
            await using var failing = new ConcurrencyScope();
            failing.Run(async ct =>
            {
                try { await Task.Delay(Timeout.Infinite, ct); }
                catch (OperationCanceledException) { siblingSawCancel = true; throw; }
            });
            failing.Run(async ct => { await Task.Delay(20); throw new InvalidOperationException("boom"); });
        }
        catch (AggregateException e)
        {
            caught = e.InnerExceptions.Count + ":" + e.InnerExceptions[0].GetType().Name + ":" + e.InnerExceptions[0].Message;
        }
        CheckEq("a failure surfaces as an AggregateException", caught, "1:InvalidOperationException:boom");
        Check("and cancels its siblings", siblingSawCancel);

        // Several failures, all reported. Sorted: which failed first is timing.
        string all = "none";
        try
        {
            await using var many = new ConcurrencyScope();
            foreach (string name in new[] { "c", "a", "b" })
                many.Run(ct => { throw new ArgumentException(name); });
        }
        catch (AggregateException e)
        {
            all = string.Join(",", e.InnerExceptions.Select(x => x.Message).OrderBy(m => m, StringComparer.Ordinal));
        }
        CheckEq("every failure is collected", all, "a,b,c");

        // Cancellation flows down, not up.
        var parent = new ConcurrencyScope();
        var child = parent.CreateChild();
        var sibling = parent.CreateChild();
        child.Cancel();
        Check("cancelling a child leaves the parent running", !parent.IsCancellationRequested && !sibling.IsCancellationRequested);
        parent.Cancel();
        Check("cancelling the parent cancels its children", sibling.IsCancellationRequested);
        await parent.JoinAsync();

        string late = "none";
        try { parent.Run(ct => { }); }
        catch (InvalidOperationException) { late = "refused"; }
        CheckEq("a joined scope starts nothing more", late, "refused");
    }

    // -- actors ---------------------------------------------------------------

    sealed record CounterMessage(int Add, TaskCompletionSource<int> Reply);

    sealed class Counter : Actor<CounterMessage>
    {
        private int _total;

        protected override Task ReceiveAsync(CounterMessage message)
        {
            _total += message.Add;
            message.Reply?.SetResult(_total);
            return Task.CompletedTask;
        }
    }

    sealed record Note(string Text, TaskCompletionSource<string> Reply);

    sealed class Recorder : Actor<Note>
    {
        private readonly List<string> _seen = new List<string>();
        public static int Stopped;

        protected override async Task ReceiveAsync(Note note)
        {
            if (note.Reply != null)
            {
                note.Reply.SetResult(string.Join(",", _seen));
                return;
            }
            // An await between messages must not let the next one overtake.
            if (_seen.Count % 3 == 0) await Task.Yield();
            _seen.Add(note.Text);
        }

        protected override void OnStopped() => Interlocked.Increment(ref Stopped);
    }

    static async Task ActorsWork()
    {
        ActorRef<CounterMessage> counter = Actors.Spawn("counter", () => new Counter());
        // From several threads at once: the actor's state needs no lock.
        await Task.WhenAll(Enumerable.Range(0, 4).Select(t => Task.Run(() =>
        {
            for (int i = 1; i <= 25; i++)
                counter.Post(new CounterMessage(t * 25 + i, null));
        })));
        int total = await counter.AskAsync<int>(reply => new CounterMessage(0, reply));
        CheckEq("an actor sums 1..100 posted from four threads", total, 5050);
        counter.Complete();
        await counter.Completion;
        Check("completing the mailbox ends the actor", counter.Completion.IsCompletedSuccessfully);

        ActorRef<Note> recorder = Actors.Spawn("recorder", () => new Recorder());
        for (int i = 1; i <= 10; i++)
            await recorder.SendAsync(new Note(i.ToString(), null));
        string order = await recorder.AskAsync<string>(reply => new Note(null, reply));
        CheckEq("messages are handled in the order sent", order, "1,2,3,4,5,6,7,8,9,10");
        recorder.Complete();
        await recorder.Completion;
        CheckEq("OnStopped runs when the actor ends", Volatile.Read(ref Recorder.Stopped), 1);

        var mailbox = new Mailbox<int>(2);
        string posts = mailbox.Post(1) + "," + mailbox.Post(2) + "," + mailbox.Post(3);
        CheckEq("a full bounded mailbox refuses a post", posts, "True,True,False");
        mailbox.Complete();
        CheckEq("and a completed one refuses everything", mailbox.Post(4), false);
    }

    // -- supervision ----------------------------------------------------------

    sealed record Job(string Kind, TaskCompletionSource<string> Reply);

    sealed class Fragile : Actor<Job>
    {
        public static int Started;
        private int _handled;

        protected override void OnStarted() => Interlocked.Increment(ref Started);

        protected override Task ReceiveAsync(Job job)
        {
            if (job.Reply != null)
            {
                job.Reply.SetResult("handled " + _handled);
                return Task.CompletedTask;
            }
            if (job.Kind == "crash") throw new InvalidOperationException("crashed on purpose");
            if (job.Kind == "fatal") throw new NotSupportedException("cannot continue");
            _handled++;
            return Task.CompletedTask;
        }
    }

    static Task<string> Status(ActorRef<Job> actor) => actor.AskAsync<string>(r => new Job("ask", r));

    static async Task Supervision()
    {
        Fragile.Started = 0;
        ActorRef<Job> restarting = Actors.Spawn("restarting", () => new Fragile(), SupervisorStrategy.RestartOnFailure);
        restarting.Post(new Job("work", null));
        restarting.Post(new Job("work", null));
        restarting.Post(new Job("crash", null));
        restarting.Post(new Job("work", null));
        CheckEq("a restart starts from fresh state", await Status(restarting), "handled 1");
        CheckEq("and is counted", restarting.Restarts, 1);
        CheckEq("the factory ran for the original and the replacement", Fragile.Started, 2);
        restarting.Stop();
        await restarting.Completion;

        ActorRef<Job> resuming = Actors.Spawn("resuming", () => new Fragile(), SupervisorStrategy.ResumeOnFailure);
        resuming.Post(new Job("work", null));
        resuming.Post(new Job("crash", null));
        resuming.Post(new Job("work", null));
        CheckEq("resume keeps the state", await Status(resuming), "handled 2");
        resuming.Complete();
        await resuming.Completion;

        // Restart what can be recovered from, escalate what cannot.
        var selective = new SupervisorStrategy(e => e is NotSupportedException
            ? SupervisorDirective.Escalate
            : SupervisorDirective.Restart);
        ActorRef<Job> escalating = Actors.Spawn("escalating", () => new Fragile(), selective);
        escalating.Post(new Job("crash", null));
        escalating.Post(new Job("fatal", null));
        string how = "completed";
        try { await escalating.Completion; }
        catch (ActorFailedException e) { how = e.Actor + ": " + e.InnerException.GetType().Name + " / " + e.Message; }
        CheckEq("escalation faults the completion", how,
            "escalating: NotSupportedException / Actor 'escalating' failed: cannot continue");
        CheckEq("after the restart that came first", escalating.Restarts, 1);

        ActorRef<Job> limited = Actors.Spawn("limited", () => new Fragile(),
            new SupervisorStrategy(_ => SupervisorDirective.Restart, maxRestarts: 2));
        for (int i = 0; i < 5; i++) limited.Post(new Job("crash", null));
        string limit = "completed";
        try { await limited.Completion; }
        catch (ActorFailedException) { limit = "escalated after " + limited.Restarts; }
        CheckEq("too many restarts escalate", limit, "escalated after 2");

        ActorRef<Job> stopping = Actors.Spawn("stopping", () => new Fragile(),
            new SupervisorStrategy(_ => SupervisorDirective.Stop));
        stopping.Post(new Job("crash", null));
        await stopping.Completion;
        Check("a stop directive ends the actor cleanly", stopping.Completion.IsCompletedSuccessfully);
        CheckEq("and its mailbox no longer accepts", stopping.Post(new Job("work", null)), false);
    }

    // -- select ---------------------------------------------------------------

    static async Task Selects()
    {
        Channel<int> numbers = Channel.CreateUnbounded<int>();
        Channel<string> words = Channel.CreateUnbounded<string>();
        int numberSum = 0;
        var seenWords = new List<string>();
        var selector = new Selector()
            .On<int>(numbers.Reader, n => numberSum += n)
            .On<string>(words.Reader, w => seenWords.Add(w));

        Task producers = Task.WhenAll(
            Task.Run(async () =>
            {
                for (int i = 1; i <= 50; i++) { await numbers.Writer.WriteAsync(i); if (i % 10 == 0) await Task.Yield(); }
                numbers.Writer.Complete();
            }),
            Task.Run(async () =>
            {
                foreach (string w in new[] { "delta", "alpha", "charlie", "bravo" }) { await words.Writer.WriteAsync(w); await Task.Yield(); }
                words.Writer.Complete();
            }));
        await selector.RunAsync();
        await producers;
        CheckEq("select drains channels of two types", numberSum, 1275);
        CheckEq("every word arrived", string.Join(",", seenWords.OrderBy(w => w, StringComparer.Ordinal)), "alpha,bravo,charlie,delta");

        // Both ready: the earlier case wins.
        Channel<int> high = Channel.CreateUnbounded<int>();
        Channel<int> low = Channel.CreateUnbounded<int>();
        high.Writer.TryWrite(1);
        low.Writer.TryWrite(2);
        string winner = "";
        var priority = new Selector()
            .On<int>(high.Reader, _ => winner = "high")
            .On<int>(low.Reader, _ => winner = "low");
        await priority.SelectAsync();
        CheckEq("when both are ready the first case wins", winner, "high");
        await priority.SelectAsync();
        CheckEq("and the other is next", winner, "low");

        Channel<int> empty = Channel.CreateUnbounded<int>();
        string ran = "";
        await new Selector().On<int>(empty.Reader, _ => ran = "case").Default(() => ran = "default").SelectAsync();
        CheckEq("default runs when nothing is ready", ran, "default");

        var readers = new[] { Channel.CreateUnbounded<int>(), Channel.CreateUnbounded<int>(), Channel.CreateUnbounded<int>() };
        _ = Task.Run(async () => { await Task.Delay(20); readers[2].Writer.TryWrite(42); });
        var (index, item) = await Select.ReceiveAnyAsync<int>(readers.Select(c => c.Reader).ToArray());
        CheckEq("ReceiveAnyAsync says which channel answered", index + ":" + item, "2:42");

        foreach (var c in readers) c.Writer.Complete();
        string closed = "open";
        try { await Select.ReceiveAnyAsync<int>(readers.Select(c => c.Reader).ToArray()); }
        catch (ChannelClosedException) { closed = "closed"; }
        CheckEq("and throws once every channel is closed", closed, "closed");
    }

    // -- pipelines ------------------------------------------------------------

    static async Task Pipelines()
    {
        List<int> squares = await Pipeline.From(Enumerable.Range(1, 100))
            .Select(x => x * x, parallelism: 4)
            .Where(x => x % 2 == 0)
            .ToListAsync();
        CheckEq("a parallel stage keeps order", string.Join(",", squares.Take(5)), "4,16,36,64,100");
        CheckEq("and loses nothing", squares.Count + ":" + squares.Sum(), "50:171700");

        // Later items finish first; order must still be input order.
        List<int> delayed = await Pipeline.From(Enumerable.Range(1, 8))
            .SelectAsync(async x => { await Task.Delay((9 - x) * 5); return x; }, parallelism: 8)
            .ToListAsync();
        CheckEq("order is input order, not finishing order", string.Join(",", delayed), "1,2,3,4,5,6,7,8");

        List<int[]> batches = await Pipeline.From(Enumerable.Range(1, 10)).Batch(3).ToListAsync();
        CheckEq("batches", string.Join("", batches.Select(b => "[" + string.Join(",", b) + "]")), "[1,2,3][4,5,6][7,8,9][10]");

        string failed = "none";
        try
        {
            await Pipeline.From(Enumerable.Range(1, 20))
                .Select(x => x == 5 ? throw new InvalidOperationException("bad " + x) : x)
                .Select(x => x + 1, parallelism: 2)
                .ToListAsync();
        }
        catch (InvalidOperationException e) { failed = e.Message; }
        CheckEq("a failing stage ends the pipeline with its exception", failed, "bad 5");

        // A bounded channel between stages: the producer cannot run ahead.
        int produced = 0;
        int maxAhead = 0;
        int consumed = 0;
        await Pipeline.From(Produce(), capacity: 2)
            .ForEachAsync(_ =>
            {
                consumed++;
                maxAhead = Math.Max(maxAhead, Volatile.Read(ref produced) - consumed);
                Thread.Sleep(1);
            });
        CheckEq("everything produced was consumed", consumed, 30);
        Check("and the producer never ran far ahead", maxAhead <= 4);

        IEnumerable<int> Produce()
        {
            for (int i = 0; i < 30; i++)
            {
                Interlocked.Increment(ref produced);
                yield return i;
            }
        }

        var source = Channel.CreateUnbounded<string>();
        foreach (string s in new[] { "x", "yy", "zzz" }) source.Writer.TryWrite(s);
        source.Writer.Complete();
        List<int> lengths = await Pipeline.From<string>(source.Reader).Select(s => s.Length).ToListAsync();
        CheckEq("a pipeline can start from a channel", string.Join(",", lengths), "1,2,3");
    }
}
