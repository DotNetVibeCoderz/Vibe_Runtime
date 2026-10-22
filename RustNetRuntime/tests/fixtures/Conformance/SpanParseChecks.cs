using System;
using System.Globalization;

namespace Conformance;

// double.Parse over a slice of a string without copying it: how a reader of a long
// comma-separated line takes each field. The span overload binds to the same arity
// as Parse(string, NumberStyles, IFormatProvider).
// Expected values were produced by dotnet.
static partial class Program
{
    static void SpanParseChecks()
    {
        const string line = "12.5,-0.25,1e3,7";
        var sum = 0.0;
        var start = 0;
        var fields = 0;
        for (var k = 0; k <= line.Length; k++)
        {
            if (k == line.Length || line[k] == ',')
            {
                sum += double.Parse(line.AsSpan(start, k - start), NumberStyles.Float, CultureInfo.InvariantCulture);
                fields++;
                start = k + 1;
            }
        }
        CheckStr("double.Parse(ReadOnlySpan<char>, NumberStyles, IFormatProvider)", $"{fields}|{sum.ToString(CultureInfo.InvariantCulture)}", "4|1019.25");
        var ok = double.TryParse("3.75".AsSpan(), NumberStyles.Float, CultureInfo.InvariantCulture, out var x);
        CheckStr("double.TryParse(ReadOnlySpan<char>, NumberStyles, IFormatProvider, out double)", $"{ok}|{x.ToString(CultureInfo.InvariantCulture)}", "True|3.75");
    }
}
