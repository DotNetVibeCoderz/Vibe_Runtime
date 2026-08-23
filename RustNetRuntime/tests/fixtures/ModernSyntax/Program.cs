using System;

namespace ModernSyntax;

// ── C# 9: records ────────────────────────────────────────────────────────────
public record struct Point(int X, int Y);

// ── C# 8: switch expressions and property patterns ──────────────────────────
public enum Shape { Circle, Square, Triangle }

// ── C# 9: init-only properties ──────────────────────────────────────────────
public sealed class Reading
{
    public string Sensor { get; init; } = "";
    public double Value { get; init; }
}

// ── C# 8: static local functions, C# 7: tuples ──────────────────────────────
public static class Program
{
    private static int checks;
    private static int failures;

    private static void Check(string label, bool ok)
    {
        checks++;
        if (!ok)
        {
            failures++;
            Console.WriteLine("FAIL " + label);
        }
    }

    private static void CheckStr(string label, string actual, string expected)
    {
        Check(label + " (got '" + actual + "' want '" + expected + "')", actual == expected);
    }

    public static void Main()
    {
        // ── String interpolation (C# 6, compiled through
        //    DefaultInterpolatedStringHandler since C# 10) ──────────────────
        int count = 25;
        string name = "RustCLR";
        CheckStr("interpolation", $"Found {count} on {name}", "Found 25 on RustCLR");
        CheckStr("interpolation nested", $"{$"{count}"}", "25");
        CheckStr("interpolation empty", $"", "");
        CheckStr("interpolation literal only", $"plain", "plain");
        CheckStr("interpolation alignment", $"[{count,5}]", "[   25]");
        CheckStr("interpolation left align", $"[{count,-5}]", "[25   ]");
        CheckStr("interpolation expression", $"{count * 2 + 1}", "51");
        CheckStr("interpolation bool", $"{count > 10}", "True");

        // ── Target-typed new (C# 9) ────────────────────────────────────────
        Reading reading = new() { Sensor = "temp-1", Value = 30.5 };
        CheckStr("init-only property", reading.Sensor, "temp-1");
        Check("init-only double", reading.Value == 30.5);

        // ── Tuples and deconstruction (C# 7) ───────────────────────────────
        var pair = (Low: 3, High: 9);
        Check("tuple field", pair.High == 9);
        var (low, high) = pair;
        Check("tuple deconstruction", low == 3 && high == 9);

        // ── Switch expressions (C# 8) ──────────────────────────────────────
        string DescribeShape(Shape s) => s switch
        {
            Shape.Circle => "round",
            Shape.Square => "boxy",
            _ => "pointy",
        };
        CheckStr("switch expression", DescribeShape(Shape.Circle), "round");
        CheckStr("switch expression default", DescribeShape(Shape.Triangle), "pointy");

        // ── Pattern matching (C# 7-9) ──────────────────────────────────────
        object boxed = 42;
        Check("type pattern", boxed is int);
        if (boxed is int unboxed)
        {
            Check("type pattern binding", unboxed == 42);
        }

        int score = 87;
        string grade = score switch
        {
            >= 90 => "A",
            >= 80 => "B",
            >= 70 => "C",
            _ => "F",
        };
        CheckStr("relational pattern", grade, "B");

        Check("logical pattern", score is > 50 and < 100);
        Check("negated pattern", score is not 0);

        // ── Null-coalescing and null-conditional (C# 6-8) ──────────────────
        string missing = null;
        CheckStr("null coalescing", missing ?? "fallback", "fallback");
        Check("null conditional", missing?.Length is null);

        string present = "abc";
        Check("null conditional value", present?.Length == 3);

        // ── Expression-bodied members (C# 6-7) ─────────────────────────────
        Check("expression-bodied", Doubled(21) == 42);

        // ── Local functions (C# 7), static local functions (C# 8) ──────────
        static int Triple(int n) => n * 3;
        Check("static local function", Triple(14) == 42);

        // ── out variables (C# 7) ───────────────────────────────────────────
        Check("out variable", int.TryParse("123", out int parsed) && parsed == 123);
        Check("out variable failure", !int.TryParse("nope", out int _));

        // ── Digit separators and binary literals (C# 7) ────────────────────
        Check("digit separator", 1_000_000 == 1000000);
        Check("binary literal", 0b1010_1010 == 170);

        // ── readonly struct with records (C# 10 record struct) ─────────────
        var origin = new Point(3, 4);
        Check("record struct field", origin.X == 3 && origin.Y == 4);

        // ── Ranges and indices (C# 8) on arrays ────────────────────────────
        int[] numbers = { 10, 20, 30, 40, 50 };
        Check("index from end", numbers[^1] == 50);
        int[] middle = numbers[1..4];
        Check("range length", middle.Length == 3);
        Check("range content", middle[0] == 20 && middle[2] == 40);

        // ── String methods that modern code leans on ───────────────────────
        CheckStr("string contains", "abcdef".Contains("cd") ? "yes" : "no", "yes");
        CheckStr("interpolated in method call", Wrap($"{low}-{high}"), "[3-9]");

        // ── using declarations do not need a runtime feature, but the
        //    compiler emits try/finally around them ────────────────────────
        Check("try/finally still works", ScopedTotal() == 30);

        Console.WriteLine("checks=" + checks + " failures=" + failures);
    }

    private static int Doubled(int n) => n * 2;

    private static string Wrap(string inner) => "[" + inner + "]";

    private static int ScopedTotal()
    {
        int total = 0;
        try
        {
            total += 10;
            total += 20;
        }
        finally
        {
            // A `using` declaration compiles to exactly this shape.
            total += 0;
        }
        return total;
    }
}
