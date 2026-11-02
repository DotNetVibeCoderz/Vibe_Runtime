using System;
using System.Collections.Generic;
using System.Linq;

namespace Conformance;

// Shapes a retained-mode UI toolkit (Buitenzorg.UI) needed that RustCLR got
// wrong or lacked. Expected values were produced by dotnet.
static partial class Program
{
    class UiNode { public UiNode? Parent; }
    sealed class UiWindow : UiNode { }
    sealed class UiPanel : UiNode { }

    // `p as T` with `T : class` compiles to `isinst !!T; unbox.any !!T`. A
    // failed test hands `unbox.any` a null, which is a cast to a reference
    // type and must give null rather than throw NullReferenceException.
    static T? FindAncestor<T>(UiNode n) where T : UiNode
    {
        for (var p = n.Parent; p != null; p = p.Parent) { var t = p as T; if (t != null) return t; }
        return null;
    }

    static T? FindAncestorObject<T>(UiNode n) where T : UiNode
    {
        for (UiNode? p = n.Parent; p != null; p = p.Parent) { object? o = p as T; if (o != null) return (T)o; }
        return null;
    }

    static T? AsOrDefault<T>(object? o) where T : class => o as T;
    static T? NullableFrom<T>(object? o) where T : struct => (T?)o;

    static void UiToolkitChecks()
    {
        var window = new UiWindow();
        var panel = new UiPanel { Parent = window };
        var leaf = new UiNode { Parent = panel };
        CheckStr("generic `as T` that fails inside a loop",
            $"{FindAncestor<UiWindow>(leaf) == window}|{FindAncestorObject<UiWindow>(leaf) == window}|{FindAncestor<UiPanel>(leaf) == panel}|{FindAncestor<UiWindow>(window) == null}",
            "True|True|True|True");
        CheckStr("unbox.any of null to a reference type and to Nullable<T>",
            $"{AsOrDefault<string>(5) == null}|{AsOrDefault<string>(null) == null}|{AsOrDefault<string>("s")}|{NullableFrom<int>(null).HasValue}|{NullableFrom<int>(7)}",
            "True|True|s|False|7");

        const string text = "a.b.c/d.e";
        CheckStr("LastIndexOf(char, int) and (char, int, int)",
            $"{text.LastIndexOf('.', 4)}|{text.LastIndexOf('.', 2)}|{text.LastIndexOf('.', 8, 3)}|{text.LastIndexOf('.', 6, 3)}|{text.LastIndexOf('.', 0)}|{"".LastIndexOf('x', 0)}",
            "3|1|7|-1|-1|-1");
        CheckStr("LastIndexOf(string, int, ...) overloads",
            $"{text.LastIndexOf(".", 4)}|{text.LastIndexOf("c/", 8, 5)}|{text.LastIndexOf("c/", 8, 3)}|{text.LastIndexOf("B.C", 6, StringComparison.OrdinalIgnoreCase)}|{text.LastIndexOf("B.C", 8, 7, StringComparison.Ordinal)}|{text.LastIndexOf(".", text.Length)}|{text.LastIndexOf("", 3)}|{"".LastIndexOf("", 0)}",
            "3|4|-1|2|-1|7|4|0");
        CheckStr("IndexOfAny / LastIndexOfAny with a start and a count",
            $"{text.IndexOfAny(new[] { '/', 'c' }, 2)}|{text.IndexOfAny(new[] { '/' }, 0, 5)}|{text.LastIndexOfAny(new[] { '.', '/' })}|{text.LastIndexOfAny(new[] { '/', 'a' }, 4)}|{text.LastIndexOfAny(new[] { '.' }, 6, 2)}",
            "4|-1|7|0|-1");
        var threw = false;
        try { _ = text.LastIndexOf('.', 99); } catch (ArgumentOutOfRangeException) { threw = true; }
        CheckStr("LastIndexOf past the end throws ArgumentOutOfRangeException", threw.ToString(), "True");

        // `IComparable.CompareTo(object)` on a boxed primitive: an interface
        // call whose receiver is a box, as a sorted list of `object` keys does.
        var keys = new List<object> { 3, 1, 2 };
        keys.Sort((x, y) => ((IComparable)x).CompareTo(y));
        CheckStr("IComparable.CompareTo on boxed values",
            $"{string.Join(",", keys)}|{Sign(((IComparable)5L).CompareTo(7L))}|{Sign(((IComparable)2.5).CompareTo(1.0))}|{Sign(((IComparable)"b").CompareTo("a"))}|{((IComparable)4).CompareTo(null)}|{((IComparable)'a').CompareTo('a')}|{Sign(((IComparable)true).CompareTo(false))}",
            "1,2,3|-1|1|1|1|0|1");
        var mixed = false;
        try { _ = ((IComparable)1).CompareTo("x"); } catch (ArgumentException) { mixed = true; }
        CheckStr("IComparable.CompareTo with another type throws ArgumentException", mixed.ToString(), "True");

        CheckStr("Environment.GetEnvironmentVariable of an unset name",
            (Environment.GetEnvironmentVariable("RUSTCLR_CONFORMANCE_SURELY_UNSET") == null).ToString(), "True");

        // `args.Contains(x)` on an array binds to the .NET 10 span overload
        // `MemoryExtensions.Contains<T>(ReadOnlySpan<T>, T)`.
        string[] flags = { "--selftest", "--fast" };
        int[] numbers = { 4, 5, 6 };
        CheckStr("MemoryExtensions.Contains on arrays",
            $"{flags.Contains("--fast")}|{flags.Contains("--slow")}|{numbers.Contains(5)}|{numbers.Contains(9)}|{"abc".AsSpan().Contains('b')}",
            "True|False|True|False|True");
    }

    static int Sign(int v) => v < 0 ? -1 : v > 0 ? 1 : 0;
}
