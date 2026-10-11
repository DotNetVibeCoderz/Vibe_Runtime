using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;

namespace Conformance;

// decimal as business code uses it: prices, totals, rounding, formatting.
// Expected values were produced by dotnet.
static partial class Program
{
    sealed record LineItem(string Name, decimal Price, int Qty)
    {
        public decimal Total => Price * Qty;
    }

    static decimal taxRate = 0.11m;

    static void DecimalChecks()
    {
        var inv = CultureInfo.InvariantCulture;
        decimal a = 0.1m, b = 0.2m;
        CheckStr("decimal arithmetic is exact and keeps scale",
            $"{a + b}|{1.50m + 2.5m}|{1.10m * 3}|{10m - 0.01m}|{-2.5m * 4}|{7m % 2.5m}|{1m / 3m}|{2m / 3m}|{10m / 4m}|{1.00m / 1m}",
            DecimalArithmeticExpected);

        CheckStr("decimal rounding",
            $"{Math.Round(2.345m, 2)}|{Math.Round(2.345m, 2, MidpointRounding.AwayFromZero)}|{Math.Round(-2.5m)}|{decimal.Round(2.5m)}|{Math.Floor(-2.1m)}|{Math.Ceiling(2.1m)}|{decimal.Truncate(-2.9m)}|{Math.Round(1234.5678m, 2)}",
            DecimalRoundingExpected);

        decimal price = 19.99m;
        int qty = 3;
        double asDouble = (double)price;
        CheckStr("decimal conversions",
            $"{(int)price}|{asDouble.ToString(inv)}|{(decimal)0.1}|{(decimal)2.5f}|{(long)-7.9m}|{Convert.ToDecimal("12.50", inv)}|{decimal.Parse("1,234.56", inv)}|{decimal.TryParse("nope", out _)}|{price * qty}|{(decimal)qty / 4}",
            DecimalConversionExpected);

        CheckStr("decimal formatting",
            $"{1234567.891m.ToString("N2", inv)}|{5m.ToString("F2", inv)}|{0.125m.ToString("P1", inv)}|{12.5m.ToString("0.00", inv)}|{1234.5m:N0}|{-0.5m}|{decimal.MaxValue}|{decimal.Zero}|{decimal.One + decimal.MinusOne}",
            DecimalFormatExpected);

        CheckStr("decimal comparison and equality",
            $"{1.0m == 1.00m}|{1.0m.Equals(1.00m)}|{2.01m < 2.1m}|{Math.Max(3.5m, 2m)}|{Math.Min(-1m, 0.5m)}|{Math.Abs(-4.25m)}|{Math.Sign(-0.1m)}|{1.5m.CompareTo(2m)}|{new HashSet<decimal> { 1.0m, 1.00m, 2m }.Count}",
            DecimalCompareExpected);

        var lines = new List<LineItem> { new("Coffee", 4.50m, 3), new("Tea", 3.25m, 2), new("Cake", 6.00m, 1) };
        var subtotal = lines.Sum(l => l.Total);
        var tax = Math.Round(subtotal * taxRate, 2, MidpointRounding.AwayFromZero);
        CheckStr("decimal in records and LINQ",
            $"{subtotal}|{tax}|{subtotal + tax}|{lines.Max(l => l.Price)}|{lines.Min(l => l.Total)}|{lines.Average(l => l.Price)}|{string.Join(",", lines.OrderBy(l => l.Total).Select(l => l.Name))}|{lines[0] == new LineItem("Coffee", 4.5m, 3)}",
            DecimalLinqExpected);

        string overflow;
        try { var big = decimal.MaxValue; var _ = big + 1m; overflow = "none"; } catch (OverflowException) { overflow = "OverflowException"; }
        string divide;
        try { var zero = 0m; var _ = 1m / zero; divide = "none"; } catch (DivideByZeroException) { divide = "DivideByZeroException"; }
        CheckStr("decimal errors", $"{overflow}|{divide}", "OverflowException|DivideByZeroException");
    }
}

static partial class Program
{
    const string DecimalArithmeticExpected = "0.3|4.00|3.30|9.99|-10.0|2.0|0.3333333333333333333333333333|0.6666666666666666666666666667|2.5|1.00";
    const string DecimalRoundingExpected = "2.34|2.35|-2|2|-3|3|-2|1234.57";
    const string DecimalConversionExpected = "19|19.99|0.1|2.5|-7|12.50|1234.56|False|59.97|0.75";
    const string DecimalFormatExpected = "1,234,567.89|5.00|12.5 %|12.50|1,235|-0.5|79228162514264337593543950335|0|0";
    const string DecimalCompareExpected = "True|True|True|3.5|-1|4.25|-1|-1|2";
    const string DecimalLinqExpected = "26.00|2.86|28.86|6.00|6.00|4.5833333333333333333333333333|Cake,Tea,Coffee|True";
}
