using System;

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

    static int Fib(int n) { return n < 2 ? n : Fib(n - 1) + Fib(n - 2); }

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

        Console.WriteLine("checks=" + checks + " failures=" + failures);
    }
}
