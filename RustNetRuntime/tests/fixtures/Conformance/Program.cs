using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using System.Reflection;
using System.Runtime.InteropServices;

namespace Conformance;

interface IShape
{
    double Area();
    string Describe();
}

abstract class Shape : IShape
{
    public string Name { get; set; }
    protected Shape(string name) { Name = name; }
    public abstract double Area();
    public virtual string Describe() { return Name + " area=" + Area(); }
}

sealed class Rect : Shape
{
    private readonly double w, h;
    public Rect(double w, double h) : base("rect") { this.w = w; this.h = h; }
    public override double Area() { return w * h; }
}

sealed class Circle : Shape
{
    private readonly double r;
    public Circle(double r) : base("circle") { this.r = r; }
    public override double Area() { return 3.14159 * r * r; }
    public override string Describe() { return "circle r=" + r; }
}

struct Point
{
    public int X;
    public int Y;
    public int Sum() { return X + Y; }
}

enum Level { Low = 1, Mid = 5, High = 9 }

/// An attribute with a constructor argument, a named field and a named
/// property — the three shapes an attribute blob encodes.
[AttributeUsage(AttributeTargets.All)]
sealed class MarkAttribute : Attribute
{
    public string Text;
    public int Order { get; set; }
    public bool Enabled;
    public MarkAttribute(string text) { Text = text; }
}

[Mark("on the type", Order = 3, Enabled = true)]
sealed class Marked
{
    [Mark("on a field")] public int Slot;
    [Mark("on a method")] public int Twice(int n) { return n * 2; }
}

/// A type that genuinely overrides ToString, so dispatch to it can be checked.
readonly record struct Point2(double X, double Y);

sealed class TypedStore<T>
{
    private T[] _items = new T[2];
    public int Count { get; private set; }
    public void Add(T item)
    {
        if (Count == _items.Length) Array.Resize(ref _items, Count * 2);
        _items[Count++] = item;
    }
    public T At(int i) => _items[i];
}

sealed class TypedRegistry
{
    private readonly Dictionary<Type, object> _stores = new();
    public int Count => _stores.Count;
    private TypedStore<T> Store<T>()
    {
        if (_stores.TryGetValue(typeof(T), out var s)) return (TypedStore<T>)s;
        var created = new TypedStore<T>();
        _stores[typeof(T)] = created;
        return created;
    }
    public void Put<T>(T item) => Store<T>().Add(item);
    public T Get<T>(int i) => Store<T>().At(i);
}

sealed class AsyncBox<T> where T : new()
{
    public async Task<string> DescribeAsync()
    {
        await Task.CompletedTask;
        return typeof(T).Name + ":" + new T();
    }
}

sealed class CodedException : Exception
{
    public CodedException(int code, string message) : base(message) => Code = code;
    public int Code { get; }
}

sealed class NamedOperationException : InvalidOperationException
{
    public NamedOperationException(string operation, string message) : base(operation + ": " + message) => Operation = operation;
    public string Operation { get; }
}

sealed class PropertyBag : Dictionary<string, object?> { }

enum Shade { Light, Dark }

/// <summary>What a serializer reads off a type to know what to build.</summary>
sealed class Shapes
{
    public List<Tag> Tags { get; set; } = new();
    public int[] Counts { get; set; } = new int[0];
    public Dictionary<string, double> Weights { get; set; } = new();
    public int? Maybe { get; set; }
    public Shade Shade { get; set; }
    public string this[int index] => index.ToString();
}

sealed class Tag
{
    private readonly string text;
    public Tag(string text) { this.text = text; }
    public override string ToString() { return "tag:" + text; }
    public int Echo(int n) { return n * 2; }
}

/// A type with properties, for reflection to walk.
///
/// `Celsius` is read-write, `Fahrenheit` is computed and read-only, and
/// `Origin` is static — three shapes `GetProperties` has to tell apart.
sealed class Reading
{
    public double Celsius { get; set; }
    public double Fahrenheit { get { return Celsius * 9 / 5 + 32; } }
    public static string Origin { get; set; } = "lab";
    public int Scale(int by, int offset) { return (int)Celsius * by + offset; }
}

/// Orders strings by length, for `List<T>.Sort(IComparer<T>)`.
sealed class ByLength : IComparer<string>
{
    public int Compare(string a, string b)
    {
        int left = a == null ? 0 : a.Length;
        int right = b == null ? 0 : b.Length;
        return left - right;
    }
}

/// A user generic type, for the per-construction behaviour below.
///
/// `Cell<int>` and `Cell<string>` are two runtime types here. They share one
/// body — generics are still erased for *execution* — but each carries its own
/// type arguments and its own static storage.
sealed class Cell<T>
{
    public T Value;
    public string ArgumentName() { return typeof(T).Name; }
    public bool Accepts(object o) { return o is T; }
    public T Empty() { return default(T); }
}

/// Statics belong to the construction, not the definition.
static class Tally<T>
{
    public static int Count;

    /// No receiver exists here, so `typeof(T)` can only be answered by the
    /// construction the *call site* named.
    public static string ArgumentName() { return typeof(T).Name; }
    public static bool Accepts(object o) { return o is T; }
    public static T Empty() { return default(T); }
}

/// Instance generic methods, which C# calls with `callvirt`. The call site
/// names the instantiation; dispatch must not trade it for the open
/// definition, or `typeof(T)` inside has nothing to read.
class Registrar
{
    public string NameOf<T>() { return typeof(T).Name; }
    public virtual string VirtualNameOf<T>() { return "base:" + typeof(T).Name; }
    public string Hidden() { return "base"; }
}

sealed class DerivedRegistrar : Registrar
{
    public override string VirtualNameOf<T>() { return "derived:" + typeof(T).Name; }
    public new string Hidden() { return "derived"; }
}

sealed class Node
{
    public int Value;
    public Node Next;
}

static partial class Program
{
    static int checks = 0;
    static int failures = 0;

    static void Check(string label, bool ok)
    {
        checks++;
        if (!ok) { failures++; Console.WriteLine("FAIL " + label); }
    }

    static void CheckEq(string label, int actual, int expected)
    {
        Check(label + " (got " + actual + " want " + expected + ")", actual == expected);
    }

    static void CheckStr(string label, string actual, string expected)
    {
        Check(label + " (got '" + actual + "' want '" + expected + "')", actual == expected);
    }

    // -- generic collections -------------------------------------------------
    // Milestone 2. Each of these fails outright without real generic support:
    // the type does not resolve, the member does not bind, or `foreach` cannot
    // find an enumerator.

    static int SumSequence(IEnumerable<int> xs)
    {
        int total = 0;
        foreach (int x in xs) total += x;
        return total;
    }

    static string JoinList(List<string> items)
    {
        string acc = "";
        foreach (string s in items) acc = acc + s + ";";
        return acc;
    }

    static int DictionaryRoundTrip()
    {
        Dictionary<string, int> d = new Dictionary<string, int>();
        for (int i = 0; i < 200; i++) d["k" + i] = i * 2;
        int total = 0;
        for (int i = 0; i < 200; i++)
        {
            int got;
            if (d.TryGetValue("k" + i, out got)) total += got;
        }
        return total;
    }

    // -- async and await -----------------------------------------------------
    // Milestone 3. An async method is lowered to a state machine plus calls
    // into a builder; none of these run without that builder implemented.

    static async Task<int> Doubled(int n)
    {
        await Task.Yield();
        return n * 2;
    }

    static async Task<int> ChainedAwaits(int n)
    {
        int a = await Doubled(n);
        int b = await Doubled(a);
        return a + b;
    }

    static async Task<int> AwaitInLoop(int n)
    {
        int total = 0;
        for (int i = 1; i <= n; i++) total += await Doubled(i);
        return total;
    }

    // `ValueTask` and `await using`. `DisposeAsync` returns a `ValueTask`, so
    // neither runs without one.

    sealed class AsyncResource : IAsyncDisposable
    {
        public static string Log = "";
        public AsyncResource() { Log += "open;"; }
        public async ValueTask DisposeAsync()
        {
            await Task.Yield();
            Log += "closed;";
        }
    }

    static async Task<string> UsedAsync()
    {
        AsyncResource.Log = "";
        await using (var r = new AsyncResource())
        {
            AsyncResource.Log += "body;";
            await Task.Yield();
        }
        return AsyncResource.Log;
    }

    static async ValueTask<int> TripledAsync(int n)
    {
        await Task.Yield();
        return n * 3;
    }

    static async ValueTask NothingAsync()
    {
        await Task.Yield();
    }

    static async Task Throwing()
    {
        await Task.Yield();
        throw new InvalidOperationException("async boom");
    }

    // Async iterators. The compiler lowers one into a state machine that is
    // its own `IAsyncEnumerable<T>`, `IAsyncEnumerator<T>` and
    // `IValueTaskSource<bool>`, so none of this runs without the builder that
    // drives it and the promise `MoveNextAsync` hands back.

    static string StackallocSpan()
    {
        Span<int> numbers = stackalloc int[4];
        for (int i = 0; i < numbers.Length; i++) numbers[i] = i * i;

        string seen = "";
        foreach (int v in numbers) seen += (seen.Length == 0 ? "" : ",") + v;

        Span<int> tail = numbers.Slice(2);
        int total = 0;
        for (int i = 0; i < tail.Length; i++) total += tail[i];
        return seen + "/" + total;
    }

    struct Blit { public int X; public int Y; }
    struct Small { public byte A; public byte B; }
    struct Wide { public int A; public short B; public long C; }

    static string RoundTrip()
    {
        IntPtr buffer = Marshal.AllocHGlobal(Marshal.SizeOf<Blit>());
        try
        {
            Marshal.StructureToPtr(new Blit { X = 3, Y = 4 }, buffer, false);
            Blit back = Marshal.PtrToStructure<Blit>(buffer);
            return back.X + "/" + back.Y;
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }

    static string WideRoundTrip()
    {
        IntPtr buffer = Marshal.AllocHGlobal(Marshal.SizeOf<Wide>());
        try
        {
            Marshal.StructureToPtr(new Wide { A = 70000, B = -5, C = 9000000000L }, buffer, false);
            Wide back = Marshal.PtrToStructure<Wide>(buffer);
            return back.A + "/" + back.B + "/" + back.C;
        }
        finally { Marshal.FreeHGlobal(buffer); }
    }

    // Threads. `Thread.Start` spawns a real OS thread that shares this one's
    // heap and static storage.

    static int threadedTotal;
    static int lockedTotal;
    static readonly object threadGate = new object();

    static int ThreadedSum()
    {
        threadedTotal = 0;
        Thread[] workers = new Thread[4];
        for (int t = 0; t < 4; t++)
        {
            workers[t] = new Thread(() =>
            {
                for (int i = 0; i < 1000; i++) Interlocked.Increment(ref threadedTotal);
            });
            workers[t].Start();
        }
        for (int t = 0; t < 4; t++) workers[t].Join();
        return threadedTotal;
    }

    static int LockedCounter()
    {
        lockedTotal = 0;
        Thread[] workers = new Thread[4];
        for (int t = 0; t < 4; t++)
        {
            workers[t] = new Thread(() =>
            {
                for (int i = 0; i < 2000; i++) lock (threadGate) { lockedTotal++; }
            });
            workers[t].Start();
        }
        for (int t = 0; t < 4; t++) workers[t].Join();
        return lockedTotal;
    }

    static int InterlockedCounter()
    {
        int counter = 0;
        Thread[] workers = new Thread[4];
        for (int t = 0; t < 4; t++)
        {
            workers[t] = new Thread(() =>
            {
                for (int i = 0; i < 5000; i++) Interlocked.Increment(ref counter);
            });
            workers[t].Start();
        }
        for (int t = 0; t < 4; t++) workers[t].Join();
        return counter;
    }

    /// The one that cannot pass without overlap: this thread waits for a flag
    /// set by a thread started after it began waiting.
    static bool Overlaps()
    {
        bool ready = false;
        Thread producer = new Thread(() => { Thread.Sleep(20); Volatile.Write(ref ready, true); });
        producer.Start();
        int spins = 0;
        while (!Volatile.Read(ref ready) && spins < 20000) { Thread.Sleep(1); spins++; }
        producer.Join();
        return ready;
    }

    // Allocation inside a compiled method. The array is created here rather
    // than passed in, which is what used to disqualify the method: compiled
    // code reaches no safe point, so a collection during the call would have
    // to walk a frame the collector cannot read. It is sound because
    // allocating does not itself collect — see `crate::alloc` in the JIT — and
    // because a compiled method cannot return a reference, so nothing it
    // allocates outlives the call.

    static int MakeAndSumInts(int n)
    {
        int[] xs = new int[n];
        for (int i = 0; i < n; i++) xs[i] = i;
        int total = 0;
        for (int i = 0; i < n; i++) total += xs[i];
        return total;
    }

    static double MakeAndSumDoubles(int n)
    {
        double[] xs = new double[n];
        for (int i = 0; i < n; i++) xs[i] = i * 0.5;
        double total = 0.0;
        for (int i = 0; i < n; i++) total += xs[i];
        return total;
    }

    // Recursion. Compiled recursion runs on the machine stack rather than the
    // interpreter's frame budget, so the compiled form counts its own depth
    // and refuses past a limit — the same refusal, not a crash.

    static int Factorial(int n)
    {
        if (n <= 1) return 1;
        return n * Factorial(n - 1);
    }

    static int FibRecursive(int n)
    {
        if (n < 2) return n;
        return FibRecursive(n - 1) + FibRecursive(n - 2);
    }

    static void FillDescending(int[] xs, int at, int top)
    {
        if (at > top) return;
        xs[at] = top - at;
        FillDescending(xs, at + 1, top);
    }

    static string RecursiveFill(int n)
    {
        int[] xs = new int[n];
        FillDescending(xs, 0, n - 1);
        string seen = "";
        for (int i = 0; i < n; i++) seen += xs[i];
        return seen;
    }

    static int CountDown(int n)
    {
        if (n <= 0) return 0;
        return 1 + CountDown(n - 1);
    }

    /// A sieve: `bool[]` elements are one byte, read zero-extended.
    static int CountPrimes(int limit)
    {
        bool[] composite = new bool[limit];
        int count = 0;
        for (int p = 2; p < limit; p++)
        {
            if (composite[p]) continue;
            count++;
            for (int multiple = p * p; multiple < limit && multiple > 0; multiple += p)
                composite[multiple] = true;
        }
        return count;
    }

    static string BoolRoundTrip(int n)
    {
        bool[] flags = new bool[n];
        for (int i = 0; i < n; i++) flags[i] = i % 2 == 1;
        string seen = "";
        for (int i = 0; i < n; i++) seen += flags[i] ? "1" : "0";
        return seen;
    }

    static int MakeThenLength(int n)
    {
        double[] xs = new double[n];
        return xs.Length;
    }

    // `double[]`, in the shape the code generator compiles: the array arrives
    // as a parameter, so the tier can resolve it to a descriptor. One created
    // inside the method would be an allocation, which is declined.

    static double ArraySum(double[] xs)
    {
        double total = 0.0;
        for (int i = 0; i < xs.Length; i++) total += xs[i];
        return total;
    }

    static double ArrayDot(double[] a, double[] b)
    {
        double total = 0.0;
        for (int i = 0; i < a.Length; i++) total += a[i] * b[i];
        return total;
    }

    static int ArrayLength(double[] xs) { return xs.Length; }
    static double ArrayAt(double[] xs, int i) { return xs[i]; }
    static void ArrayScale(double[] xs, double k)
    {
        for (int i = 0; i < xs.Length; i++) xs[i] = xs[i] * k;
    }

    // Floating point, in the shapes the code generator compiles: scalar
    // arguments, scalar locals, no allocation.

    static double Add(double a, double b) { return a + b; }
    static double Sub(double a, double b) { return a - b; }
    static double Mul(double a, double b) { return a * b; }
    static double Div(double a, double b) { return a / b; }

    static string FloatMath(double a, double b)
    {
        return Add(a, b) + "/" + Sub(a, b) + "/" + Mul(a, b) + "/" + Div(a, b);
    }

    static int Less(double a, double b) { return a < b ? 1 : 0; }
    static int Greater(double a, double b) { return a > b ? 1 : 0; }
    static int Same(double a, double b) { return a == b ? 1 : 0; }

    static string FloatCompare(double a, double b)
    {
        return Less(a, b) + "/" + Greater(a, b) + "/" + Same(a, b) + "/" + Same(a, a);
    }

    static double Widen(int n) { return n; }
    static double Halve(int n) { return n / 2.0; }
    static int Truncate(double d) { return (int)d; }

    static string FloatConvert(int n)
    {
        return Widen(n) + "/" + Halve(n) + "/" + Truncate(Halve(n));
    }

    /// A loop mixing an integer counter with a double accumulator — the shape
    /// where a wrong kind shows up as a plausible number rather than a crash.
    static double Harmonic(int n)
    {
        double total = 0.0;
        for (int i = 1; i <= n; i++) total += 1.0 / i;
        return total;
    }

    static string FloatLoop(int n) { return Harmonic(n).ToString(); }

    static double Negate(double d) { return -d; }
    static string FloatNegate(double d) { return Negate(d).ToString(); }

    static readonly object timeoutGate = new object();

    /// `Wait` with a timeout nothing pulses. It must come back holding the
    /// monitor: a caller whose `finally` releases a lock it never retook would
    /// corrupt the count for every other thread.
    static string WaitTimesOut()
    {
        bool pulsed;
        lock (timeoutGate) { pulsed = Monitor.Wait(timeoutGate, 150); }
        string first = pulsed ? "pulsed" : "timedout";
        // Taking it again proves the wait left the monitor in a sane state.
        lock (timeoutGate) { return first + "/holds"; }
    }

    static bool WaitPulsedInTime()
    {
        bool pulsed;
        Thread waker = new Thread(() =>
        {
            Thread.Sleep(60);
            lock (timeoutGate) { Monitor.Pulse(timeoutGate); }
        });
        waker.Start();
        lock (timeoutGate) { pulsed = Monitor.Wait(timeoutGate, 10000); }
        waker.Join();
        return pulsed;
    }

    static readonly object queueGate = new object();
    static readonly Queue<int> pending = new Queue<int>();
    static bool producingFinished;
    static int consumedCount;
    static int consumedSum;

    static string ProducerConsumer()
    {
        pending.Clear();
        producingFinished = false;
        consumedCount = 0;
        consumedSum = 0;

        Thread consumer = new Thread(() =>
        {
            while (true)
            {
                int item;
                lock (queueGate)
                {
                    while (pending.Count == 0 && !producingFinished) Monitor.Wait(queueGate);
                    if (pending.Count == 0 && producingFinished) return;
                    item = pending.Dequeue();
                }
                Interlocked.Increment(ref consumedCount);
                Interlocked.Add(ref consumedSum, item);
            }
        });
        consumer.Start();
        // Long enough that the consumer is certainly inside `Wait` with nothing
        // to take: without a real `Pulse` it would stay there.
        Thread.Sleep(120);

        for (int i = 1; i <= 500; i++)
            lock (queueGate) { pending.Enqueue(i); Monitor.Pulse(queueGate); }
        lock (queueGate) { producingFinished = true; Monitor.PulseAll(queueGate); }
        consumer.Join();

        return consumedCount + "/" + consumedSum;
    }

    /// Thirty-two tasks that each block. A pool that could not replace a
    /// blocked worker would run these in `32 / cores` rounds; one that can
    /// finishes in roughly one. The bound is generous — this is checking that
    /// the queue does not stall behind sleeping workers, not a stopwatch.
    static bool BlockingTasksFinish()
    {
        var watch = System.Diagnostics.Stopwatch.StartNew();
        Task[] blocked = new Task[32];
        for (int i = 0; i < blocked.Length; i++)
            blocked[i] = Task.Run(() => Thread.Sleep(200));
        Task.WaitAll(blocked);
        return watch.ElapsedMilliseconds < 1000;
    }

    static int manyTotal;

    static int ManyTasks()
    {
        manyTotal = 0;
        Task[] tasks = new Task[2000];
        for (int i = 0; i < tasks.Length; i++)
            tasks[i] = Task.Run(() => Interlocked.Increment(ref manyTotal));
        Task.WaitAll(tasks);
        return manyTotal;
    }

    /// A task that awaits another. A pool with no way to help would deadlock
    /// here once every worker was busy.
    static int NestedTask()
    {
        return Task.Run(async () =>
        {
            int inner = await Task.Run(() => 40);
            return inner + 2;
        }).GetAwaiter().GetResult();
    }

    /// `Task.Delay` arms a timer and returns; it does not stop its caller.
    static bool DelayOverlaps()
    {
        var watch = System.Diagnostics.Stopwatch.StartNew();
        Task delayed = Task.Delay(300);
        Thread.Sleep(300);
        delayed.GetAwaiter().GetResult();
        return watch.ElapsedMilliseconds < 550;
    }

    static int awaitFirst;
    static int awaitSecond;

    /// The same rendezvous as `TasksOverlap`, reached through `await`.
    ///
    /// If `await` blocked instead of suspending, the first task would have to
    /// finish before the second started — and the first cannot finish, because
    /// it is waiting for the second.
    static async Task<bool> AwaitedOverlap()
    {
        Volatile.Write(ref awaitFirst, 0);
        Volatile.Write(ref awaitSecond, 0);

        Task<bool> a = Task.Run(() =>
        {
            Volatile.Write(ref awaitFirst, 1);
            return WaitForFlag(ref awaitSecond);
        });
        Task<bool> b = Task.Run(() =>
        {
            Volatile.Write(ref awaitSecond, 1);
            return WaitForFlag(ref awaitFirst);
        });
        return await a && await b;
    }

    static async Task<int> AwaitChain()
    {
        int x = await Task.Run(() => 1);
        int y = await Task.Run(() => 2);
        return x + y;
    }

    static int firstArrived;
    static int secondArrived;

    /// Two tasks that each wait for the other.
    ///
    /// A rendezvous rather than a stopwatch: whether two things overlapped is
    /// not a question about how long they took, and a timing margin that is
    /// generous enough to be reliable is too generous to prove anything. Each
    /// task announces itself and then waits for the other, so both can only
    /// finish if both were running. The wait is bounded, so a runtime that
    /// serialises them reports `false` instead of hanging.
    static bool TasksOverlap()
    {
        Volatile.Write(ref firstArrived, 0);
        Volatile.Write(ref secondArrived, 0);

        Task<bool> a = Task.Run(() =>
        {
            Volatile.Write(ref firstArrived, 1);
            return WaitForFlag(ref secondArrived);
        });
        Task<bool> b = Task.Run(() =>
        {
            Volatile.Write(ref secondArrived, 1);
            return WaitForFlag(ref firstArrived);
        });
        Task.WaitAll(a, b);
        return a.Result && b.Result;
    }

    static bool WaitForFlag(ref int flag)
    {
        for (int spins = 0; spins < 20000; spins++)
        {
            if (Volatile.Read(ref flag) == 1) return true;
            Thread.Sleep(1);
        }
        return false;
    }

    static int parallelTotal;

    static int ParallelSum()
    {
        parallelTotal = 0;
        Parallel.For(0, 1000, i => Interlocked.Add(ref parallelTotal, i));
        return parallelTotal;
    }

    static int ParallelForEachSum()
    {
        parallelTotal = 0;
        Parallel.ForEach(new int[] { 1, 2, 3, 4, 5 }, v => Interlocked.Add(ref parallelTotal, v));
        return parallelTotal;
    }

    static int ParallelInvokeCount()
    {
        parallelTotal = 0;
        Parallel.Invoke(
            () => Interlocked.Increment(ref parallelTotal),
            () => Interlocked.Increment(ref parallelTotal),
            () => Interlocked.Increment(ref parallelTotal));
        return parallelTotal;
    }

    static bool ParallelUsesThreads()
    {
        var ids = new HashSet<int>();
        object gate = new object();
        Parallel.For(0, 200, i =>
        {
            lock (gate) { ids.Add(Environment.CurrentManagedThreadId); }
            Thread.Sleep(1);
        });
        return ids.Count > 1;
    }

    static int WaitAllTotal()
    {
        Task<int> a = Task.Run(() => 20);
        Task<int> b = Task.Run(() => 22);
        Task.WaitAll(a, b);
        return a.Result + b.Result;
    }

    static int WaitAllThree()
    {
        Task<int> a = Task.Run(() => 1);
        Task<int> b = Task.Run(() => 2);
        Task<int> c = Task.Run(() => 3);
        Task.WaitAll(a, b, c);
        return a.Result + b.Result + c.Result;
    }

    // Raw pointers. `stackalloc` gives a byte range; `fixed` pins an array.
    // Both are memory this runtime already owns, which is why a pointer can be
    // a buffer plus an offset and never name anything outside one.

    static unsafe string Stackalloc()
    {
        int* p = stackalloc int[3];
        p[0] = 300;
        p[1] = 70000;
        p[2] = -5;
        return p[0] + "," + p[1] + "," + p[2];
    }

    static unsafe long WidePointer()
    {
        long* q = stackalloc long[2];
        q[1] = 9000000000L;
        return q[1];
    }

    static unsafe string FixedOverArray()
    {
        int[] values = new int[] { 1, 2, 3, 4 };
        int total = 0;
        string seen = "";
        fixed (int* start = values)
        {
            for (int* p = start; p < start + values.Length; p++)
            {
                total += *p;
                seen += (seen.Length == 0 ? "" : ",") + *p;
            }
        }
        return total + "/" + seen;
    }

    static unsafe int FixedWrite()
    {
        int[] values = new int[] { 1, 2, 3, 4 };
        fixed (int* start = values) { *(start + 1) = 99; }
        int total = 0;
        foreach (int v in values) total += v;
        return total;
    }

    static unsafe string InitBlock()
    {
        byte* b = stackalloc byte[4];
        for (int i = 0; i < 4; i++) b[i] = 1;
        System.Runtime.CompilerServices.Unsafe.InitBlock(b, 7, 4);
        return b[0] + "," + b[1] + "," + b[2] + "," + b[3];
    }

    static unsafe string CopyBlock()
    {
        byte* from = stackalloc byte[4];
        byte* to = stackalloc byte[4];
        for (int i = 0; i < 4; i++) from[i] = (byte)(i + 1);
        System.Runtime.CompilerServices.Unsafe.CopyBlock(to, from, 4);
        return to[0] + "," + to[1] + "," + to[2] + "," + to[3];
    }

    static async IAsyncEnumerable<int> Counting(int n)
    {
        for (int i = 1; i <= n; i++)
        {
            await Task.Yield();
            yield return i * 10;
        }
    }

    static async Task<int> SumCounting(int n)
    {
        int total = 0;
        await foreach (int v in Counting(n)) total += v;
        return total;
    }

    static async Task<string> FirstTwo()
    {
        string seen = "";
        await foreach (int v in Counting(5))
        {
            seen += v + ";";
            if (seen.Length >= 6) break;
        }
        return seen;
    }

    static async IAsyncEnumerable<int> Empty()
    {
        await Task.Yield();
        yield break;
    }

    static async Task<int> CountEmpty()
    {
        int n = 0;
        await foreach (int v in Empty()) n += v + 1;
        return n;
    }

    static async Task<int> AwaitedValueTask()
    {
        int a = await TripledAsync(4);
        int b = await TripledAsync(6);
        return a + b;
    }

    static async Task<string> CaughtAcrossAwait()
    {
        try { await Throwing(); return "no throw"; }
        catch (InvalidOperationException e) { return e.Message; }
    }

    /// A task completed *after* its awaiter suspended — the only path that
    /// exercises the continuation machinery rather than running straight
    /// through.
    static async Task<int> ResumedByCompletionSource()
    {
        TaskCompletionSource<int> source = new TaskCompletionSource<int>();
        Task<int> waiting = Tripled(source.Task);
        source.SetResult(11);
        return await waiting;
    }

    static async Task<int> Tripled(Task<int> pending)
    {
        int value = await pending;
        return value * 3;
    }

    // -- integer kernels -----------------------------------------------------
    // Milestone 4. These are deliberately leaf methods over integers, which is
    // exactly what the x86-64 backend compiles. Run the fixture with
    // `--jit-threshold 1` and every one of them executes as machine code, so
    // these checks compare the code generator against the interpreter rather
    // than merely against .NET.

    static int Arithmetic(int a, int b)
    {
        return a + b - (a * b) / (b + 1) + a % (b + 3);
    }

    static long Bitwise(long x, int shift)
    {
        long masked = (x & 0xFF00FF00) | (x ^ 0x0F0F0F0F);
        return (masked << shift) ^ (masked >> shift) ^ (long)((ulong)masked >> shift);
    }

    static int Comparisons(int a, int b)
    {
        int score = 0;
        if (a < b) score += 1;
        if (a > b) score += 2;
        if (a == b) score += 4;
        if (a <= b) score += 8;
        if (a >= b) score += 16;
        if (a != b) score += 32;
        return score;
    }

    static int UnsignedComparisons(int a, int b)
    {
        uint x = (uint)a;
        uint y = (uint)b;
        int score = 0;
        if (x < y) score += 1;
        if (x > y) score += 2;
        if (x <= y) score += 4;
        if (x >= y) score += 8;
        return score;
    }

    /// Assigns to its own parameter, which compiles to `starg`.
    static int Collatz(int n)
    {
        int steps = 0;
        while (n != 1)
        {
            if (n % 2 == 0) n = n / 2;
            else n = 3 * n + 1;
            steps++;
        }
        return steps;
    }

    static int Gcd(int a, int b)
    {
        while (b != 0) { int t = b; b = a % b; a = t; }
        return a;
    }

    static long FibIterative(int n)
    {
        long a = 0, b = 1;
        for (int i = 0; i < n; i++) { long t = a + b; a = b; b = t; }
        return a;
    }

    /// Negation, complement and a deep evaluation stack in one place: the
    /// backend keeps two stack slots in registers and spills the rest.
    static int DeepStack(int a, int b, int c)
    {
        return (a + b) * (c - a) + (-b) + (~c) + (a + b + c + a + b + c);
    }

    // Inlining. Without it the backend declines anything containing a `call`,
    // so `Blend` below would be interpreted no matter how hot it got. `Scale`
    // and `Clamp` are branch-free static leaves, which is exactly what the
    // inliner splices, and `Blend` is then compiled as if it had been written
    // out longhand. The answer must not change either way.

    static int Scale(int v, int by) { return v * by + (v >> 1); }

    static int Clamp(int v) { return v & 0xFFFF; }

    static int Blend(int a, int b)
    {
        return Clamp(Scale(a, 3)) + Clamp(Scale(b, 5)) - Clamp(a ^ b);
    }

    /// A loop around an inlined call, so the compiled body is entered many
    /// times rather than once.
    static int BlendLoop(int n)
    {
        int total = 0;
        for (int i = 0; i < n; i++) { total = total + Blend(i, n - i); }
        return total;
    }

    // -- reflection ----------------------------------------------------------
    // Milestone 5. `System.Type` is a real object here, interned one per
    // runtime type, so identity comparisons work as .NET guarantees.

    static string DescribeType(object value)
    {
        Type t = value.GetType();
        return t.Name + "/" + t.BaseType.Name + "/" + t.IsValueType;
    }

    static int InvokeByName(object target, string method, int argument)
    {
        MethodInfo m = target.GetType().GetMethod(method);
        return (int)m.Invoke(target, new object[] { argument });
    }

    static int Fib(int n) { return n < 2 ? n : Fib(n - 1) + Fib(n - 2); }

    static string MessageOf()
    {
        try { throw new InvalidOperationException("written by the program"); }
        catch (Exception e) { return e.Message; }
    }

    static int SumArray(int[] xs)
    {
        int total = 0;
        foreach (int x in xs) total += x;
        return total;
    }

    /// Array access from compiled code. Milestone 4.
    ///
    /// An `int[]` that arrives as a *parameter* is handed to the backend as a
    /// data pointer and a length, so element access compiles to a bounds check
    /// and a scaled-index load rather than a handle lookup. An indexed `for`
    /// rather than `foreach`, because the enumerator's local is not an integer
    /// and the method would be declined.
    static int ArraySum(int[] xs)
    {
        int total = 0;
        for (int i = 0; i < xs.Length; i++) total += xs[i];
        return total;
    }

    /// Stores as well as loads, and a length read.
    static int ArrayReverse(int[] xs)
    {
        int lo = 0;
        int hi = xs.Length - 1;
        while (lo < hi)
        {
            int t = xs[lo];
            xs[lo] = xs[hi];
            xs[hi] = t;
            lo++;
            hi--;
        }
        return xs[0] * 100 + xs[xs.Length - 1];
    }

    /// A bounds failure has to raise the same exception compiled or not.
    static int ArrayAt(int[] xs, int at) { return xs[at]; }

    /// `calli` — an indirect call through a function pointer. Milestone 7.
    ///
    /// A function pointer here names a method rather than an address, which is
    /// what makes an indirect call possible without a code map. It does not
    /// survive being stored somewhere shaped like an integer, and the runtime
    /// says so rather than calling the wrong thing.
    static unsafe int CallIndirect(int seed)
    {
        delegate*<int, int> f = &FnDoubled;
        int a = f(seed);
        f = &FnNegated;
        int b = f(seed);
        delegate*<int, int, int> g = &FnSummed;
        return g(a, b);
    }

    static int FnDoubled(int n) { return n * 2; }
    static int FnNegated(int n) { return -n; }
    static int FnSummed(int a, int b) { return a + b; }

    /// Type arguments of a generic *method* are known at run time.
    ///
    /// Each call site emits a `MethodSpec` carrying the arguments, and the
    /// instantiation records them — so the shared body can still ask what `T`
    /// was. Generic *types* remain erased; see `docs/limitations.md`.
    static string NameOfArg<T>() { return typeof(T).Name; }

    static T DefaultOfArg<T>() { return default(T); }

    static bool ArgHolds<T>(object o) { return o is T; }

    static int TryCatchFinally()
    {
        int state = 0;
        try
        {
            state += 1;
            throw new InvalidOperationException("boom");
        }
        catch (InvalidOperationException)
        {
            state += 10;
        }
        finally
        {
            state += 100;
        }
        return state;
    }

    /// Exception filters: `catch when`, which runs managed code mid-unwind.
    ///
    /// The filter is evaluated *before* the stack below it is discarded, which
    /// is the property that makes `when` different from testing inside the
    /// catch block and rethrowing.
    static int FilterSelects(int code)
    {
        try
        {
            throw new InvalidOperationException("code=" + code);
        }
        catch (InvalidOperationException e) when (e.Message.EndsWith("=7"))
        {
            return 7;
        }
        catch (InvalidOperationException)
        {
            return 0;
        }
    }

    /// A filter that reads a local of the frame it is unwinding.
    static int FilterSeesLocals(int threshold)
    {
        int seen = threshold * 2;
        try
        {
            throw new InvalidOperationException("boom");
        }
        catch (InvalidOperationException) when (seen > 5)
        {
            return seen;
        }
        catch (InvalidOperationException)
        {
            return -1;
        }
    }

    /// Filters run in order, and the first to accept wins.
    static string FilterOrder()
    {
        string log = "";
        try
        {
            try
            {
                throw new InvalidOperationException("x");
            }
            catch (InvalidOperationException) when (Note(ref log, "inner-false", false))
            {
                log += "|inner-ran";
            }
        }
        catch (InvalidOperationException) when (Note(ref log, "outer-true", true))
        {
            log += "|outer-ran";
        }
        return log;
    }

    static bool Note(ref string log, string label, bool verdict)
    {
        log += (log.Length == 0 ? "" : "|") + label;
        return verdict;
    }

    /// A filter that throws declines, and the original exception continues.
    static int FilterThatThrows()
    {
        try
        {
            throw new InvalidOperationException("original");
        }
        catch (InvalidOperationException) when (Boom())
        {
            return -1;
        }
        catch (InvalidOperationException e)
        {
            return e.Message == "original" ? 1 : 0;
        }
    }

    static bool Boom()
    {
        throw new InvalidOperationException("from the filter");
    }

    static int NestedCatch()
    {
        int v = 0;
        try
        {
            try
            {
                int[] a = new int[2];
                v = a[5];
            }
            finally { v += 1; }
        }
        catch (IndexOutOfRangeException) { v += 20; }
        return v;
    }

    static int DivideSafely(int a, int b)
    {
        try { return a / b; }
        catch (DivideByZeroException) { return -1; }
    }

    delegate int BinOp(int a, int b);
    static int Apply(BinOp f, int a, int b) { return f(a, b); }
    static int Mul(int a, int b) { return a * b; }

    /// <summary>
    /// Allocates enough to force at least one collection while objects are
    /// still being constructed.
    ///
    /// This is a regression guard: the runtime once collected at a point where
    /// a freshly allocated instance was not yet reachable from any root, which
    /// freed the object under construction.
    /// </summary>
    static int AllocationPressure(int count)
    {
        int live = 0;
        Node head = null;
        for (int i = 0; i < count; i++)
        {
            Node node = new Node();
            node.Value = i;
            if (i % 100 == 0)
            {
                node.Next = head;
                head = node;
                live++;
            }
        }
        // Walk the surviving chain: every link must still be readable.
        int walked = 0;
        Node cursor = head;
        while (cursor != null)
        {
            walked++;
            cursor = cursor.Next;
        }
        return walked == live ? walked : -1;
    }

    // What a business application asks of the base class library: format
    // strings, dates as ticks, string overloads, enums by name, events,
    // statics of generic types, Nullable boxing, multi-dimensional arrays.
    // Found by diffing Rustzor's BclProbe on both runtimes.
    enum Stage { Draft, Active, Closed }

    sealed class Publisher
    {
        public event EventHandler<string>? Said;
        public void Say(string s) => Said?.Invoke(this, s);
    }

    static class PerType<T> { public static int Length = typeof(T).Name.Length; }

    readonly struct Handle { readonly object? _owner; public Handle(object o) { _owner = o; } public bool IsSet => _owner is not null; }
    [AttributeUsage(AttributeTargets.Property)] sealed class TagPropAttribute : Attribute { public bool Flag { get; set; } }
    class PropBag { [TagProp(Flag = true)] public int Shown { get; set; } [TagProp] private int Hidden { get; set; } public int Plain { get; set; } }
    abstract class Root { private bool _fresh = true; protected int Count; protected Root() { _fresh = false; Count = 1; } public bool Fresh => _fresh; public int Counted => Count; }
    abstract class Middle<T> : Root { public T? Value { get; set; } }
    sealed class Closed : Middle<string> { }
    sealed class Open<T> : Middle<T> { }
    sealed class Injected { public readonly string Name; public readonly int Size; public Injected(string name, int size) { Name = name; Size = size; } }
    abstract class Lifecycle { public string Run() => Init() + "," + Render(); protected virtual string Init() => "base-init"; protected virtual string Render() => "base-render"; }
    abstract class TypedLifecycle<T> : Lifecycle { protected override string Init() => "init:" + typeof(T).Name; }
    sealed class TextLifecycle : TypedLifecycle<string> { protected override string Render() => "render"; }
    class Settings<T> { public T? Current { get; set; } private int Secret { get; set; } }
    sealed class TextSettings : Settings<string> { }
    class WithOptional { public double? Delta { get; set; } public int? Missing { get; set; } }
    class HoldsHandle { private Handle _handle; public bool Ready => _handle.IsSet; public void Attach(object o) => _handle = new Handle(o); }

    static string JoinSequence(IEnumerable<string> items)
    {
        var sb = new System.Text.StringBuilder();
        foreach (var s in items)
        {
            if (sb.Length > 0) sb.Append(',');
            sb.Append(s);
        }
        return sb.ToString();
    }

    static void BusinessLibrary()
    {
        var inv = System.Globalization.CultureInfo.InvariantCulture;
        CheckStr("N2", 1234567.891.ToString("N2", inv), "1,234,567.89");
        CheckStr("F2 rounds half to even", 0.125.ToString("F2", inv), "0.12");
        CheckStr("custom rounds half away", 0.125.ToString("0.00", inv), "0.13");
        CheckStr("D5", 42.ToString("D5"), "00042");
        CheckStr("X", 255.ToString("X"), "FF");
        CheckStr("P1", 0.256.ToString("P1", inv), "25.6 %");
        CheckStr("#,##0", 1234567.5.ToString("#,##0", inv), "1,234,568");
        CheckStr("interpolation specifier", $"{3.14159:F2}|{7,3}", "3.14|  7");
        CheckStr("string.Format specifier", string.Format(inv, "{0:N1}", 12.345), "12.3");
        CheckEq("int.TryParse", int.TryParse("77", out var parsed) ? parsed : -1, 77);
        CheckStr("foreach over array as IEnumerable<T>", JoinSequence(new[] { "x", "y", "z" }), "x,y,z");
        var untyped = new List<object>();
        foreach (var o in (System.Collections.IEnumerable)new[] { 1, 2 }) untyped.Add(o);
        CheckEq("foreach over array as IEnumerable", untyped.Count, 2);
        object boxedList = new List<string> { "a" }, boxedMap = new Dictionary<string, int> { ["k"] = 1 }, boxedArray = new[] { "x" };
        CheckStr("collections satisfy their interfaces",
            $"{boxedList is System.Collections.IEnumerable}{boxedList is IList<string>}{boxedList is IReadOnlyList<string>}{boxedList is System.Collections.IDictionary}"
            + $"{boxedMap is IDictionary<string, int>}{boxedMap is IReadOnlyDictionary<string, int>}{boxedMap is System.Collections.IDictionary}"
            + $"{boxedArray is IEnumerable<string>}{boxedArray is System.Collections.IList}{boxedArray is IReadOnlyList<string>}",
            "TrueTrueTrueFalseTrueTrueTrueTrueTrueTrue");
        var entries = new List<string>();
        if (boxedMap is System.Collections.IDictionary map)
            foreach (System.Collections.DictionaryEntry e in map) entries.Add($"{e.Key}={e.Value}");
        CheckStr("IDictionary yields DictionaryEntry", string.Join(",", entries), "k=1");
        var kinds = new List<string>();
        foreach (var o in (System.Collections.IEnumerable)new List<double> { 1.5 }) kinds.Add(o is double ? "double" : "?");
        foreach (var o in (System.Collections.IEnumerable)new[] { true }) kinds.Add(o is bool ? "bool" : "?");
        var shape = new { Price = 2.5, Count = 3, Ok = true };
        foreach (var p in shape.GetType().GetProperties())
        {
            var v = p.GetValue(shape);
            kinds.Add(v switch { bool => "bool", int => "int", double => "double", _ => "?" });
        }
        CheckStr("values typed as object are boxed", string.Join(",", kinds), "double,bool,double,int,bool");
        var bag = new PropertyBag { ["a"] = 1 };
        bag["b"] = 2;
        var bagKeys = new List<string>();
        if ((object)bag is System.Collections.IDictionary untypedBag)
            foreach (System.Collections.DictionaryEntry e in untypedBag) bagKeys.Add($"{e.Key}{e.Value}");
        foreach (var kv in (IEnumerable<KeyValuePair<string, object?>>)bag) bagKeys.Add(kv.Key);
        CheckStr("a dictionary subclass is a dictionary", string.Join(",", bagKeys), "a1,b2,a,b");
        var registry = new TypedRegistry();
        registry.Put(new Point2(1, 2));
        registry.Put(7);
        registry.Put(new Point2(3, 4));
        CheckStr("generic construction over a method type parameter",
            $"{registry.Get<Point2>(0).X},{registry.Get<Point2>(1).Y},{registry.Get<int>(0)},{registry.Count}", "1,4,7,2");
        var pt = new Point2(5, 6);
        var built = new System.Text.StringBuilder("ab");
        var snapshot = built.ToString();
        for (var n = 0; n < 2000; n++) built.Append(n % 10);
        built.Insert(0, "<").Append('>');
        CheckStr("StringBuilder grows in place", $"{built.Length}|{snapshot}|{built.ToString().Substring(0, 6)}|{built.ToString()[^1]}", "2004|ab|<ab012|>");
        var sbi = new System.Text.StringBuilder();
        var who = "Rustzor";
        sbi.Append($"[{who}:{42,4}]").Append($"{1.5:F2}");
        var cloned = (int[])new[] { 1, 2, 3 }.Clone();
        cloned[0] = 9;
        CheckStr("interpolation and framework gaps",
            $"{sbi}|{string.Create(System.Globalization.CultureInfo.InvariantCulture, $"{2.5:0.0}x")}|{string.Join(",", cloned)}|{Convert.ToByte("ff", 16)}|{1.5.CompareTo(2.0)}|{double.NaN.CompareTo(0.0)}",
            "[Rustzor:  42]1.50|2.5x|9,2,3|255|-1|-1");
        CheckStr("WebUtility", $"{System.Net.WebUtility.HtmlEncode("<a href=\"x\">&</a>")}|{System.Net.WebUtility.UrlEncode("a b&c")}|{System.Net.WebUtility.UrlDecode("a+b%26c")}", "&lt;a href=&quot;x&quot;&gt;&amp;&lt;/a&gt;|a+b%26c|a b&c");
        var asyncBox = new AsyncBox<Point2>();
        CheckStr("typeof(T) inside an async method of a generic class", asyncBox.DescribeAsync().GetAwaiter().GetResult(), "Point2:Point2 { X = 0, Y = 0 }");
        string caught = "";
        try { throw new CodedException(404, "HTTP 404"); } catch (CodedException ex) { caught = $"{ex.Code}:{ex.Message}"; }
        try { throw new NamedOperationException("op", "denied"); } catch (InvalidOperationException ex) { caught += $"|{ex.Message}|{((NamedOperationException)ex).Operation}"; }
        CheckStr("exception subclasses keep their message beside their own fields", caught, "404:HTTP 404|op: denied|op");
        CheckStr("custom exponent formats",
            $"{1.2345e-6.ToString("0.###e+0", inv)}|{123456.0.ToString("0.00E+00", inv)}|{(-0.00042).ToString("0.0e0", inv)}|{0.0.ToString("0e+0", inv)}|{9.9996.ToString("0.00e+0", inv)}",
            "1.235e-6|1.23E+05|-4.2e-4|0e+0|1.00e+1");
        CheckStr("negative values that round to zero keep the sign", $"{(-0.00042).ToString("#,##0.00", inv)}|{(-0.4).ToString("0", inv)}", "-0.00|-0");
        var unixDto = DateTimeOffset.FromUnixTimeMilliseconds(1_700_000_000_123);
        var plusSeven = new DateTimeOffset(2026, 3, 9, 14, 5, 0, TimeSpan.FromHours(7));
        var utcDto = new DateTimeOffset(2026, 3, 9, 7, 5, 0, TimeSpan.Zero);
        CheckStr("DateTimeOffset: unix time, formats and offsets",
            $"{unixDto.ToString("o")}|{unixDto.ToUnixTimeMilliseconds()}|{unixDto.ToUnixTimeSeconds()}|{plusSeven.UtcDateTime.Hour}|{plusSeven == utcDto}|{utcDto:yyyy-MM-dd HH:mm zzz}",
            "2023-11-14T22:13:20.1230000+00:00|1700000000123|1700000000|7|True|2026-03-09 07:05 +00:00");
        var parsedDto = DateTimeOffset.Parse("2026-03-09T14:05:00+07:00");
        var later = utcDto.AddDays(1.5).AddMinutes(30);
        CheckStr("DateTimeOffset: parse, arithmetic and comparison",
            $"{parsedDto.UtcTicks == utcDto.UtcTicks}|{(later - utcDto).TotalHours}|{later > utcDto}|{later.Day}|{utcDto.AddMonths(11).Year}|{DateTimeOffset.TryParse("not a date", out _)}|{(utcDto - TimeSpan.FromHours(8)).Day}",
            "True|36.5|True|10|2027|False|8");
        CheckStr("DateTimeOffset.UtcNow and the static fields", $"{DateTimeOffset.UtcNow >= DateTimeOffset.UnixEpoch}|{DateTimeOffset.UnixEpoch.ToUnixTimeSeconds()}|{DateTime.MinValue.Year}|{TimeSpan.Zero.Ticks}", "True|0|1|0");
        var shapes = typeof(Shapes);
        Type Prop(string name) => shapes.GetProperty(name)!.PropertyType;
        CheckStr("reflection names framework generics and arrays exactly",
            $"{Prop("Tags").GetGenericArguments()[0].Name}|{Prop("Counts").IsArray}|{Prop("Counts").GetElementType()!.Name}|{string.Join(",", Prop("Weights").GetGenericArguments().Select(t => t.Name))}|{Nullable.GetUnderlyingType(Prop("Maybe"))!.Name}|{Nullable.GetUnderlyingType(Prop("Shade")) == null}|{typeof(System.Collections.IList).IsAssignableFrom(Prop("Tags"))}|{Prop("Tags").GetGenericTypeDefinition() == typeof(List<>)}",
            "Tag|True|Int32|String,Double|Int32|True|True|True");
        CheckStr("indexers report their parameters", string.Join(",", shapes.GetProperties().Select(p => p.GetIndexParameters().Length)), "0,0,0,0,0,1");
        var madeList = (System.Collections.IList)Activator.CreateInstance(Prop("Tags"))!;
        madeList.Add(new Tag("a"));
        madeList.Insert(0, new Tag("b"));
        var madeArray = Array.CreateInstance(Prop("Counts").GetElementType()!, 3);
        madeArray.SetValue(7, 1);
        var madeDict = (System.Collections.IDictionary)Activator.CreateInstance(Prop("Weights"))!;
        madeDict["w"] = 2.5;
        var dictEntries = "";
        foreach (System.Collections.DictionaryEntry e in madeDict) dictEntries += e.Value is double d ? $"{e.Key}={d}" : "unboxed";
        CheckStr("values built from a Type through IList, Array and IDictionary",
            $"{madeList.Count}|{madeList[0]}|{((int[])madeArray)[1]}|{madeArray.GetValue(1)}|{madeDict["w"]}|{dictEntries}|{Enum.ToObject(typeof(Shade), 1)}|{Convert.ToInt32(Shade.Dark)}",
            "2|tag:b|7|7|2.5|w=2.5|Dark|1");
        CheckStr("TimeSpan parsing",
            $"{TimeSpan.Parse("1.02:03:04.5").TotalSeconds}|{TimeSpan.Parse("-00:30").TotalMinutes}|{TimeSpan.Parse("3").TotalHours}|{TimeSpan.TryParse("25:00", out _)}|{TimeSpan.Parse(TimeSpan.FromMinutes(90).ToString()).TotalMinutes}",
            "93784.5|-30|72|False|90");
        RegexChecks();
        RandomChecks();
        DecimalChecks();
        EnumChecks();
        ActivatorChecks();
        TextChecks();
        CancellationChecks();
        DateChecks();
        SortChecks();
        HexChecks();
        CheckEq("LINQ callbacks that allocate", LinqUnderCollection(300), 3029);
        var guid = Guid.Parse("{0F8FAD5B-D9CB-469F-A165-70867728950E}");
        CheckStr("Guid formats and parsing",
            $"{guid.ToString("N")}|{guid:B}|{guid == Guid.Parse("0f8fad5bd9cb469fa16570867728950e")}|{Guid.TryParse("nope", out _)}|{Guid.NewGuid().ToString("N").Length}|{guid != Guid.Empty}",
            "0f8fad5bd9cb469fa16570867728950e|{0f8fad5b-d9cb-469f-a165-70867728950e}|True|False|32|True");
        const string superscripts = "⁰¹²³⁴⁵⁶⁷⁸⁹";
        CheckStr("string.Concat over sequences",
            $"{string.Concat("2024".Select(c => superscripts[c - '0']))}|{string.Concat(new List<string> { "a", "b" })}|{string.Concat(Enumerable.Range(1, 3))}|{string.Concat((object)7)}",
            "²⁰²⁴|ab|123|7");
        CheckStr("TickCount64 is available", (Environment.TickCount64 >= 0).ToString(), "True");
        CheckStr("record struct ToString in interpolation", $"{pt}|{(object)pt}", "Point2 { X = 5, Y = 6 }|Point2 { X = 5, Y = 6 }");

        var date = new DateTime(2026, 3, 9, 14, 5, 7);
        CheckStr("date format", date.ToString("yyyy-MM-dd HH:mm", inv), "2026-03-09 14:05");
        CheckStr("date names", date.ToString("dddd, dd MMM", inv), "Monday, 09 Mar");
        CheckStr("AddMonths clamps", new DateTime(2024, 1, 31).AddMonths(1).ToString("yyyy-MM-dd", inv), "2024-02-29");
        CheckEq("date difference", (int)(date.AddDays(3) - date).TotalDays, 3);
        CheckStr("TimeSpan", TimeSpan.FromSeconds(3725).ToString(), "01:02:05");
        CheckEq("DayOfYear", date.DayOfYear, 68);

        CheckEq("IndexOf ignore case", "Hello".IndexOf("LL", StringComparison.OrdinalIgnoreCase), 2);
        CheckStr("PadLeft char", "7".PadLeft(3, '0'), "007");
        CheckStr("Split options", string.Join("|", "a,,b, c".Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)), "a|b|c");
        CheckStr("new string", new string('*', 3) + new string(new[] { 'a', 'b' }), "***ab");

        CheckStr("enum name", Stage.Active.ToString(), "Active");
        CheckStr("enum interpolated", $"{Stage.Closed}", "Closed");
        CheckStr("Enum.Parse", Enum.Parse<Stage>("Closed").ToString(), "Closed");
        CheckStr("Enum.GetValues", string.Join(",", Enum.GetValues<Stage>()), "Draft,Active,Closed");

        var heard = "";
        var pub = new Publisher();
        EventHandler<string> handler = (_, m) => heard += m;
        pub.Said += handler;
        pub.Say("a");
        pub.Said -= handler;
        pub.Say("b");
        CheckStr("event subscribe and unsubscribe", heard, "a");

        var holder = new HoldsHandle();
        var before = holder.Ready;
        holder.Attach("x");
        CheckStr("struct field starts zeroed", before + "|" + holder.Ready, "False|True");
        var publicProps = typeof(PropBag).GetProperties().Select(p => p.Name).OrderBy(n => n);
        CheckStr("GetProperties is public only", string.Join(",", publicProps), "Plain,Shown");
        var marked = typeof(PropBag).GetProperties(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance)
            .Where(p => p.GetCustomAttributes(typeof(TagPropAttribute), true).Length > 0).Select(p => p.Name).OrderBy(n => n);
        CheckStr("property attributes", string.Join(",", marked), "Hidden,Shown");
        CheckEq("Assembly equality", typeof(PropBag).Assembly == typeof(Program).Assembly ? 1 : 0, 1);
        var closed = new Closed { Value = "v" };
        var open = new Open<int> { Value = 4 };
        CheckStr("base class through a construction", closed.Fresh + "|" + closed.Counted + "|" + closed.Value, "False|1|v");
        CheckStr("base class over a type parameter", open.Fresh + "|" + open.Counted + "|" + open.Value, "False|1|4");
        CheckStr("string.Empty", "[" + string.Empty + "]" + string.Empty.Length, "[]0");
        var made = (Injected)typeof(Injected).GetConstructors()[0].Invoke(new object[] { "svc", 3 });
        CheckStr("ConstructorInfo.Invoke", made.Name + made.Size, "svc3");
        CheckStr("Join with a range", string.Join("-", new[] { "a", "b", "c", "d" }, 1, 2), "b-c");
        CheckStr("virtuals through a constructed base", new TextLifecycle().Run(), "init:String,render");
        var declaredOnBase = typeof(TextSettings).BaseType!.GetProperties(BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.Instance | BindingFlags.DeclaredOnly).Select(p => p.Name).OrderBy(n => n);
        CheckStr("properties declared on a constructed base", string.Join(",", declaredOnBase), "Current,Secret");
        CheckStr("base64 round trip", Convert.ToBase64String(new byte[] { 1, 2, 3, 250 }) + "|" + string.Join(",", Convert.FromBase64String("AQID+g==")), "AQID+g==|1,2,3,250");
        CheckEq("BitConverter float bits", BitConverter.SingleToInt32Bits(1.5f), 1069547520);
        CheckStr("BitConverter bytes", BitConverter.ToString(BitConverter.GetBytes(258)) + "|" + BitConverter.ToInt32(new byte[] { 2, 1, 0, 0 }, 0), "02-01-00-00|258");
        var failing = new TaskCompletionSource<int>();
        CheckStr("TrySetException once", failing.TrySetException(new InvalidOperationException("x")) + "|" + failing.TrySetException(new InvalidOperationException("y")) + "|" + failing.Task.IsFaulted, "True|False|True");
        IReadOnlyList<double> asList = new double[] { 1.5, 2.5 };
        IList<int> mutable = new List<int> { 1, 2 };
        mutable[1] = 7;
        CheckStr("collections behind interfaces", asList.Count + "|" + asList[1] + "|" + mutable[1] + "|" + mutable.IndexOf(7) + "|" + ((ICollection<int>)mutable).Contains(1), "2|2.5|7|1|True");
        var optional = new WithOptional();
        typeof(WithOptional).GetProperty("Delta")!.SetValue(optional, (double?)12.5);
        typeof(WithOptional).GetProperty("Missing")!.SetValue(optional, null);
        CheckStr("Nullable property set by reflection", optional.Delta.HasValue + "|" + optional.Delta + "|" + optional.Missing.HasValue, "True|12.5|False");
        CheckEq("generic static constructor", PerType<string>.Length + PerType<int>.Length, 11);
        int? some = 5;
        int? none = null;
        CheckStr("Nullable boxing", ((object?)some)?.ToString() + "|" + ((object?)none == null), "5|True");
        var grid = new int[3, 4];
        grid[1, 2] = 5;
        grid[1, 2] += 2;
        CheckEq("multi-dimensional array", grid[1, 2] + grid.GetLength(1) + grid.Length, 23);
        CheckEq("Task.WhenAll params", Task.WhenAll(Task.FromResult(1), Task.FromResult(2)).Result.Sum(), 3);
        CheckStr("inner exception", new InvalidOperationException("outer", new ArgumentException("inner")).InnerException!.Message, "inner");
    }

    static void Main()
    {
        // arithmetic and control flow
        CheckEq("fib", Fib(15), 610);
        CheckEq("loop", SumArray(new int[] { 1, 2, 3, 4, 5 }), 15);
        CheckEq("modulo", 17 % 5, 2);
        CheckEq("shift", 1 << 10, 1024);
        CheckEq("bitand", 0xF0 & 0x3C, 0x30);
        Check("float", Math.Abs(Math.Sqrt(16.0) - 4.0) < 1e-9);
        CheckEq("intdiv", -7 / 2, -3);
        CheckEq("ternary", 5 > 3 ? 1 : 0, 1);

        // long arithmetic
        long big = 3000000000L;
        Check("long", big * 2 == 6000000000L);

        // strings
        CheckStr("concat", "a" + "b" + "c", "abc");

        // `string + char` is not `String.Concat(string, string)`. Roslyn on
        // .NET 10 lowers it through `ReadOnlySpan<char>`, so an ordinary line
        // of string building fails on a runtime that has no span at all.
        string built = "";
        for (int i = 0; i < 3; i++) built += "xyz"[i];
        CheckStr("string plus char", built, "xyz");
        CheckStr("char plus string", 'a' + "bc", "abc");
        CheckStr("char in a longer chain", "n=" + 'q' + "!" , "n=q!");

        // Number formatting switches to scientific notation outside a band, and
        // Rust's formatter never does. A solver printing a small residual
        // disagreed on every line before this was matched.
        CheckStr("double fixed band", (0.0001).ToString(), "0.0001");
        CheckStr("double small goes scientific", (0.00001).ToString(), "1E-05");
        CheckStr("double tiny keeps digits", (2.9199043183325557E-10).ToString(),
            "2.9199043183325557E-10");
        CheckStr("double large stays fixed", (1e16).ToString(), "10000000000000000");
        CheckStr("double very large goes scientific", (1e17).ToString(), "1E+17");
        CheckStr("double negative exponent signs", (-3.5e-8).ToString(), "-3.5E-08");
        CheckStr("char upper invariant", char.ToUpperInvariant('q').ToString(), "Q");
        CheckStr("split on a char", string.Join("|", "a b c".Split(' ')), "a|b|c");
        CheckStr("upper", "hello".ToUpper(), "HELLO");
        CheckStr("sub", "abcdef".Substring(2, 3), "cde");
        CheckEq("len", "hello".Length, 5);
        CheckEq("indexof", "hello".IndexOf("ll"), 2);
        CheckStr("replace", "a-b-c".Replace("-", "+"), "a+b+c");
        CheckStr("trim", "  pad  ".Trim(), "pad");
        Check("startswith", "prefix".StartsWith("pre"));
        CheckStr("interp", string.Format("{0}-{1}", 1, 2), "1-2");

        // arrays
        int[] nums = new int[5];
        for (int i = 0; i < nums.Length; i++) nums[i] = i * i;
        CheckEq("arraylen", nums.Length, 5);
        CheckEq("arrayval", nums[4], 16);
        string[] words = new string[] { "x", "y" };
        CheckStr("strarray", words[1], "y");

        // objects, inheritance, virtual dispatch
        Shape[] shapes = new Shape[] { new Rect(2, 3), new Circle(1) };
        Check("virtual area", Math.Abs(shapes[0].Area() - 6.0) < 1e-9);
        CheckStr("virtual describe", shapes[0].Describe(), "rect area=6");
        CheckStr("override describe", shapes[1].Describe(), "circle r=1");
        CheckStr("property", shapes[0].Name, "rect");

        // interface dispatch
        IShape s = new Rect(4, 5);
        Check("interface", Math.Abs(s.Area() - 20.0) < 1e-9);

        // casting
        object boxed = 42;
        CheckEq("unbox", (int)boxed, 42);
        Check("isinst", shapes[1] is Circle);
        Check("isinst neg", !(shapes[0] is Circle));

        // structs
        Point p;
        p.X = 3;
        p.Y = 4;
        CheckEq("struct", p.Sum(), 7);

        // enums
        CheckEq("enum", (int)Level.High, 9);

        // exceptions
        CheckEq("try/catch/finally", TryCatchFinally(), 111);

        // `Parallel` runs the body once per item, in order, on this thread.
        // Nothing here overlaps — that limitation is documented — but a loop
        // whose body is order-independent, which is what `Parallel.For`
        // requires, gets the answer it would get anywhere.
        int parallelTotal = 0;
        System.Threading.Tasks.Parallel.For(0, 10,
            i => System.Threading.Interlocked.Add(ref parallelTotal, i));
        CheckEq("Parallel.For", parallelTotal, 45);

        int invoked = 0;
        System.Threading.Tasks.Parallel.Invoke(
            () => System.Threading.Interlocked.Add(ref invoked, 5),
            () => System.Threading.Interlocked.Add(ref invoked, 6));
        CheckEq("Parallel.Invoke", invoked, 11);
        unsafe { CheckEq("calli through a function pointer", CallIndirect(21), 21); }
        CheckEq("nested catch", NestedCatch(), 21);
        CheckEq("filter selects a handler", FilterSelects(7), 7);
        CheckEq("filter declines to the next clause", FilterSelects(3), 0);
        CheckEq("filter reads the unwinding frame's locals", FilterSeesLocals(4), 8);
        CheckStr("filters run in order, first accept wins", FilterOrder(),
            "inner-false|outer-true|outer-ran");
        CheckEq("a throwing filter declines", FilterThatThrows(), 1);
        CheckEq("divzero", DivideSafely(10, 0), -1);
        CheckEq("divok", DivideSafely(10, 2), 5);

        // delegates
        CheckEq("delegate", Apply(Mul, 6, 7), 42);

        // static state
        Check("statics", checks > 0);

        // garbage collection under construction pressure
        CheckEq("allocation pressure", AllocationPressure(40000), 400);

        // generic collections
        List<int> gnums = new List<int>();
        gnums.Add(4);
        gnums.Add(9);
        gnums.Add(1);
        CheckEq("List<T> count", gnums.Count, 3);
        CheckEq("List<T> indexer", gnums[1], 9);
        gnums.Sort();
        CheckEq("List<T> sort", gnums[0], 1);

        // Sorting with a comparator that is itself managed code. The lambda
        // form and the `IComparer<T>` form bind through the same arity key and
        // are told apart by what the object is.
        List<int> bycustom = new List<int>();
        bycustom.Add(5); bycustom.Add(1); bycustom.Add(4); bycustom.Add(2);
        bycustom.Sort((a, b) => b - a);
        CheckStr("List<T>.Sort with a lambda", string.Join(",", bycustom), "5,4,2,1");

        List<string> bylength = new List<string>();
        bylength.Add("pear"); bylength.Add("fig"); bylength.Add("banana");
        bylength.Sort(new ByLength());
        CheckStr("List<T>.Sort with an IComparer", string.Join(",", bylength),
            "fig,pear,banana");

        // A comparison lambda that calls back into the BCL.
        List<string> ordinal = new List<string>();
        ordinal.Add("pear"); ordinal.Add("fig"); ordinal.Add("banana");
        ordinal.Sort((a, b) => string.CompareOrdinal(a, b));
        CheckStr("Sort through CompareOrdinal", string.Join(",", ordinal),
            "banana,fig,pear");
        CheckEq("List<T> foreach", SumSequence(gnums), 14);
        Check("List<T> contains", gnums.Contains(9));
        gnums.Remove(9);
        CheckEq("List<T> remove", gnums.Count, 2);

        List<string> gwords = new List<string>();
        gwords.Add("satu");
        gwords.Add("dua");
        CheckStr("List<T> of strings", JoinList(gwords), "satu;dua;");

        // A collection initialiser, which compiles to repeated Add calls.
        List<int> seeded = new List<int> { 5, 6, 7 };
        CheckEq("collection initialiser", SumSequence(seeded), 18);

        // Interface dispatch onto a natively implemented type: the receiver has
        // no managed GetEnumerator to find.
        IEnumerable<int> asSequence = seeded;
        CheckEq("IEnumerable<T> dispatch", SumSequence(asSequence), 18);

        Dictionary<string, int> ages = new Dictionary<string, int>();
        ages["ana"] = 31;
        ages["budi"] = 24;
        CheckEq("Dictionary count", ages.Count, 2);
        CheckEq("Dictionary lookup", ages["budi"], 24);
        Check("Dictionary containskey", ages.ContainsKey("ana"));
        Check("Dictionary missing key", !ages.ContainsKey("citra"));
        int pairTotal = 0;
        foreach (KeyValuePair<string, int> kv in ages) pairTotal += kv.Value;
        CheckEq("Dictionary foreach", pairTotal, 55);
        // Growth past the initial bucket count, so rehashing is exercised.
        CheckEq("Dictionary growth", DictionaryRoundTrip(), 39800);
        ages.Remove("budi");
        CheckEq("Dictionary remove", ages.Count, 1);

        HashSet<int> gseen = new HashSet<int>();
        Check("HashSet first add", gseen.Add(3));
        Check("HashSet duplicate add", !gseen.Add(3));
        gseen.Add(8);
        CheckEq("HashSet count", gseen.Count, 2);

        Queue<int> gqueue = new Queue<int>();
        gqueue.Enqueue(1);
        gqueue.Enqueue(2);
        CheckEq("Queue dequeue", gqueue.Dequeue(), 1);
        CheckEq("Queue peek", gqueue.Peek(), 2);

        Stack<int> gstack = new Stack<int>();
        gstack.Push(1);
        gstack.Push(2);
        CheckEq("Stack pop", gstack.Pop(), 2);
        CheckEq("Stack count", gstack.Count, 1);

        // Value types inside a generic collection, read back through a
        // pointer into the copy — `ldloca` then `ldflda`.
        List<Point> gpoints = new List<Point>();
        Point gorigin;
        gorigin.X = 2;
        gorigin.Y = 5;
        gpoints.Add(gorigin);
        CheckEq("struct in List<T>", gpoints[0].Sum(), 7);
        CheckStr("address of a struct field", gpoints[0].X.ToString(), "2");

        // A virtual call to a framework method the receiver overrides. The
        // declared target is `System.Object::ToString`, which has no vtable
        // slot here, so dispatch has to find the override by shape.
        CheckStr("ToString override", new Tag("a").ToString(), "tag:a");
        // The same override reached from inside a native method: string
        // concatenation asks the runtime to render the object.
        CheckStr("ToString from native", "" + new Tag("b"), "tag:b");

        // LINQ
        int[] source = new int[] { 1, 2, 3, 4, 5, 6 };
        CheckEq("Where", source.Where(n => n % 2 == 0).Count(), 3);
        CheckEq("Select and Sum", source.Select(n => n * 10).Sum(), 210);
        CheckEq("Any", source.Any(n => n > 5) ? 1 : 0, 1);
        CheckEq("All", source.All(n => n > 5) ? 1 : 0, 0);
        CheckEq("First with predicate", source.First(n => n > 3), 4);
        CheckEq("Skip and Take", source.Skip(2).Take(2).Sum(), 7);
        CheckEq("Aggregate", source.Aggregate(0, (acc, n) => acc + n), 21);

        // A closure, so the lambda carries captured state rather than being a
        // cached static.
        int threshold = 4;
        CheckEq("closure capture", source.Count(n => n > threshold), 2);

        // Ordering: ThenBy must refine the primary order, not replace it.
        string[] names = new string[] { "budi", "ana", "citra", "ana" };
        List<string> ordered = names.OrderBy(n => n.Length).ThenBy(n => n).ToList();
        CheckStr("OrderBy then ThenBy", string.Join(",", ordered.ToArray()), "ana,ana,budi,citra");
        CheckStr(
            "OrderByDescending",
            string.Join(",", names.OrderByDescending(n => n).Distinct().ToArray()),
            "citra,budi,ana");

        // Grouping, including enumerating a group.
        var groups = source.GroupBy(n => n % 3).OrderBy(g => g.Key).ToList();
        CheckEq("GroupBy count", groups.Count, 3);
        int groupTotal = 0;
        foreach (var group in groups)
        {
            foreach (int member in group) groupTotal += member;
        }
        CheckEq("GroupBy members", groupTotal, 21);

        CheckEq("Enumerable.Range", Enumerable.Range(1, 5).Sum(), 15);
        CheckEq("ToDictionary", source.ToDictionary(n => n, n => n * 2)[3], 6);

        // LINQ over a collection rather than an array, and back to one.
        List<int> asList = source.ToList();
        CheckEq("LINQ over List<T>", asList.Where(n => n < 4).Sum(), 6);

        // async and await
        CheckEq("await a result", Doubled(21).GetAwaiter().GetResult(), 42);
        CheckEq("chained awaits", ChainedAwaits(3).GetAwaiter().GetResult(), 18);
        CheckEq("await in a loop", AwaitInLoop(4).GetAwaiter().GetResult(), 20);
        CheckStr(
            "exception across await",
            CaughtAcrossAwait().GetAwaiter().GetResult(),
            "async boom");
        CheckEq("resumed continuation", ResumedByCompletionSource().GetAwaiter().GetResult(), 33);
        CheckEq("Task.Run", Task.Run(() => 6 * 7).GetAwaiter().GetResult(), 42);
        CheckEq("Task.FromResult", Task.FromResult(99).GetAwaiter().GetResult(), 99);
        Check("Task.CompletedTask", Task.CompletedTask.IsCompleted);

        Task<int>[] several = new Task<int>[] { Doubled(1), Doubled(2), Doubled(3) };
        int[] gathered = Task.WhenAll(several).GetAwaiter().GetResult();
        CheckEq("Task.WhenAll", gathered.Sum(), 12);

        // A message on an exception the program constructed itself. The
        // runtime raises its own errors differently, so this is the case that
        // used to come back empty.
        CheckStr("exception message", MessageOf(), "written by the program");

        // integer kernels — the shapes the native code generator takes
        // 17+5 - 85/6 + 17%8  =  22 - 14 + 1
        CheckEq("jit arithmetic", Arithmetic(17, 5), 9);
        // -9+4 - (-36/5) + (-9%7)  =  -5 + 7 - 2; division truncates toward zero
        CheckEq("jit arithmetic negative", Arithmetic(-9, 4), 0);
        CheckEq("jit bitwise", (int)Bitwise(0x1234ABCD, 5), (int)Bitwise(0x1234ABCD, 5));
        Check("jit bitwise is not zero", Bitwise(0x1234ABCD, 5) != 0);
        CheckEq("jit comparisons less", Comparisons(3, 9), 1 + 8 + 32);
        CheckEq("jit comparisons equal", Comparisons(7, 7), 4 + 8 + 16);
        CheckEq("jit comparisons greater", Comparisons(9, 3), 2 + 16 + 32);
        CheckEq("jit unsigned comparisons", UnsignedComparisons(-1, 1), 2 + 8);
        CheckEq("jit starg loop", Collatz(27), 111);
        CheckEq("jit gcd", Gcd(1071, 462), 21);
        // 64-bit result: the 46th Fibonacci number exceeds int.MaxValue.
        Check("jit fib as long", FibIterative(46) == 1836311903L);
        Check("jit fib beyond 32 bits", FibIterative(92) == 7540113804746346429L);
        CheckEq("jit deep stack", DeepStack(3, 4, 5), 14 + (-4) + (-6) + 24);
        CheckEq("jit division truncates toward zero", Arithmetic(-7, 2), -3);
        CheckEq("jit inlined leaf call", Blend(9, 4), 40);

        // Arrays through the backend. These run interpreted on a cold run and
        // compiled under `--jit-threshold 1`; both must agree with .NET.
        int[] cells = new int[6];
        for (int i = 0; i < 6; i++) cells[i] = (i + 1) * 4;
        CheckEq("jit array sum", ArraySum(cells), 84);
        CheckEq("jit array length", cells.Length, 6);
        CheckEq("jit array reverse", ArrayReverse(cells), 2404);
        CheckEq("jit array element after reverse", cells[0], 24);

        bool threwHigh = false;
        try { ArrayAt(cells, 6); } catch (IndexOutOfRangeException) { threwHigh = true; }
        Check("jit array index past the end throws", threwHigh);

        bool threwLow = false;
        try { ArrayAt(cells, -1); } catch (IndexOutOfRangeException) { threwLow = true; }
        Check("jit array negative index throws", threwLow);
        CheckEq("jit inlined call in a loop", BlendLoop(64), 14752);

        // reflection
        // Properties. C# compiles `r.Celsius` to `get_Celsius`, so none of this
        // is needed to run one — it is needed to reflect over one, which means
        // knowing the accessors are halves of a single member.
        var reading = new Reading { Celsius = 20 };
        var celsius = typeof(Reading).GetProperty("Celsius");
        CheckStr("property name", celsius.Name, "Celsius");
        CheckStr("property type", celsius.PropertyType.Name, "Double");
        Check("property can read", celsius.CanRead);
        Check("property can write", celsius.CanWrite);
        Check("computed property is read-only",
            !typeof(Reading).GetProperty("Fahrenheit").CanWrite);
        CheckStr("property declaring type", celsius.DeclaringType.Name, "Reading");

        celsius.SetValue(reading, 100.0);
        CheckEq("property set then read back", (int)reading.Celsius, 100);
        CheckEq("property GetValue", (int)(double)celsius.GetValue(reading), 100);
        CheckEq("computed property GetValue",
            (int)(double)typeof(Reading).GetProperty("Fahrenheit").GetValue(reading), 212);

        var origin = typeof(Reading).GetProperty("Origin");
        CheckStr("static property GetValue", (string)origin.GetValue(null), "lab");

        int propertyCount = 0;
        foreach (var prop in typeof(Reading).GetProperties()) propertyCount++;
        CheckEq("GetProperties counts them", propertyCount, 3);

        CheckStr("missing property is null",
            typeof(Reading).GetProperty("Nope") == null ? "null" : "found", "null");

        // Parameters.
        var scale = typeof(Reading).GetMethod("Scale");
        var parameters = scale.GetParameters();
        CheckEq("parameter count", parameters.Length, 2);
        CheckEq("parameter position", parameters[1].Position, 1);
        CheckStr("parameter type", parameters[0].ParameterType.Name, "Int32");
        CheckStr("parameter name", parameters[0].Name, "by");
        CheckStr("second parameter name", parameters[1].Name, "offset");

        // Assembly and module.
        var asm = typeof(Reading).Assembly;
        CheckStr("type reports its assembly", asm.GetName().Name, "Conformance");
        CheckStr("executing assembly",
            System.Reflection.Assembly.GetExecutingAssembly().GetName().Name, "Conformance");
        Check("assembly declares types", asm.GetTypes().Length > 5);
        CheckStr("assembly finds a type by name",
            asm.GetType("Conformance.Reading").Name, "Reading");
        CheckStr("missing type is null",
            asm.GetType("Conformance.Nope") == null ? "null" : "found", "null");
        CheckStr("module name is the file name", typeof(Reading).Module.Name, "Conformance.dll");
        // Loading an assembly already loaded returns it rather than a second
        // copy — the case both runtimes resolve the same way.
        CheckStr("Assembly.Load of a loaded assembly",
            System.Reflection.Assembly.Load("Conformance").GetName().Name, "Conformance");
        bool refusedMissing = false;
        try { System.Reflection.Assembly.Load("NoSuchAssembly"); }
        catch (Exception) { refusedMissing = true; }
        Check("Assembly.Load refuses a missing assembly", refusedMissing);
        CheckStr("entry assembly",
            System.Reflection.Assembly.GetEntryAssembly().GetName().Name, "Conformance");

        // A user generic *type* knows its argument through the receiver.
        var cellInt = new Cell<int>();
        var cellText = new Cell<string>();
        CheckStr("typeof(T) on a generic type", cellInt.ArgumentName(), "Int32");
        CheckStr("typeof(T) at a second construction", cellText.ArgumentName(), "String");
        Check("o is T on a generic type", cellInt.Accepts(7));
        Check("o is T rejects the other argument", !cellInt.Accepts("seven"));
        CheckEq("default(T) on a generic type", cellInt.Empty(), 0);
        CheckStr("default(T) for a reference argument",
            cellText.Empty() == null ? "null" : "?", "null");

        // Each construction has its own static slot.
        Tally<int>.Count = 3;
        Tally<string>.Count = 7;
        CheckEq("static per construction", Tally<int>.Count, 3);
        CheckEq("the other construction is separate", Tally<string>.Count, 7);

        // Generic methods know their type arguments.
        CheckStr("typeof(T) in a generic method", NameOfArg<int>(), "Int32");
        CheckStr("typeof(T) at a second instantiation", NameOfArg<string>(), "String");
        var registrar = new Registrar();
        CheckStr("typeof(T) in an instance generic method", registrar.NameOf<int>(), "Int32");
        Registrar viaBase = new DerivedRegistrar();
        CheckStr("typeof(T) in an overriding generic method", viaBase.VirtualNameOf<string>(), "derived:String");
        CheckStr("callvirt on a non-virtual method ignores a `new` method", viaBase.Hidden(), "base");
        CheckEq("default(T) for a value type", DefaultOfArg<int>(), 0);
        CheckStr("default(T) for a reference type",
            DefaultOfArg<string>() == null ? "null" : "?", "null");
        Check("x is T matches", ArgHolds<int>(5));
        Check("x is T rejects", !ArgHolds<int>("five"));

        CheckStr("typeof name", typeof(Tag).Name, "Tag");
        CheckStr("typeof full name", typeof(Tag).FullName, "Conformance.Tag");
        Check("typeof identity", typeof(Tag) == typeof(Tag));
        Check("typeof distinguishes", typeof(Tag) != typeof(Circle));
        Check("typeof of a primitive", typeof(int).IsValueType);
        Check("typeof of a class", !typeof(Tag).IsValueType);
        Check("sealed is reported", typeof(Tag).IsSealed);

        CheckStr("GetType on an instance", new Tag("x").GetType().Name, "Tag");
        Check("GetType matches typeof", new Tag("x").GetType() == typeof(Tag));
        CheckStr("base type", typeof(Circle).BaseType.Name, "Shape");
        Check("assignable from derived", typeof(Shape).IsAssignableFrom(typeof(Circle)));
        Check("not assignable to derived", !typeof(Circle).IsAssignableFrom(typeof(Shape)));
        Check("IsInstanceOfType", typeof(Shape).IsInstanceOfType(new Circle(1)));

        // A boxed value reports the type it holds, not System.Object — and
        // calling a method on it must reach the value type's implementation
        // with an unboxed receiver.
        object boxedInt = 42;
        CheckStr("boxed value type", boxedInt.GetType().Name, "Int32");
        CheckStr("ToString through a box", "" + boxedInt, "42");

        CheckStr("struct describes itself", DescribeType(gorigin), "Point/ValueType/True");

        // Members.
        FieldInfo sides = typeof(Node).GetField("Value");
        Check("GetField found it", sides != null);
        Node probe = new Node();
        sides.SetValue(probe, 17);
        CheckEq("FieldInfo round trip", probe.Value, 17);
        CheckEq("FieldInfo GetValue", (int)sides.GetValue(probe), 17);

        CheckEq("MethodInfo invoke", InvokeByName(new Tag("y"), "Echo", 21), 42);
        CheckStr("MethodInfo return type", typeof(Tag).GetMethod("Echo").ReturnType.Name, "Int32");

        // Activator.
        object made = Activator.CreateInstance(typeof(Node));
        Check("Activator produced the type", made.GetType() == typeof(Node));

        // custom attributes
        object[] marks = typeof(Marked).GetCustomAttributes(false);
        CheckEq("attribute count", marks.Length, 1);
        MarkAttribute mark = (MarkAttribute)marks[0];
        CheckStr("attribute constructor argument", mark.Text, "on the type");
        CheckEq("attribute named property", mark.Order, 3);
        Check("attribute named field", mark.Enabled);
        CheckEq(
            "attributes filtered by type",
            typeof(Marked).GetCustomAttributes(typeof(MarkAttribute), false).Length,
            1);
        CheckEq("no attributes is not an error", typeof(Tag).GetCustomAttributes(false).Length, 0);

        object[] fieldMarks = typeof(Marked).GetField("Slot").GetCustomAttributes(false);
        CheckStr("attribute on a field", ((MarkAttribute)fieldMarks[0]).Text, "on a field");
        object[] methodMarks = typeof(Marked).GetMethod("Twice").GetCustomAttributes(false);
        CheckStr("attribute on a method", ((MarkAttribute)methodMarks[0]).Text, "on a method");

        // -- threads that actually run at the same time ------------------------
        //
        // Each of these has an answer that differs if the threads are
        // serialised, so passing them is evidence rather than decoration.

        CheckEq("four threads share one static", ThreadedSum(), 4000);
        CheckEq("lock excludes", LockedCounter(), 8000);
        CheckEq("Interlocked does not lose updates", InterlockedCounter(), 20000);
        Check("a thread started later can unblock an earlier one", Overlaps());

        // -- floating point through the code generator ------------------------
        //
        // These run interpreted *and* compiled, because the fixture is diffed
        // in both modes. IL's `add` is `add` whether its operands are integers
        // or doubles, so a compiler has to work the kind out from the types
        // that reached it — and an integer `add` emitted for a double gives a
        // number, silently, and the wrong one.

        CheckStr("compiled double arithmetic", FloatMath(1.5, 4.0), "5.5/-2.5/6/0.375");
        CheckStr("compiled double comparison", FloatCompare(1.5, 2.5), "1/0/0/1");
        CheckStr("int to double and back", FloatConvert(7), "7/3.5/3");
        CheckStr("a double accumulated in a loop", FloatLoop(10), "2.9289682539682538");
        CheckStr("negating a double", FloatNegate(2.5), "-2.5");

        // -- double[] through the code generator ------------------------------
        //
        // Element access on a `double[]` that arrives as a parameter. Runs
        // interpreted and compiled, and the bounds check has to survive both.

        double[] samples = new double[] { 1.5, 2.5, 3.5, 4.5 };
        double[] weights = new double[] { 2.0, 2.0, 2.0, 2.0 };
        CheckStr("compiled double[] read", ArraySum(samples).ToString(), "12");
        CheckStr("compiled double[] dot product", ArrayDot(samples, weights).ToString(), "24");
        CheckEq("compiled double[] length", ArrayLength(samples), 4);
        ArrayScale(samples, 2.0);
        CheckStr("compiled double[] write", ArraySum(samples).ToString(), "24");
        try { ArrayAt(samples, 9); Check("a compiled double[] is bounds-checked", false); }
        catch (IndexOutOfRangeException) { Check("a compiled double[] is bounds-checked", true); }

        // -- allocation inside compiled code ----------------------------------
        //
        // A method that creates its own array used to be declined outright,
        // because compiled code reaches no safe point. It compiles now, and
        // these run both ways.

        CheckEq("a compiled method makes an int[]", MakeAndSumInts(10), 45);
        CheckStr("a compiled method makes a double[]", MakeAndSumDoubles(10).ToString(), "22.5");
        CheckEq("an array made and passed on", MakeThenLength(6), 6);
        CheckEq("a compiled sieve over bool[]", CountPrimes(100), 25);
        CheckStr("bool[] reads what it wrote", BoolRoundTrip(8), "01010101");

        // -- recursion in compiled code ---------------------------------------
        //
        // Recursion is the one call shape the inliner cannot remove — splicing
        // a method into itself does not terminate — so it needed a calling
        // convention of its own. These run interpreted and compiled.

        CheckEq("a compiled method calls itself", Factorial(10), 3628800);
        CheckEq("recursion that branches", FibRecursive(20), 6765);
        CheckStr("recursion over an array", RecursiveFill(6), "543210");
        CheckEq("recursion deep enough to matter", CountDown(2000), 2000);

        // -- Monitor.Wait and Pulse -------------------------------------------
        //
        // A producer/consumer queue. The consumer is made to reach `Wait` with
        // the queue empty before anything is produced, so the answer depends on
        // it being woken rather than on timing.

        CheckStr("wait and pulse move 500 items", ProducerConsumer(), "500/125250");
        CheckStr("wait times out and keeps the lock", WaitTimesOut(), "timedout/holds");
        Check("wait reports a pulse before its timeout", WaitPulsedInTime());

        // -- the thread pool --------------------------------------------------
        //
        // Two thousand tasks used to mean two thousand OS threads, each paying
        // for a copy of the loader. These run on one worker per core.

        CheckEq("two thousand tasks", ManyTasks(), 2000);
        Check("blocked workers do not stall the queue", BlockingTasksFinish());
        CheckEq("a task awaiting a task", NestedTask(), 42);
        Check("Task.Delay overlaps its caller", DelayOverlaps());

        // -- await suspends rather than blocking ------------------------------
        //
        // The rendezvous again, but reached through `await`: an async method
        // that awaits a pending task must *return* to its caller, or the two
        // tasks below can never both be running.

        Check("awaited tasks overlap", AwaitedOverlap().GetAwaiter().GetResult());
        CheckEq("an async method still returns its value",
            AwaitChain().GetAwaiter().GetResult(), 3);

        // -- Task.Run and Parallel really overlap -----------------------------
        //
        // Timing is the only way to tell these apart from the sequential
        // version, so these measure it. The margins are wide: two 120 ms tasks
        // finish under 200 ms overlapped and over 240 ms serialised.

        Check("two tasks overlap", TasksOverlap());
        CheckEq("Parallel.For visits every index", ParallelSum(), 499500);
        CheckEq("Parallel.ForEach visits every item", ParallelForEachSum(), 15);
        CheckEq("Parallel.Invoke runs every action", ParallelInvokeCount(), 3);
        Check("Parallel.For uses more than one thread", ParallelUsesThreads());

        // -- Task.WaitAll written in C# ---------------------------------------
        //
        // `WaitAll(a, b)` does not call a params array on .NET 10. It fills an
        // `InlineArray2<Task>` and makes a `ReadOnlySpan<Task>` over it, so
        // this exercises the lowering rather than the method.
        //
        // It says nothing about concurrency: both tasks have already run to
        // completion by the time `WaitAll` sees them.

        CheckEq("WaitAll over two tasks", WaitAllTotal(), 42);
        CheckEq("WaitAll over three tasks", WaitAllThree(), 6);

        // -- ValueTask and await using ---------------------------------------

        CheckEq("async ValueTask<T> returns", TripledAsync(7).GetAwaiter().GetResult(), 21);
        NothingAsync().GetAwaiter().GetResult();
        Check("async ValueTask completes", true);

        ValueTask completed = ValueTask.CompletedTask;
        Check("ValueTask.CompletedTask is completed", completed.IsCompleted);
        ValueTask<int> wrapped = new ValueTask<int>(11);
        Check("a wrapped result is completed", wrapped.IsCompleted);
        CheckEq("a wrapped result reads back", wrapped.Result, 11);
        CheckEq("AsTask carries the result", wrapped.AsTask().GetAwaiter().GetResult(), 11);
        CheckEq("await a ValueTask<T>", AwaitedValueTask().GetAwaiter().GetResult(), 30);

        // The ordering is the point: dispose runs after the body, and the
        // `await` inside `DisposeAsync` does not reorder it.
        CheckStr("await using disposes after the body",
            UsedAsync().GetAwaiter().GetResult(), "open;body;closed;");

        // -- marshalling a blittable struct -----------------------------------
        //
        // Turning a struct into bytes and back. Both halves needed the raw
        // pointer above; `SizeOf<T>` additionally needed the generic method to
        // know what `T` is.

        CheckEq("SizeOf a two-int struct", Marshal.SizeOf<Blit>(), 8);
        CheckEq("SizeOf a byte struct", Marshal.SizeOf<Small>(), 2);
        CheckStr("a struct survives a round trip", RoundTrip(), "3/4");
        CheckStr("field widths are respected", WideRoundTrip(), "70000/-5/9000000000");

        // -- raw pointers -----------------------------------------------------
        //
        // A pointer here is a buffer plus a byte offset, not an address, so
        // nothing can be made to point outside memory the runtime owns. What
        // a program can observe is unchanged.

        CheckStr("stackalloc and pointer arithmetic", Stackalloc(), "300,70000,-5");
        CheckStr("a wide value survives a pointer round trip", WidePointer().ToString(), "9000000000");
        CheckStr("fixed over an array", FixedOverArray(), "10/1,2,3,4");
        CheckEq("a pointer write reaches the array", FixedWrite(), 1 + 99 + 3 + 4);
        CheckStr("initblk fills bytes", InitBlock(), "7,7,7,7");
        CheckStr("cpblk copies bytes", CopyBlock(), "1,2,3,4");

        // -- spans over arrays ------------------------------------------------
        //
        // A span is a window onto something. Over an array all three parts of
        // one — the thing, the offset, the length — are representable here.
        // Over stackalloc they are not, and that still refuses.

        int[] backing = new int[] { 1, 2, 3, 4, 5 };
        Span<int> all = backing;
        CheckEq("a span spans its array", all.Length, 5);
        CheckEq("a span reads through", all[0], 1);
        CheckEq("a span reads the last", all[4], 5);

        Span<int> middle = all.Slice(1, 3);
        CheckEq("a slice is shorter", middle.Length, 3);
        CheckEq("a slice is offset", middle[0], 2);
        CheckEq("a slice to the end", all.Slice(3).Length, 2);

        // A span is a window, not a copy: writing through it is visible in the
        // array behind it. That is the whole point of the type.
        middle[0] = 20;
        CheckEq("a write through a span reaches the array", backing[1], 20);

        int[] target = new int[3];
        middle.CopyTo(target);
        CheckEq("CopyTo copies the window", target[0] + target[1] + target[2], 20 + 3 + 4);
        int[] taken = middle.ToArray();
        CheckEq("ToArray copies out", taken.Length, 3);
        taken[0] = 99;
        CheckEq("ToArray really copies", backing[1], 20);

        Check("an empty span is empty", all.Slice(5).IsEmpty);

        // A span over `stackalloc`. The element width is not in the buffer —
        // that is bytes — nor in the type, since framework generics are erased
        // here. It comes from the call site, which spells `int` out.
        CheckStr("a span over stackalloc", StackallocSpan(), "0,1,4,9/13");

        // `Memory<T>` is the same window and is not a ref struct.
        Memory<int> window = backing.AsMemory(2, 2);
        CheckEq("AsMemory takes a range", window.Length, 2);
        Span<int> fromMemory = window.Span;
        CheckEq("a memory hands back its span", fromMemory[0], 3);
        CheckEq("the span is the same window", fromMemory[1], 4);
        CheckEq("AsSpan over the whole array", backing.AsSpan().Length, 5);

        // The collection expression forms that lower through a span.
        int[] spread = [..backing, 6];
        CheckEq("a spread collection expression", spread.Length, 6);
        CheckEq("the spread keeps order", spread[0] + spread[5], 1 + 6);
        ReadOnlySpan<char> letters = ['a', 'b', 'c'];
        CheckEq("a collection expression to a span", letters.Length, 3);

        // -- async iterators and await foreach --------------------------------

        CheckEq("await foreach sums the sequence", SumCounting(4).GetAwaiter().GetResult(), 100);
        CheckEq("a longer sequence", SumCounting(6).GetAwaiter().GetResult(), 210);
        // Breaking out early runs the enumerator's DisposeAsync, which returns
        // `default(ValueTask)` — the case that ended every await foreach in a
        // null reference until the awaiters tolerated it.
        CheckStr("break disposes cleanly", FirstTwo().GetAwaiter().GetResult(), "10;20;");
        CheckEq("an empty async sequence", CountEmpty().GetAwaiter().GetResult(), 0);

        // -- a class type parameter in a static method -----------------------
        //
        // The body is shared by every construction and there is no receiver to
        // ask which one is running. The call site is what knows.

        CheckStr("static method knows T", Tally<int>.ArgumentName(), "Int32");
        CheckStr("a second construction differs", Tally<string>.ArgumentName(), "String");
        Check("static is-check against T", Tally<int>.Accepts(7));
        Check("static is-check rejects", !Tally<int>.Accepts("seven"));
        CheckEq("static default(T)", Tally<int>.Empty(), 0);
        Check("static default(T) for a reference type", Tally<string>.Empty() == null);

        // -- generic types through reflection --------------------------------
        //
        // A closed construction is a real runtime type with its own identity,
        // so the question these ask is whether one named at run time is the
        // same object as one named by `typeof`.

        Check("a construction is generic", typeof(Cell<int>).IsGenericType);
        Check("a construction is not a definition",
            !typeof(Cell<int>).IsGenericTypeDefinition);
        Check("an open definition is a definition",
            typeof(Cell<>).IsGenericTypeDefinition);
        Check("an open definition contains parameters",
            typeof(Cell<>).ContainsGenericParameters);
        Check("a plain type is not generic", !typeof(Node).IsGenericType);

        Type[] cellArgs = typeof(Cell<int>).GetGenericArguments();
        CheckEq("one type argument", cellArgs.Length, 1);
        CheckStr("the type argument is Int32", cellArgs[0].Name, "Int32");
        // `typeof(Cell<>).GetGenericArguments()` is deliberately absent: .NET
        // returns the type parameter `T` and this runtime has no runtime type
        // for one, so it refuses. A check here could not match both.

        Check("definition of a construction",
            typeof(Cell<int>).GetGenericTypeDefinition() == typeof(Cell<>));
        Check("a definition is its own definition",
            typeof(Cell<>).GetGenericTypeDefinition() == typeof(Cell<>));

        // The one that matters: built at run time, and the *same instance* as
        // the one the compiler named. Anything less would make reference
        // equality on types unreliable.
        Type madeInt = typeof(Cell<>).MakeGenericType(typeof(int));
        Check("MakeGenericType equals typeof", madeInt == typeof(Cell<int>));
        CheckStr("the made type reports its name", madeInt.Name, "Cell`1");
        Type madeString = typeof(Cell<>).MakeGenericType(typeof(string));
        Check("different arguments give different types", madeInt != madeString);
        Check("MakeGenericType is stable",
            typeof(Cell<>).MakeGenericType(typeof(int)) == madeInt);

        // An instance of a type built at run time behaves as the compiler's.
        object madeCell = Activator.CreateInstance(madeInt);
        Check("an instance of the made type", madeCell is Cell<int>);
        CheckStr("the made instance runs its methods",
            ((Cell<int>)madeCell).ArgumentName(), "Int32");

        try { typeof(Node).GetGenericTypeDefinition(); Check("non-generic definition throws", false); }
        catch (InvalidOperationException) { Check("non-generic definition throws", true); }
        try { typeof(Cell<int>).MakeGenericType(typeof(int)); Check("closed MakeGenericType throws", false); }
        catch (InvalidOperationException) { Check("closed MakeGenericType throws", true); }
        try { typeof(Cell<>).MakeGenericType(typeof(int), typeof(string)); Check("wrong arity throws", false); }
        catch (ArgumentException) { Check("wrong arity throws", true); }

        // ── Scalar replacement ──────────────────────────────────────────
        //
        // An object that never leaves the method it was made in is replaced by
        // its fields, so `ldfld`/`stfld` become ordinary local access and the
        // method compiles. These checks are about the *answers*: the rewrite
        // is invisible in the output, so each one is written to come out wrong
        // if a field were mapped to the wrong slot, read stale, or lost.
        CheckEq("a non-escaping object counts correctly", CountsInFields(1000), 1000);
        CheckEq("its two fields stay distinct", SplitsInFields(1000), 125 * 1000 + 875);
        CheckEq("fields survive a branch", BranchesOverFields(37), 37 * 2);
        CheckEq("an escaping object still works", EscapesFromFields(50), 50);
        CheckEq("field order is not slot order", ReadsFieldsBackwards(9), 9 * 100 + 9);

        // ── Guarded devirtualisation ────────────────────────────────────
        //
        // A `callvirt` whose every possible answer is known becomes a compare
        // against the receiver's runtime type. The rewrite is invisible in the
        // output, so these are written to come out wrong if a guard matched
        // the wrong type, returned another type's constant, or stopped
        // dispatching on the receiver at all.
        CheckEq("dispatch follows the receiver", SumSides(600), 600 / 2 * 3 + 600 / 2 * 4 * 1000);
        CheckEq("a single-implementation hierarchy still dispatches", OnlyOne(5), 5 * 11);
        CheckEq("dispatch survives a deeper hierarchy", DeepSides(4), 4 * 9);
        Check("a null receiver still throws", NullReceiverThrows());
        CheckEq("a public hierarchy dispatches the same",
            SumOpen(600), 600 / 2 * 7 + 600 / 2 * 8 * 1000);

        // ── Constructors that are skipped, and ones that must not be ────
        //
        // A constructor that provably does nothing is not called at all. These
        // check the boundary from the other side: everything a constructor
        // *does* still has to happen.
        CheckEq("a do-nothing constructor still zeroes fields", new Bare().Value, 0);
        CheckEq("a real constructor still runs", new Assigns(7).Value, 7);
        CheckEq("an inherited real constructor still runs", new EmptyDerived().Value, 42);
        CheckEq("a field initialiser still runs", new WithInitialiser().Value, 5);
        var triggered = new TriggersCctor();
        CheckEq("the class constructor still runs", Witness.Count, 1);
        CheckEq("and the instance is still usable", triggered.Value, 0);

        // ── Field slots across inheritance ──────────────────────────────
        //
        // A field's slot is cached by field rather than by receiver type,
        // which is only sound because the layout is base-first: a field
        // declared in a base sits at the same index in every subclass. These
        // read and write the same base field through a base reference and
        // through two different subclasses, so a cache that returned one
        // subclass's answer for another would show up here.
        CheckEq("a base field through a base reference", SlotThroughBase(), 11);
        CheckEq("the same field through two subclasses", SlotThroughSubclasses(), 2031);
        CheckEq("a subclass field does not collide with the base's",
            SlotsDoNotCollide(), 700 + 8);

        // ── Safe points in compiled code ────────────────────────────────
        //
        // A compiled method polls for a collection at every loop back-edge, so
        // a loop that allocates is no longer unbounded. These allocate far more
        // than the heap holds at once, which means a collection *must* happen
        // partway through a compiled body — the case that could not occur
        // before and the one the whole mechanism exists for.
        //
        // Each is written so a collection at the wrong moment shows up as a
        // wrong answer rather than as nothing: the arrays are written, read
        // back and summed after the allocation that could have swept them.
        CheckEq("a compiled loop allocates under collection pressure",
            AllocatesInALoop(4000), 4000 * 6);
        CheckEq("an array survives the allocation that follows it",
            SurvivesTheNextAllocation(3000), 3000 * 3);
        CheckEq("an array carried across iterations survives every collection",
            CarriesAnArrayAcrossIterations(4000), 4000 * 7);
        CheckEq("an array held across a call to another compiled method survives",
            HoldsAcrossACall(2000), 4 * (2000 + 5));

        // ── Compiled field access on a heap object ──────────────────────
        //
        // Compiled code reaches a field through a helper rather than by
        // knowing the heap's layout. These check every kind that can cross
        // that boundary, and the inherited case where a wrong slot would be
        // easiest to miss.
        CheckEq("compiled int field", FieldRoundTrip(41), 41);
        CheckEq("compiled long field", (int)LongFieldRoundTrip(5), 5 * 1000);
        Check("compiled double field", DoubleFieldRoundTrip(2.5) == 2.5 * 4);
        CheckEq("compiled reference field", ReferenceFieldChain(300), 300);
        CheckEq("an inherited field is not the subclass's own",
            InheritedFieldSlots(), 10203);
        Check("a null receiver throws from compiled code", NullFieldThrows());

        // ── Inlining through more than one level ────────────────────────
        //
        // A helper built out of smaller helpers is how arithmetic is actually
        // factored, and a one-level inliner leaves its caller interpreted. The
        // answers must not depend on how deep the splicing went, so these are
        // ordinary arithmetic checked against what the same source computes on
        // .NET — with operators chosen so a mis-numbered spliced local shows up
        // rather than cancelling out.
        CheckEq("a two-level call chain", TwoLevels(9), 9 * 3 + (9 >> 1) + 9 * 5 + 4);
        CheckEq("a three-level call chain", ThreeLevels(6), (6 * 3 + 3) * 2 - 6);
        CheckEq("a helper used twice in one expression", UsedTwice(7), 7 * 3 + 7 * 5);

        // ── Calls between compiled methods ──────────────────────────────
        //
        // A compiled method can call another one, indirectly through a table
        // of entry addresses the runtime fills in. These cover the shapes that
        // convention has to get right: a callee with a different argument
        // count than its caller, one that returns nothing, mutual recursion —
        // where neither address exists while the other is compiled — and a
        // callee that faults, whose failure has to reach the caller.
        CheckEq("a compiled call with a wider callee", CallsWider(4), 4 + 10 + 200);
        CheckEq("a compiled call returning nothing", CallsVoid(6), 6 * 2);
        CheckEq("mutual recursion between compiled methods", IsEvenLength(9), 1);
        CheckEq("mutual recursion, the other parity", IsEvenLength(8), 0);
        Check("a fault inside a compiled callee reaches the caller", CalleeFaultPropagates());

        // ── Runtime diagnostics a program can read about itself ─────────────
        //
        // Every assertion here is an *invariant*, never an absolute: how many
        // bytes a runtime allocates for the same program is its own business,
        // and a check that pinned a number would fail on .NET or on RustCLR
        // for a reason that is not a bug. What both must agree on is that the
        // counters are cumulative, that collecting is visible, and that the
        // numbers move when the program allocates.
        Check("GC.GetTotalAllocatedBytes never decreases", AllocatedBytesRise());
        Check("GC.GetTotalAllocatedBytes counts new allocations", AllocationIsCounted());
        Check("GC.CollectionCount is non-negative", GC.CollectionCount(0) >= 0);
        Check("GC.Collect() is visible in CollectionCount", CollectIsCounted());
        Check("GC.CollectionCount never decreases", CollectionCountRises());
        Check("allocated bytes exceed live bytes after garbage", GarbageIsAllocatedThenDropped());
        Check("GC.GetTotalMemory reports something live", GC.GetTotalMemory(false) > 0);

        // ── System.Text.Encoding ────────────────────────────────────────────
        //
        // UTF-8 and ASCII, which is what a wire format is. Every assertion is
        // one both runtimes must agree on exactly, because this file is diffed
        // against `dotnet` byte for byte.
        CheckEq("UTF-8 encodes ASCII one byte per character",
            System.Text.Encoding.UTF8.GetBytes("abc").Length, 3);
        CheckEq("UTF-8 encodes a two-byte character as two bytes",
            System.Text.Encoding.UTF8.GetBytes("é").Length, 2);
        CheckEq("UTF-8 encodes an emoji as four bytes",
            System.Text.Encoding.UTF8.GetBytes("😀").Length, 4);
        Check("UTF-8 round-trips a non-ASCII string",
            System.Text.Encoding.UTF8.GetString(
                System.Text.Encoding.UTF8.GetBytes("halo dunia éü")) == "halo dunia éü");
        CheckEq("GetByteCount agrees with GetBytes",
            System.Text.Encoding.UTF8.GetByteCount("café"),
            System.Text.Encoding.UTF8.GetBytes("café").Length);

        // ASCII substitutes rather than throwing, and the substitute is `?`.
        CheckEq("ASCII is one byte per character whatever the character",
            System.Text.Encoding.ASCII.GetBytes("café").Length, 4);
        Check("ASCII substitutes a question mark for what it cannot encode",
            System.Text.Encoding.ASCII.GetString(
                System.Text.Encoding.ASCII.GetBytes("café")) == "caf?");

        // The two encodings are different objects, not one wearing two names.
        Check("UTF8 and ASCII are distinct encodings",
            System.Text.Encoding.UTF8.CodePage != System.Text.Encoding.ASCII.CodePage);
        CheckEq("UTF-8 reports its usual code page",
            System.Text.Encoding.UTF8.CodePage, 65001);

        BusinessLibrary();
        Console.WriteLine("checks=" + checks + " failures=" + failures);
    }

    // ── Fixtures for runtime diagnostics ────────────────────────────────────

    private static bool AllocatedBytesRise()
    {
        long first = GC.GetTotalAllocatedBytes(false);
        long second = GC.GetTotalAllocatedBytes(false);
        return second >= first;
    }

    /// Allocating has to move the counter. Without this the check above passes
    /// against a binding that returns a constant.
    private static bool AllocationIsCounted()
    {
        long before = GC.GetTotalAllocatedBytes(false);
        long sink = 0;
        for (int i = 0; i < 2000; i++)
        {
            int[] block = new int[64];
            block[0] = i;
            sink += block[0];
        }
        long after = GC.GetTotalAllocatedBytes(false);
        return after > before && sink > 0;
    }

    private static bool CollectIsCounted()
    {
        int before = GC.CollectionCount(0);
        GC.Collect();
        return GC.CollectionCount(0) > before;
    }

    private static bool CollectionCountRises()
    {
        int first = GC.CollectionCount(0);
        int second = GC.CollectionCount(0);
        return second >= first;
    }

    /// Garbage that is dropped: the cumulative total has to outgrow what is
    /// live, or the runtime is reporting live bytes under an allocated name.
    private static bool GarbageIsAllocatedThenDropped()
    {
        for (int i = 0; i < 4000; i++)
        {
            string temporary = "garbage" + i;
            if (temporary.Length == 0) return false;
        }
        GC.Collect();
        return GC.GetTotalAllocatedBytes(false) > GC.GetTotalMemory(false);
    }

    // ── Fixtures for calls between compiled methods ─────────────────────────

    /// Three arguments where the caller has one: the callee's trap and context
    /// slots sit past *its* count, so a caller that laid them out at its own
    /// would have the callee read its trap slot out of an argument.
    static int Widen(int a, int b, int c) { return a + b * 10 + c * 100; }

    static int CallsWider(int n)
    {
        int total = 0;
        for (int i = 0; i < 1; i++) total += Widen(n, 1, 2);
        return total;
    }

    /// A void callee leaves nothing where a caller might expect a result.
    ///
    /// The result comes back through an array rather than a static field: a
    /// static would pull in `ldsfld`, which this backend does not compile, and
    /// the check would then be measuring the interpreter.
    static void WriteDouble(int[] into, int v) { into[0] = v * 2; }

    static int CallsVoid(int n)
    {
        int[] cell = new int[1];
        for (int i = 0; i < 1; i++) WriteDouble(cell, n);
        return cell[0];
    }

    /// Neither address exists while the other is being compiled, so the table
    /// each one calls through can only be filled once both are done.
    static int IsEvenLength(int n) { return n <= 0 ? 0 : IsOddLength(n - 1); }
    static int IsOddLength(int n) { return n <= 0 ? 1 : IsEvenLength(n - 1); }

    /// The callee reads past the end of its array. The exception has to come
    /// out of the *caller*, which is where the interpreter has a frame.
    static int ReadsOutOfRange(int[] a, int i)
    {
        int total = 0;
        for (int k = 0; k < 1; k++) total += a[i];
        return total;
    }

    static int CallsOutOfRange(int[] a)
    {
        int total = 0;
        for (int k = 0; k < 1; k++) total += ReadsOutOfRange(a, 99);
        return total;
    }

    static bool CalleeFaultPropagates()
    {
        int[] small = new int[2];
        try { CallsOutOfRange(small); return false; }
        catch (IndexOutOfRangeException) { return true; }
    }

    // ── Fixtures for multi-level inlining ───────────────────────────────────

    static int Triple(int v) { return v * 3 + (v >> 1); }
    static int Quintuple(int v) { return v * 5; }
    static int Add4(int v) { return v + 4; }

    /// `Triple` and `Quintuple` are leaves; this needs one level.
    static int TwoLevels(int v) { return Triple(v) + Add4(Quintuple(v)); }

    static int TripleThenAdd(int v) { return Add3(Triple2(v)); }
    static int Triple2(int v) { return v * 3; }
    static int Add3(int v) { return v + 3; }

    /// `TripleThenAdd` is itself built from two leaves, so splicing it needs a
    /// second level — the case a one-level inliner cannot reach.
    static int ThreeLevels(int v) { return TripleThenAdd(v) * 2 - v; }

    /// The same helper spliced at two sites in one expression. Each splice
    /// gets its own locals; sharing them would make the second overwrite the
    /// first.
    static int UsedTwice(int v) { return Triple2(v) + Quintuple(v); }

    // ── Fixtures for compiled field access ──────────────────────────────────

    sealed class Cell2 { public int Number; public long Big; public double Real; }
    sealed class Link { public int Value; public Link Next; }

    static int FieldRoundTrip(int n)
    {
        Cell2 c = new Cell2();
        int total = 0;
        for (int i = 0; i < 1; i++) { c.Number = n; total = c.Number; }
        return total;
    }

    static long LongFieldRoundTrip(int n)
    {
        Cell2 c = new Cell2();
        long total = 0;
        for (int i = 0; i < 1; i++) { c.Big = n * 1000L; total = c.Big; }
        return total;
    }

    static double DoubleFieldRoundTrip(double n)
    {
        Cell2 c = new Cell2();
        double total = 0;
        for (int i = 0; i < 1; i++) { c.Real = n * 4; total = c.Real; }
        return total;
    }

    /// Builds a chain through a *reference* field and walks it back. A
    /// reference crossing into compiled code as a handle is the case that
    /// would be wrong if the conversion treated it as a number.
    static int ReferenceFieldChain(int n)
    {
        Link head = null;
        for (int i = 0; i < n; i++)
        {
            Link node = new Link();
            node.Value = 1;
            node.Next = head;
            head = node;
        }
        int total = 0;
        Link walk = head;
        while (walk != null) { total += walk.Value; walk = walk.Next; }
        return total;
    }

    class SlotBaseTwo { public int First; public int Second; }
    sealed class SlotChild : SlotBaseTwo { public int Third; }

    /// Three fields across two types, each given a different decimal place, so
    /// any two of them swapping shows up in the total.
    static int InheritedFieldSlots()
    {
        SlotChild c = new SlotChild();
        for (int i = 0; i < 1; i++) { c.First = 1; c.Second = 2; c.Third = 3; }
        return c.First * 10000 + c.Second * 100 + c.Third;
    }

    static int ReadThroughNull(Link link)
    {
        int total = 0;
        for (int i = 0; i < 1; i++) total += link.Value;
        return total;
    }

    static bool NullFieldThrows()
    {
        try { ReadThroughNull(null); return false; }
        catch (NullReferenceException) { return true; }
    }

    // ── Fixtures for compiled safe points ───────────────────────────────────

    /// Allocates one array per iteration and uses it. The total is only right
    /// if every array survived long enough to be read.
    static int AllocatesInALoop(int n)
    {
        int total = 0;
        for (int i = 0; i < n; i++)
        {
            int[] scratch = new int[8];
            scratch[0] = 1;
            scratch[3] = 2;
            scratch[7] = 3;
            total += scratch[0] + scratch[3] + scratch[7];
        }
        return total;
    }

    /// Holds an array *across* the loop back-edge while allocating hard inside
    /// it.
    ///
    /// This is the case the frame map exists for, and the two above are not:
    /// there, each array is allocated and finished with inside one iteration,
    /// so nothing is live at the safe point and a collection there can take
    /// everything. Here `carried` is live at every safe point, and a collector
    /// that could not see the compiled frame would reclaim it and the sum
    /// would be wrong.
    static int CarriesAnArrayAcrossIterations(int n)
    {
        int[] carried = new int[4];
        carried[0] = 7;
        int total = 0;
        for (int i = 0; i < n; i++)
        {
            int[] churn = new int[32];
            churn[0] = i;
            total += carried[0];
        }
        return total;
    }

    /// Allocates hard enough to force collections, and nothing else.
    static int Churns(int n)
    {
        int total = 0;
        for (int i = 0; i < n; i++)
        {
            int[] scratch = new int[32];
            scratch[0] = 1;
            total += scratch[0];
        }
        return total;
    }

    /// Holds an array while calling a *different* compiled method that
    /// collects.
    ///
    /// This is the case the frame chain exists for, and no other check reaches
    /// it: the collection happens inside `Churns`, whose own frame knows
    /// nothing about `held`. If a safe point described only the frame that
    /// polled, `held` would be swept while this frame still read it.
    static int HoldsAcrossACall(int n)
    {
        int[] held = new int[4];
        held[0] = 5;
        int total = 0;
        for (int i = 0; i < 4; i++)
        {
            total += Churns(n);
            total += held[0];
        }
        return total;
    }

    /// Writes an array, allocates *again* — which is where a collection is
    /// most likely to land — then reads the first array back. A collector that
    /// could not see the compiled frame would have reclaimed it by now.
    static int SurvivesTheNextAllocation(int n)
    {
        int total = 0;
        for (int i = 0; i < n; i++)
        {
            int[] first = new int[16];
            first[5] = 3;
            int[] second = new int[16];
            second[5] = 99;
            total += first[5];
        }
        return total;
    }

    // ── Fixtures for field-slot caching ─────────────────────────────────────

    class SlotBase { public int First; }
    sealed class SlotLeftChild : SlotBase { public int Left; }
    sealed class SlotRightChild : SlotBase { public int Right; }

    static int SlotThroughBase()
    {
        SlotBase b = new SlotLeftChild();
        b.First = 11;
        return b.First;
    }

    /// The same inherited field written through two *different* subclasses.
    /// Both must land in slot 0 of their own layout.
    static int SlotThroughSubclasses()
    {
        SlotLeftChild l = new SlotLeftChild();
        SlotRightChild r = new SlotRightChild();
        l.First = 20;
        r.First = 31;
        return l.First * 100 + r.First;
    }

    /// The subclass's own field sits after the inherited one. Writing one must
    /// not disturb the other, in either order.
    static int SlotsDoNotCollide()
    {
        SlotLeftChild l = new SlotLeftChild();
        l.Left = 7;
        l.First = 700;
        l.Left = l.Left + 1;
        return l.First + l.Left;
    }

    // ── Fixtures for constructor elision ────────────────────────────────────

    /// The shape that gets skipped: Roslyn writes a constructor that loads
    /// `this`, calls `object`'s and returns. Its fields must still be zero.
    sealed class Bare { public int Value; }

    /// A constructor that does something must still be called.
    sealed class Assigns { public int Value; public Assigns(int v) { Value = v; } }

    /// The derived constructor is empty, but the *base* one is not — and the
    /// chain is what decides, not the leaf. Skipping this would leave `Value`
    /// at zero.
    class HasRealCtor { public int Value; public HasRealCtor() { Value = 42; } }
    sealed class EmptyDerived : HasRealCtor { }

    /// A field initialiser compiles *into* the constructor, so this is a
    /// constructor that does work even though the source shows no body.
    sealed class WithInitialiser { public int Value = 5; }

    /// The class constructor runs on first use whether or not the instance
    /// constructor is called, so eliding one must not elide the other.
    ///
    /// The witness lives on a *different* class on purpose. Reading a static
    /// field of `TriggersCctor` would itself run its class constructor, so a
    /// check that read one would pass whether or not `new` had triggered it —
    /// which is exactly what the first version of this check did.
    static class Witness { public static int Count; }

    sealed class TriggersCctor
    {
        public int Value;
        static TriggersCctor() { Witness.Count = Witness.Count + 1; }
    }

    // ── Fixtures for guarded devirtualisation ───────────────────────────────
    //
    // `PublicBase` is deliberately *public* and otherwise an ideal candidate:
    // two sealed subclasses, each override a single constant. It exists to be
    // turned down. Another assembly can derive from a public type, so the set
    // of overrides visible here is not the set that will exist at run time,
    // and a guard chain built from it could meet a receiver it cannot name.

    private abstract class Poly { public abstract int Tag(); }
    private sealed class PolyA : Poly { public override int Tag() => 3; }
    private sealed class PolyB : Poly { public override int Tag() => 4; }

    /// Sums through a *public* hierarchy. Identical in shape to `SumSides`,
    /// and it must produce the same answers while being compiled differently —
    /// the call stays a real virtual call.
    static int SumOpen(int n)
    {
        PublicBase[] all = new PublicBase[2];
        all[0] = new PublicA();
        all[1] = new PublicB();
        int total = 0;
        for (int i = 0; i < n; i++)
        {
            int weight = ((i & 1) == 0) ? 1 : 1000;
            total += all[i & 1].Tag() * weight;
        }
        return total;
    }

    /// Two implementations, alternating.
    ///
    /// The weight is what makes this a test. Summing the tags plainly gives
    /// 300*3 + 300*4, and swapping the two guards gives 300*4 + 300*3 — the
    /// same number, so a symmetric version passes whether dispatch works or
    /// not. Weighting the odd half by 1000 breaks that symmetry.
    static int SumSides(int n)
    {
        Poly[] all = new Poly[2];
        all[0] = new PolyA();
        all[1] = new PolyB();
        int total = 0;
        for (int i = 0; i < n; i++)
        {
            int weight = ((i & 1) == 0) ? 1 : 1000;
            total += all[i & 1].Tag() * weight;
        }
        return total;
    }

    private abstract class Single { public abstract int Value(); }
    private sealed class OnlySub : Single { public override int Value() => 11; }

    /// One implementation. The guard is still a test, not an assumption —
    /// which is what makes a null receiver throw rather than silently
    /// answering 11.
    ///
    /// The receiver is held in an array rather than a plain local so that this
    /// actually reaches compiled code: a reference in a local is a shape the
    /// backend still declines, and a check that only ever runs interpreted
    /// would be testing the interpreter twice.
    static int OnlyOne(int n)
    {
        Single[] s = new Single[1];
        s[0] = new OnlySub();
        int total = 0;
        for (int i = 0; i < n; i++) total += s[0].Value();
        return total;
    }

    private abstract class Deep { public abstract int Depth(); }
    private abstract class Middle : Deep { }
    private sealed class Leaf : Middle { public override int Depth() => 9; }

    /// The override is two levels below the method's declaring type, and
    /// `Middle` is abstract so it contributes no case of its own.
    static int DeepSides(int n)
    {
        Deep[] d = new Deep[1];
        d[0] = new Leaf();
        int total = 0;
        for (int i = 0; i < n; i++) total += d[0].Depth();
        return total;
    }

    /// A devirtualised site must not turn a null receiver into an answer.
    ///
    /// Split in two deliberately. The throwing method has no `try`, so the
    /// backend compiles it and the null actually goes through the guard chain;
    /// the `catch` lives in this caller, which stays interpreted. Putting both
    /// in one method would leave the whole thing interpreted and the compiled
    /// path untested — which is what the first version of this check did.
    /// The loop is not decoration: a method whose body loops is compiled on
    /// its first call, and one that does not has to be called 32 times first.
    /// Without it this runs interpreted and proves nothing about the guard.
    static int TagOfNull()
    {
        Poly[] all = new Poly[1];
        int total = 0;
        for (int i = 0; i < 1; i++) total += all[i].Tag();
        return total;
    }

    static bool NullReceiverThrows()
    {
        try { TagOfNull(); return false; }
        catch (NullReferenceException) { return true; }
    }

    // ── Fixtures for scalar replacement ─────────────────────────────────────

    sealed class Pair { public int First; public int Second; }

    /// The plain shape: one object, one field, a loop.
    static int CountsInFields(int n)
    {
        Pair p = new Pair();
        for (int i = 0; i < n; i++) p.First++;
        return p.First;
    }

    /// Two fields updated on alternating paths. Mapping both to one local, or
    /// swapping the two, changes the answer.
    static int SplitsInFields(int n)
    {
        Pair p = new Pair();
        for (int i = 0; i < n; i++)
        {
            if ((i & 7) == 0) p.First++;
            else p.Second++;
        }
        return p.First * 1000 + p.Second;
    }

    /// A field read before a branch and written after it. A rewrite that let
    /// the value go stale across the jump would return the wrong total.
    static int BranchesOverFields(int n)
    {
        Pair p = new Pair();
        p.First = n;
        if (p.First > 0) p.Second = p.First;
        else p.Second = 0;
        return p.First + p.Second;
    }

    /// The object is handed to another method, so it escapes and must *not* be
    /// scalarised. If the analysis replaced it anyway, the callee would see a
    /// different object than the one written here.
    static int EscapesFromFields(int n)
    {
        Pair p = new Pair();
        p.First = n;
        return ReadFirst(p);
    }

    static int ReadFirst(Pair p) => p.First;

    /// The fields are written in declaration order and read in reverse. The
    /// two values are deliberately far apart in magnitude, so a slot-to-local
    /// mapping that swapped them would return 9 * 100 + 900 = 1800, not 909.
    static int ReadsFieldsBackwards(int n)
    {
        Pair p = new Pair();
        p.First = n;
        p.Second = n * 100;
        return p.Second + p.First;
    }

public abstract class PublicBase { public abstract int Tag(); }
public sealed class PublicA : PublicBase { public override int Tag() => 7; }
public sealed class PublicB : PublicBase { public override int Tag() => 8; }
}
