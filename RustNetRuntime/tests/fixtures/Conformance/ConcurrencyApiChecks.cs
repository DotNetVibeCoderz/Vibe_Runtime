using System;
using System.Collections.Concurrent;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Channels;
using System.Threading.Tasks;

namespace Conformance;

// Milestone 8.1: the official concurrency surface — System.Collections.Concurrent,
// System.Threading.Channels, SemaphoreSlim, ThreadLocal<T> and [ThreadStatic].
// Every expected value was produced by dotnet. Anything whose order .NET does
// not promise (a bag, several writers) is checked as a sum or a sorted list.
static partial class Program
{
    static void ConcurrencyApiChecks()
    {
        ConcurrentQueueChecks();
        ConcurrentStackChecks();
        ConcurrentBagChecks();
        ConcurrentDictionaryChecks();
        BlockingCollectionChecks();
        SemaphoreSlimChecks();
        ChannelChecks();
        ThreadLocalChecks();
        ThreadStaticChecks();
    }

    // ── ConcurrentQueue ──────────────────────────────────────────────────────

    static void ConcurrentQueueChecks()
    {
        var q = new ConcurrentQueue<int>();
        var empty = q.IsEmpty;
        for (int i = 1; i <= 5; i++) q.Enqueue(i);
        q.TryPeek(out var head);
        q.TryDequeue(out var first);
        var items = string.Join(",", q);
        var array = string.Join(",", q.ToArray());
        var drained = 0;
        while (q.TryDequeue(out var v)) drained += v;
        var afterDrain = q.TryDequeue(out var none);
        CheckStr("ConcurrentQueue: FIFO, peek, drain",
            $"{empty}|{head}|{first}|{items}|{array}|{drained}|{afterDrain}|{none}|{q.Count}|{q.IsEmpty}",
            "True|1|1|2,3,4,5|2,3,4,5|14|False|0|0|True");

        var seeded = new ConcurrentQueue<string>(new[] { "a", "b" });
        CheckStr("ConcurrentQueue: seeded from a collection", string.Join("", seeded) + seeded.Count, "ab2");

        var shared = new ConcurrentQueue<int>();
        var writers = new Thread[4];
        for (int t = 0; t < writers.Length; t++)
        {
            int start = t * 1000;
            writers[t] = new Thread(() => { for (int i = 1; i <= 1000; i++) shared.Enqueue(start + i); });
            writers[t].Start();
        }
        foreach (var w in writers) w.Join();
        long total = 0;
        int taken = 0;
        while (shared.TryDequeue(out var v)) { total += v; taken++; }
        CheckStr("ConcurrentQueue: four writer threads lose nothing", $"{taken}|{total}", "4000|8002000");
    }

    // ── ConcurrentStack ──────────────────────────────────────────────────────

    static void ConcurrentStackChecks()
    {
        var s = new ConcurrentStack<int>();
        s.Push(1);
        s.Push(2);
        s.PushRange(new[] { 3, 4, 5 });
        s.TryPeek(out var top);
        s.TryPop(out var popped);
        var buffer = new int[2];
        var many = s.TryPopRange(buffer);
        var rest = string.Join(",", s.ToArray());
        s.Clear();
        var none = s.TryPop(out var zero);
        CheckStr("ConcurrentStack: LIFO, ranges, clear",
            $"{top}|{popped}|{many}|{buffer[0]},{buffer[1]}|{rest}|{none}|{zero}|{s.IsEmpty}",
            "5|5|2|4,3|2,1|False|0|True");
    }

    // ── ConcurrentBag ────────────────────────────────────────────────────────

    static void ConcurrentBagChecks()
    {
        var bag = new ConcurrentBag<int>();
        Parallel.For(1, 101, i => bag.Add(i));
        var count = bag.Count;
        var sum = bag.Sum();
        var sorted = bag.OrderBy(x => x).Take(3).ToArray();
        var peeked = bag.TryPeek(out _);
        int takenSum = 0;
        while (bag.TryTake(out var v)) takenSum += v;
        CheckStr("ConcurrentBag: adds from a parallel loop, then taken",
            $"{count}|{sum}|{string.Join(",", sorted)}|{peeked}|{takenSum}|{bag.IsEmpty}",
            "100|5050|1,2,3|True|5050|True");
    }

    // ── ConcurrentDictionary ─────────────────────────────────────────────────

    static void ConcurrentDictionaryChecks()
    {
        var d = new ConcurrentDictionary<string, int>();
        var added = d.TryAdd("one", 1);
        var again = d.TryAdd("one", 100);
        d["two"] = 2;
        var got = d.TryGetValue("two", out var two);
        var missing = d.TryGetValue("nine", out var nine);
        var factoryCalls = 0;
        var three = d.GetOrAdd("three", k => { factoryCalls++; return k.Length; });
        var threeAgain = d.GetOrAdd("three", k => { factoryCalls++; return -1; });
        var four = d.GetOrAdd("four", 4);
        var updated = d.AddOrUpdate("one", 0, (k, old) => old + 10);
        var inserted = d.AddOrUpdate("five", 5, (k, old) => old + 10);
        var swapped = d.TryUpdate("two", 22, 2);
        var notSwapped = d.TryUpdate("two", 99, 2);
        var removed = d.TryRemove("four", out var gone);
        var notRemoved = d.TryRemove("four", out _);
        var keys = string.Join(",", d.Keys.OrderBy(k => k));
        var values = string.Join(",", d.Values.OrderBy(v => v));
        var pairs = string.Join(",", d.OrderBy(p => p.Key).Select(p => p.Key + "=" + p.Value));
        CheckStr("ConcurrentDictionary: add, get, update, remove",
            $"{added}|{again}|{got}|{two}|{missing}|{nine}|{three}|{threeAgain}|{factoryCalls}|{four}|{updated}|{inserted}|{swapped}|{notSwapped}|{removed}|{gone}|{notRemoved}|{d.Count}|{d.ContainsKey("two")}",
            "True|False|True|2|False|0|5|5|1|4|11|5|True|False|True|4|False|4|True");
        CheckStr("ConcurrentDictionary: keys, values and pairs",
            $"{keys}|{values}|{pairs}",
            "five,one,three,two|5,5,11,22|five=5,one=11,three=5,two=22");

        var thrown = "";
        try { _ = d["absent"]; }
        catch (KeyNotFoundException e) { thrown = e.GetType().Name; }
        CheckStr("ConcurrentDictionary: a missing key throws KeyNotFoundException", thrown, "KeyNotFoundException");

        var counters = new ConcurrentDictionary<int, int>();
        var threads = new Thread[4];
        for (int t = 0; t < threads.Length; t++)
        {
            threads[t] = new Thread(() =>
            {
                for (int i = 0; i < 1000; i++) counters.AddOrUpdate(i % 10, 1, (_, old) => old + 1);
            });
            threads[t].Start();
        }
        foreach (var th in threads) th.Join();
        CheckStr("ConcurrentDictionary: AddOrUpdate from four threads loses no increment",
            $"{counters.Count}|{counters.Values.Sum()}|{counters[3]}", "10|4000|400");

        var removedPair = d.TryRemove(new KeyValuePair<string, int>("two", 22));
        var array = d.ToArray().OrderBy(p => p.Key).Select(p => p.Key);
        d.Clear();
        CheckStr("ConcurrentDictionary: remove a pair, ToArray, Clear",
            $"{removedPair}|{string.Join(",", array)}|{d.Count}|{d.IsEmpty}", "True|five,one,three|0|True");
    }

    // ── BlockingCollection ───────────────────────────────────────────────────

    static void BlockingCollectionChecks()
    {
        using var items = new BlockingCollection<int>(boundedCapacity: 4);
        var producer = new Thread(() =>
        {
            for (int i = 1; i <= 50; i++) items.Add(i);
            items.CompleteAdding();
        });
        producer.Start();
        int sum = 0, count = 0;
        foreach (var v in items.GetConsumingEnumerable()) { sum += v; count++; }
        producer.Join();
        var takeAfter = items.TryTake(out var nothing);
        var addThrew = "";
        try { items.Add(1); }
        catch (InvalidOperationException e) { addThrew = e.GetType().Name; }
        CheckStr("BlockingCollection: bounded producer/consumer, completion",
            $"{count}|{sum}|{items.IsAddingCompleted}|{items.IsCompleted}|{takeAfter}|{nothing}|{addThrew}|{items.BoundedCapacity}",
            "50|1275|True|True|False|0|InvalidOperationException|4");

        var stack = new BlockingCollection<int>(new ConcurrentStack<int>());
        stack.Add(1);
        stack.Add(2);
        var t1 = stack.Take();
        var timedOut = stack.TryTake(out var t2, 10);
        var empty = stack.TryTake(out _, 10);
        CheckStr("BlockingCollection: over a stack, TryTake with a timeout",
            $"{t1}|{timedOut}|{t2}|{empty}|{stack.Count}|{stack.BoundedCapacity}", "2|True|1|False|0|-1");
    }

    // ── SemaphoreSlim ────────────────────────────────────────────────────────

    static void SemaphoreSlimChecks()
    {
        using var gate = new SemaphoreSlim(2, 3);
        var a = gate.Wait(0);
        var b = gate.Wait(0);
        var c = gate.Wait(0);
        var before = gate.CurrentCount;
        var previous = gate.Release();
        var previous2 = gate.Release(2);
        var tooMany = "";
        try { gate.Release(); }
        catch (SemaphoreFullException e) { tooMany = e.GetType().Name; }
        CheckStr("SemaphoreSlim: counts, Wait(0), Release",
            $"{a}|{b}|{c}|{before}|{previous}|{previous2}|{gate.CurrentCount}|{tooMany}",
            "True|True|False|0|0|1|3|SemaphoreFullException");

        var limit = new SemaphoreSlim(2);
        int inside = 0, peak = 0, done = 0;
        var workers = Enumerable.Range(0, 8).Select(_ => Task.Run(async () =>
        {
            await limit.WaitAsync();
            try
            {
                var now = Interlocked.Increment(ref inside);
                int seen;
                while (now > (seen = Volatile.Read(ref peak)) && Interlocked.CompareExchange(ref peak, now, seen) != seen) { }
                await Task.Delay(5);
                Interlocked.Decrement(ref inside);
                Interlocked.Increment(ref done);
            }
            finally { limit.Release(); }
        })).ToArray();
        Task.WaitAll(workers);
        CheckStr("SemaphoreSlim: WaitAsync admits at most two at a time",
            $"{done}|{(peak <= 2 && peak >= 1)}|{limit.CurrentCount}", "8|True|2");

        var timed = new SemaphoreSlim(0);
        var timedResult = timed.Wait(20);
        var asyncTimed = timed.WaitAsync(20).Result;
        CheckStr("SemaphoreSlim: Wait and WaitAsync time out", $"{timedResult}|{asyncTimed}", "False|False");
    }

    // ── Channels ─────────────────────────────────────────────────────────────

    static void ChannelChecks()
    {
        var ch = Channel.CreateUnbounded<int>();
        var w1 = ch.Writer.TryWrite(1);
        ch.Writer.TryWrite(2);
        ch.Writer.TryWrite(3);
        var canCount = ch.Reader.CanCount;
        var count = ch.Reader.Count;
        ch.Reader.TryPeek(out var peek);
        ch.Reader.TryRead(out var r1);
        ch.Writer.Complete();
        var afterComplete = ch.Writer.TryWrite(4);
        var rest = new List<int>();
        while (ch.Reader.TryRead(out var v)) rest.Add(v);
        var more = ch.Reader.WaitToReadAsync().AsTask().Result;
        var finished = ch.Reader.Completion.IsCompleted;
        CheckStr("Channel: unbounded TryWrite/TryRead, completion",
            $"{w1}|{canCount}|{count}|{peek}|{r1}|{afterComplete}|{string.Join(",", rest)}|{more}|{finished}",
            "True|True|3|1|1|False|2,3|False|True");

        var closed = Channel.CreateUnbounded<string>();
        closed.Writer.Complete();
        var writeThrew = "";
        try { closed.Writer.WriteAsync("x").AsTask().Wait(); }
        catch (AggregateException e) { writeThrew = e.InnerException.GetType().Name; }
        var readThrew = "";
        try { closed.Reader.ReadAsync().AsTask().Wait(); }
        catch (AggregateException e) { readThrew = e.InnerException.GetType().Name; }
        var completeTwice = closed.Writer.TryComplete();
        CheckStr("Channel: writing to or reading from a completed channel",
            $"{writeThrew}|{readThrew}|{completeTwice}", "ChannelClosedException|ChannelClosedException|False");

        var failed = Channel.CreateUnbounded<int>();
        failed.Writer.Complete(new InvalidOperationException("boom"));
        var why = "";
        try { failed.Reader.Completion.Wait(); }
        catch (AggregateException e) { why = e.InnerException.GetType().Name + ":" + e.InnerException.Message; }
        CheckStr("Channel: completing with an error faults Completion", why, "InvalidOperationException:boom");

        CheckStr("Channel: producer and consumer tasks, ReadAllAsync", ChannelPipeline().Result, "100|5050|True");
        CheckStr("Channel: a bounded channel makes the writer wait", BoundedBackPressure().Result, "True|1|2|2,3");

        foreach (var mode in new[] { BoundedChannelFullMode.DropOldest, BoundedChannelFullMode.DropNewest, BoundedChannelFullMode.DropWrite })
        {
            var dropping = Channel.CreateBounded<int>(new BoundedChannelOptions(2) { FullMode = mode });
            var results = new List<bool>();
            for (int i = 1; i <= 4; i++) results.Add(dropping.Writer.TryWrite(i));
            var kept = new List<int>();
            while (dropping.Reader.TryRead(out var v)) kept.Add(v);
            var expected = mode switch
            {
                BoundedChannelFullMode.DropOldest => "True,True,True,True|3,4",
                BoundedChannelFullMode.DropNewest => "True,True,True,True|1,4",
                _ => "True,True,True,True|1,2", // DropWrite still answers true
            };
            CheckStr("Channel: bounded " + mode, string.Join(",", results) + "|" + string.Join(",", kept), expected);
        }

        var dropped = new List<int>();
        var withCallback = Channel.CreateBounded<int>(new BoundedChannelOptions(1) { FullMode = BoundedChannelFullMode.DropOldest }, x => dropped.Add(x));
        withCallback.Writer.TryWrite(1);
        withCallback.Writer.TryWrite(2);
        withCallback.Writer.TryWrite(3);
        CheckStr("Channel: the item-dropped callback sees what was dropped", string.Join(",", dropped), "1,2");

        var waitMode = Channel.CreateBounded<int>(1);
        var firstFits = waitMode.Writer.TryWrite(1);
        var secondDoesNot = waitMode.Writer.TryWrite(2);
        var waitToWrite = waitMode.Writer.WaitToWriteAsync().AsTask();
        var pendingBeforeRead = waitToWrite.IsCompleted;
        waitMode.Reader.TryRead(out _);
        var writable = waitToWrite.Result;
        CheckStr("Channel: bounded Wait mode, WaitToWriteAsync",
            $"{firstFits}|{secondDoesNot}|{pendingBeforeRead}|{writable}", "True|False|False|True");

        var opts = Channel.CreateUnbounded<int>(new UnboundedChannelOptions { SingleReader = true, SingleWriter = true });
        opts.Writer.TryWrite(7);
        CheckStr("Channel: single reader/writer options still deliver", opts.Reader.ReadAsync().AsTask().Result.ToString(), "7");

        CheckStr("Channel: cancelling a pending ReadAsync", CancelledRead(), "True|TaskCanceledException");
        CheckStr("Channel: several writer tasks, one reader", FanIn().Result, "400|80200");
    }

    static async Task<string> ChannelPipeline()
    {
        var ch = Channel.CreateUnbounded<int>();
        var producer = Task.Run(async () =>
        {
            for (int i = 1; i <= 100; i++) await ch.Writer.WriteAsync(i);
            ch.Writer.Complete();
        });
        int count = 0, sum = 0;
        await foreach (var v in ch.Reader.ReadAllAsync()) { count++; sum += v; }
        await producer;
        return $"{count}|{sum}|{ch.Reader.Completion.IsCompleted}";
    }

    static async Task<string> BoundedBackPressure()
    {
        var ch = Channel.CreateBounded<int>(new BoundedChannelOptions(2) { FullMode = BoundedChannelFullMode.Wait });
        await ch.Writer.WriteAsync(1);
        await ch.Writer.WriteAsync(2);
        var third = ch.Writer.WriteAsync(3).AsTask();
        await Task.Delay(20);
        var stalled = !third.IsCompleted;
        var got = await ch.Reader.ReadAsync();
        await third;
        var count = ch.Reader.Count;
        ch.Writer.Complete();
        var rest = new List<int>();
        await foreach (var v in ch.Reader.ReadAllAsync()) rest.Add(v);
        return $"{stalled}|{got}|{count}|{string.Join(",", rest)}";
    }

    static string CancelledRead()
    {
        var ch = Channel.CreateUnbounded<int>();
        using var cts = new CancellationTokenSource();
        var pending = ch.Reader.ReadAsync(cts.Token).AsTask();
        var wasPending = !pending.IsCompleted;
        cts.Cancel();
        var why = "";
        try { pending.Wait(); }
        catch (AggregateException e) { why = e.InnerException.GetType().Name; }
        return $"{wasPending}|{why}";
    }

    static async Task<string> FanIn()
    {
        var ch = Channel.CreateUnbounded<int>();
        var writers = Enumerable.Range(0, 4).Select(w => Task.Run(async () =>
        {
            for (int i = 1; i <= 100; i++) await ch.Writer.WriteAsync(w * 100 + i);
        })).ToArray();
        var completer = Task.WhenAll(writers).ContinueWith(_ => ch.Writer.Complete());
        int count = 0;
        long sum = 0;
        while (await ch.Reader.WaitToReadAsync())
            while (ch.Reader.TryRead(out var v)) { count++; sum += v; }
        await completer;
        return $"{count}|{sum}";
    }

    // ── ThreadLocal<T> and [ThreadStatic] ────────────────────────────────────

    static void ThreadLocalChecks()
    {
        int created = 0;
        using var local = new ThreadLocal<int>(() => Interlocked.Increment(ref created) * 10, trackAllValues: true);
        var before = local.IsValueCreated;
        var mine = local.Value;
        local.Value = mine + 1;
        int other = 0;
        var t = new Thread(() => { other = local.Value; local.Value = 99; });
        t.Start();
        t.Join();
        var all = string.Join(",", local.Values.OrderBy(v => v));
        CheckStr("ThreadLocal: one value per thread, factory once per thread",
            $"{before}|{mine}|{local.Value}|{other}|{created}|{local.IsValueCreated}|{all}",
            "False|10|11|20|2|True|11,99");

        var plain = new ThreadLocal<string>();
        CheckStr("ThreadLocal: no factory gives default", (plain.Value == null) + "|" + plain.IsValueCreated, "True|True");
    }

    [ThreadStatic] static int s_perThread;
    [ThreadStatic] static string s_perThreadName;
    static int s_shared;

    static void ThreadStaticChecks()
    {
        s_perThread = 5;
        s_perThreadName = "main";
        s_shared = 5;
        int seenOther = -1;
        string seenName = "unset";
        var t = new Thread(() =>
        {
            seenOther = s_perThread;
            seenName = s_perThreadName ?? "null";
            s_perThread = 42;
            s_shared = 42;
        });
        t.Start();
        t.Join();
        CheckStr("[ThreadStatic]: each thread sees its own slot; an ordinary static is shared",
            $"{s_perThread}|{s_perThreadName}|{seenOther}|{seenName}|{s_shared}", "5|main|0|null|42");
    }
}
