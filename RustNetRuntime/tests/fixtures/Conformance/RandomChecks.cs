using System;
using System.Linq;

namespace Conformance;

// A seeded Random is deterministic in .NET, and programs rely on it for
// reproducible simulations, games and tests. The sequence must match exactly.
static partial class Program
{
    static void RandomChecks()
    {
        var r = new Random(42);
        var ints = string.Join(",", Enumerable.Range(0, 5).Select(_ => r.Next()));
        var bounded = string.Join(",", Enumerable.Range(0, 8).Select(_ => r.Next(100)));
        var ranged = string.Join(",", Enumerable.Range(0, 6).Select(_ => r.Next(-5, 5)));
        var doubles = string.Join(",", Enumerable.Range(0, 3).Select(_ => r.NextDouble().ToString("0.000000", System.Globalization.CultureInfo.InvariantCulture)));
        var wide = r.Next(int.MinValue, int.MaxValue);
        var bytes = new byte[6];
        r.NextBytes(bytes);
        CheckStr("Random(42) reproduces .NET's sequence",
            $"{ints}|{bounded}|{ranged}|{doubles}|{wide}|{string.Join(",", bytes)}",
            RandomExpected);

        var a = new Random(7);
        var b = new Random(7);
        var same = Enumerable.Range(0, 20).All(_ => a.Next(1000) == b.Next(1000));
        var c = new Random(-123456);
        CheckStr("equal seeds give equal sequences, negative seeds work", $"{same}|{c.Next(1000)}|{new Random(0).Next()}", RandomSeedExpected);
    }
}

static partial class Program
{
    // Produced by dotnet (net10.0).
    const string RandomExpected = "1434747710,302596119,269548474,1122627734,361709742|26,72,51,17,76,23,25,50|-2,-2,-3,0,-5,3|0.577197,0.397471,0.152193|-193515359|110,139,9,20,87,154";
    const string RandomSeedExpected = "True|265|1559595546";
}
