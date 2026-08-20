using System;
using System.Collections.Generic;
using System.Linq;
using System.Threading.Tasks;

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

/// A type that genuinely overrides ToString, so dispatch to it can be checked.
sealed class Tag
{
    private readonly string text;
    public Tag(string text) { this.text = text; }
    public override string ToString() { return "tag:" + text; }
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

    static async Task Throwing()
    {
        await Task.Yield();
        throw new InvalidOperationException("async boom");
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
        CheckEq("nested catch", NestedCatch(), 21);
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

        Console.WriteLine("checks=" + checks + " failures=" + failures);
    }
}
