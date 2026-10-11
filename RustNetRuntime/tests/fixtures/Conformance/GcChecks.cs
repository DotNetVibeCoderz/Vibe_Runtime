using System.Collections.Generic;
using System.Linq;

namespace Conformance;

// Called directly by crates/rustclr-bcl/tests/gc_natives.rs under a collector
// that runs as often as it can. Every callback here allocates, so a collection
// lands inside a native LINQ call: the delegate it was handed, and the objects
// a Select has gathered so far, are reachable only from that native.
static partial class Program
{
    static List<Tag> GcTags(int n) => Enumerable.Range(0, n).Select(i => new Tag("t" + i)).ToList();

    static int GcPartSelect(int n) => GcTags(n).Sum(t => t.ToString().Length);

    static int GcPartWhere(int n) => GcTags(n).Where(t => t.ToString().Contains('1') && new string('x', 48).Length == 48).Count();

    static int GcPartAny(int n) => GcTags(n).Any(t => new int[32].Length == 32 && t.ToString().EndsWith("99")) ? 1 : 0;

    static int GcPartLengths(int n) => GcTags(n).Select(t => t.ToString() + "!").Sum(s => s.Length);

    static List<Tag> LoopTags(int n)
    {
        var list = new List<Tag>();
        for (var i = 0; i < n; i++) list.Add(new Tag("t" + i));
        return list;
    }

    static int SumByLoop(List<Tag> tags)
    {
        var total = 0;
        foreach (var t in tags) total += t.ToString().Length;
        return total;
    }

    static int GcLoopLoop(int n) => SumByLoop(LoopTags(n));
    static int GcSelectLoop(int n) => SumByLoop(GcTags(n));
    static int GcLoopSum(int n) => LoopTags(n).Sum(t => t.ToString().Length);

    static int GcConstText(int n)
    {
        var list = new List<Tag>();
        for (var i = 0; i < n; i++) list.Add(new Tag("abc"));
        var total = 0;
        foreach (var t in list) total += t.ToString().Length;
        return total;
    }

    static int GcStringsOnly(int n)
    {
        var list = new List<string>();
        for (var i = 0; i < n; i++) list.Add("t" + i);
        var total = 0;
        foreach (var s in list) total += s.Length;
        return total;
    }

    static int GcStringsArray(int n)
    {
        var array = new string[n];
        for (var i = 0; i < n; i++) array[i] = "t" + i;
        var total = 0;
        for (var i = 0; i < n; i++) total += array[i].Length;
        return total;
    }

    static int GcTagsArray(int n)
    {
        var array = new Tag[n];
        for (var i = 0; i < n; i++) array[i] = new Tag("t" + i);
        var total = 0;
        for (var i = 0; i < n; i++) total += array[i].ToString().Length;
        return total;
    }

    static int LinqUnderCollection(int n) => n + GcPartWhere(n) + GcPartAny(n) + GcPartLengths(n);
}
