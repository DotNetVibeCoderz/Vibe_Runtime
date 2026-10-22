using System;
using System.Collections.Generic;

namespace Conformance;

// Array.Sort over two arrays at once (keys and the items that go with them),
// with a comparer, and over a range; and copying a dictionary. A design tool
// sorts the edges of three hundred shapes this way to snap to them. Expected
// values were produced by dotnet.
static partial class Program
{
    sealed class Descending : IComparer<double>
    {
        public int Compare(double x, double y) => y.CompareTo(x);
    }

    static void SortChecks()
    {
        var keys = new[] { 30.5, 10.0, 20.25, 10.0, -4.0 };
        var items = new[] { 0, 1, 2, 3, 4 };
        Array.Sort(keys, items);
        CheckStr("Array.Sort(keys, items) moves the items with their keys (stable)",
            string.Join(",", keys) + "|" + string.Join(",", items), "-4,10,10,20.25,30.5|4,1,3,2,0");

        var names = new[] { "pear", "apple", "fig" };
        var prices = new[] { 3.5, 1.25, 7.0 };
        Array.Sort(names, prices);
        CheckStr("Array.Sort(string keys, double items)", string.Join(",", names) + "|" + string.Join(",", prices), "apple,fig,pear|1.25,7,3.5");

        var k2 = new[] { 1.0, 3.0, 2.0 };
        var i2 = new[] { "a", "c", "b" };
        Array.Sort(k2, i2, new Descending());
        CheckStr("Array.Sort(keys, items, comparer)", string.Join(",", k2) + "|" + string.Join(",", i2), "3,2,1|c,b,a");

        var part = new[] { 9, 8, 7, 6, 5, 4 };
        Array.Sort(part, 1, 4);
        CheckStr("Array.Sort(array, index, length) sorts only the range", string.Join(",", part), "9,5,6,7,8,4");

        var outOfRange = "";
        try { Array.Sort(part, 4, 5); }
        catch (ArgumentException e) { outOfRange = e.GetType().Name; }
        CheckStr("Array.Sort past the end throws", outOfRange, "ArgumentException");

        var list = new List<int> { 1 };
        var derived = "";
        try { _ = list[5]; }
        catch (ArgumentException e) { derived = e.GetType().Name; }
        CheckStr("catch (ArgumentException) catches the runtime's ArgumentOutOfRangeException", derived, "ArgumentOutOfRangeException");

        var withComparison = new[] { 3, 1, 2 };
        Array.Sort(withComparison, (a, b) => b - a);
        CheckStr("Array.Sort(array, Comparison) still works", string.Join(",", withComparison), "3,2,1");

        var source = new Dictionary<string, string> { ["n12"] = "07.03", ["n13"] = "Angke" };
        var copy = new Dictionary<string, string>(source);
        source["n12"] = "changed";
        CheckStr("new Dictionary(dictionary) copies the entries", copy.Count + "|" + copy["n12"] + "|" + copy["n13"], "2|07.03|Angke");
    }
}
