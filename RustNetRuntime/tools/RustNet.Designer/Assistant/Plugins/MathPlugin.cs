using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Globalization;
using System.Linq;
using Microsoft.SemanticKernel;

namespace RustNet.Designer.Assistant.Plugins;

/// <summary>
/// Arithmetic the assistant should not be doing in its head: frame budgets,
/// pixel arithmetic, ADC scaling, byte counts.
/// </summary>
public sealed class MathPlugin
{
    [KernelFunction("calculate")]
    [Description("Evaluate an arithmetic expression exactly. Supports + - * / % ^, parentheses, "
        + "pi and e, and the functions abs sqrt cbrt exp ln log log2 log10 sin cos tan asin acos atan "
        + "atan2 sinh cosh tanh floor ceil round trunc sign pow hypot deg rad min max sum avg.")]
    public string Calculate(
        [Description("The expression, e.g. \"(320-16)/8\" or \"round(3.3/4096*1500, 2)\".")] string expression)
    {
        try
        {
            double value = Expression.Evaluate(expression);
            if (double.IsNaN(value) || double.IsInfinity(value))
            {
                return $"{expression} = {value} (not a finite number)";
            }
            string exact = value.ToString("R", CultureInfo.InvariantCulture);
            string pretty = value == Math.Floor(value) && Math.Abs(value) < 1e15
                ? ((long)value).ToString(CultureInfo.InvariantCulture)
                : value.ToString("0.##########", CultureInfo.InvariantCulture);
            return pretty == exact ? $"{expression} = {pretty}" : $"{expression} = {pretty} (exact {exact})";
        }
        catch (Exception ex)
        {
            return $"Cannot evaluate \"{expression}\": {ex.Message}";
        }
    }

    [KernelFunction("statistics")]
    [Description("Count, sum, mean, median, min, max and standard deviation of a list of numbers.")]
    public string Statistics(
        [Description("Numbers separated by commas, spaces or newlines.")] string numbers)
    {
        List<double> values = new();
        foreach (string part in numbers.Split(new[] { ',', ' ', '\t', '\n', '\r', ';' },
                     StringSplitOptions.RemoveEmptyEntries))
        {
            if (double.TryParse(part, NumberStyles.Float, CultureInfo.InvariantCulture, out double v))
            {
                values.Add(v);
            }
        }
        if (values.Count == 0)
        {
            return "No numbers found in: " + numbers;
        }

        values.Sort();
        double sum = values.Sum();
        double mean = sum / values.Count;
        double median = values.Count % 2 == 1
            ? values[values.Count / 2]
            : (values[values.Count / 2 - 1] + values[values.Count / 2]) / 2;
        // Population standard deviation — these are complete sample sets
        // (pin counts, frame times), not samples of a larger population.
        double variance = values.Sum(v => (v - mean) * (v - mean)) / values.Count;

        return $"""
            count  {values.Count}
            sum    {F(sum)}
            mean   {F(mean)}
            median {F(median)}
            min    {F(values[0])}
            max    {F(values[^1])}
            stddev {F(Math.Sqrt(variance))}
            """;

        static string F(double v) => v.ToString("0.##########", CultureInfo.InvariantCulture);
    }

    [KernelFunction("convert_base")]
    [Description("Convert an integer between bases. Useful for register values, RGB565 words and masks.")]
    public string ConvertBase(
        [Description("The value, e.g. \"255\", \"0xFF\", \"0b1010\".")] string value,
        [Description("Target base: 2, 8, 10 or 16.")] int toBase = 16)
    {
        string s = value.Trim();
        try
        {
            long n = s.StartsWith("0x", StringComparison.OrdinalIgnoreCase)
                ? Convert.ToInt64(s.Substring(2), 16)
                : s.StartsWith("0b", StringComparison.OrdinalIgnoreCase)
                    ? Convert.ToInt64(s.Substring(2), 2)
                    : long.Parse(s, CultureInfo.InvariantCulture);

            return toBase switch
            {
                2 => $"{s} = 0b{Convert.ToString(n, 2)}",
                8 => $"{s} = 0o{Convert.ToString(n, 8)}",
                10 => $"{s} = {n}",
                16 => $"{s} = 0x{n:X}",
                _ => $"Base {toBase} is not supported; use 2, 8, 10 or 16.",
            };
        }
        catch (Exception ex)
        {
            return $"Cannot convert \"{value}\": {ex.Message}";
        }
    }
}
