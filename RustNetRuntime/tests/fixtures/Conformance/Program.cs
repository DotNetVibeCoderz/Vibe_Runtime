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

sealed class Node
{
    public int Value;
    public Node Next;
}

static class Program
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

        // -- the thread pool --------------------------------------------------
        //
        // Two thousand tasks used to mean two thousand OS threads, each paying
        // for a copy of the loader. These run on one worker per core.

        CheckEq("two thousand tasks", ManyTasks(), 2000);
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

        Console.WriteLine("checks=" + checks + " failures=" + failures);
    }
}
