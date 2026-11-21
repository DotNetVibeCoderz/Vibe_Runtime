using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;

namespace Conformance;

// Milestone 8.3: what RustNet.Concurrency needed from the runtime, found by
// running tests/fixtures/Concurrency under RustCLR. Each check fails without
// the fix it names.
static partial class Program
{
    static void StructuredConcurrencyChecks()
    {
        AggregateExceptionChecks();
        OwnExceptionChecks();
        TaskStatusChecks();
        DelayTokenChecks();
        GenericToArrayChecks();
        UnwrapHoldsNoWorkerChecks();
        LostRaceChecks();
        IntrinsicChecks();
        LibraryGuardChecks();
        Check("GC.KeepAlive in a loop keeps nothing alive afterwards", VoidCallsKeepNothing(200) < 100_000);
    }

    // `await` on a task that finishes between `IsCompleted` and the
    // continuation being registered. The continuation used to run nested
    // inside the suspending `MoveNext`, so a loop that kept losing that race
    // kept nesting, and overflowed a pool thread's stack at random a few
    // thousand iterations in — the process died. Fifty thousand hops make
    // losing it often enough a near certainty.
    static void LostRaceChecks()
    {
        long sum = HopMany(50_000).Result;
        Check("50,000 awaits of Task.Run complete without overflowing a stack", sum == 50_000L * 49_999 / 2);
    }

    static async Task<long> HopMany(int n)
    {
        long sum = 0;
        for (int i = 0; i < n; i++)
        {
            int k = i;
            sum += await Task.Run(() => k);
        }
        return sum;
    }

    // `new AggregateException(IEnumerable<Exception>)` and `InnerExceptions`:
    // a scope reports every child that failed this way.
    static void AggregateExceptionChecks()
    {
        var parts = new List<Exception> { new InvalidOperationException("a"), new ArgumentException("b") };
        var many = new AggregateException("Several failed.", parts);
        Check("AggregateException keeps every inner exception", many.InnerExceptions.Count == 2);
        Check("in order", many.InnerExceptions[0].Message == "a" && many.InnerExceptions[1].Message == "b");
        Check("and the first is InnerException", many.InnerException?.Message == "a");
        Check("its message lists them as .NET does", many.Message == "Several failed. (a) (b)");

        var array = new AggregateException(new Exception("x"), new Exception("y"));
        Check("the params overload too", array.InnerExceptions.Count == 2 && array.Message == "One or more errors occurred. (x) (y)");

        var nested = new AggregateException(new Exception("p"), new AggregateException(new Exception("q"), new Exception("r")));
        var flat = nested.Flatten();
        Check("Flatten pulls nested aggregates up", string.Join(",", flat.InnerExceptions.Select(e => e.Message)) == "p,q,r");

        int seen = 0;
        foreach (var e in many.InnerExceptions) seen++;
        Check("InnerExceptions enumerates", seen == 2);

        // What `Task.Wait` throws around one failure.
        var failed = Task.Run(() => { throw new FormatException("bad"); });
        try { failed.Wait(); }
        catch (AggregateException e)
        {
            Check("Wait's AggregateException has one inner exception", e.InnerExceptions.Count == 1 && e.InnerExceptions[0] is FormatException);
        }
    }

    sealed class OwnFailure : Exception
    {
        public OwnFailure(string who, Exception inner) : base("failed: " + who, inner)
        {
            Who = who;
        }

        public string Who { get; }
    }

    // A program's own exception with a field of its own and `base(message,
    // inner)`. The inner exception was kept past Exception's declared slots,
    // which is the subclass's first field, so assigning `Who` replaced it.
    static void OwnExceptionChecks()
    {
        var e = new OwnFailure("actor", new NotSupportedException("deep"));
        Check("an own exception keeps its inner exception", e.InnerException is NotSupportedException);
        Check("and its message", e.Message == "failed: actor");
        Check("and its own field", e.Who == "actor");
        Check("GetBaseException reaches the inner one", e.GetBaseException().Message == "deep");
    }

    static void TaskStatusChecks()
    {
        var source = new TaskCompletionSource<int>();
        string pending = source.Task.Status.ToString();
        source.SetResult(1);
        Check("TaskStatus prints its name", pending == "WaitingForActivation" && source.Task.Status.ToString() == "RanToCompletion");
        var failed = Task.FromException(new Exception());
        Check("and Faulted", failed.Status == TaskStatus.Faulted);
    }

    // `Task.Delay(int, CancellationToken)`: the token is a value type from
    // another assembly, so its typed binding key never matched and the call
    // had no implementation.
    static void DelayTokenChecks()
    {
        var cts = new CancellationTokenSource();
        Task delay = Task.Delay(60_000, cts.Token);
        cts.Cancel();
        string how = "none";
        try { delay.Wait(); }
        catch (AggregateException e) { how = e.InnerExceptions[0].GetType().Name; }
        Check("Task.Delay(int, token) ends when the token is cancelled", delay.IsCanceled && how == "TaskCanceledException");

        var timed = new CancellationTokenSource();
        Task span = Task.Delay(TimeSpan.FromMinutes(1), timed.Token);
        timed.Cancel();
        string spanHow = "none";
        try { span.Wait(5_000); }
        catch (AggregateException e) { spanHow = e.InnerExceptions[0].GetType().Name; }
        Check("and Task.Delay(TimeSpan, token)", span.IsCanceled && spanHow == "TaskCanceledException");
    }

    sealed class SnapshotList<T>
    {
        private readonly List<T> _items = new List<T>();
        public void Add(T item) => _items.Add(item);
        public T[] Snapshot() => _items.ToArray();
    }

    // `_items.ToArray()` inside `SnapshotList<T>` names `List<!0>`; load time bound
    // that to a placeholder, and the array it built read every int as zero.
    static void GenericToArrayChecks()
    {
        var holder = new SnapshotList<int>();
        holder.Add(4);
        holder.Add(7);
        int[] items = holder.Snapshot();
        Check("List<T>.ToArray inside a generic class keeps the values", items.Length == 2 && items[0] == 4 && items[1] == 7);
        var strings = new SnapshotList<string>();
        strings.Add("s");
        Check("for reference types too", strings.Snapshot()[0] == "s");
    }

    // `Task.Run(async () => …)` whose inner task lives a long time — an
    // actor's message loop. The outer task used to be settled by a pool job
    // that *waited* for the inner one, so each held a worker, and enough of
    // them left nothing to run anything else.
    static void UnwrapHoldsNoWorkerChecks()
    {
        var gate = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        var loops = new Task[300];
        for (int i = 0; i < loops.Length; i++)
            loops[i] = Task.Run(async () => { await gate.Task; });
        Task<int> other = Task.Run(() => 42);
        bool ran = other.Wait(10_000);
        gate.SetResult(true);
        Task.WaitAll(loops);
        Check("long-lived async Task.Run bodies leave the pool free", ran && other.Result == 42);
        Check("and each outer task finishes with its inner one", loops.All(t => t.Status == TaskStatus.RanToCompletion));
    }
}
