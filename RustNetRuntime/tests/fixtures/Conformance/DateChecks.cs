using System;
using System.Globalization;

namespace Conformance;

// DateTime.ParseExact and TryParseExact with the invariant culture, as apps
// read stored dates ("yyyy-MM-dd") and what people type. Expected values were
// produced by dotnet.
static partial class Program
{
    static void DateChecks()
    {
        var inv = CultureInfo.InvariantCulture;
        string Show(DateTime d) => d.ToString("yyyy-MM-dd HH:mm:ss.fff", inv);
        var iso = DateTime.ParseExact("2026-10-11", "yyyy-MM-dd", inv);
        var dmy = DateTime.ParseExact("11/10/2026 07:05", "dd/MM/yyyy HH:mm", inv);
        var named = DateTime.ParseExact("9 Mar 26 3:04 PM", "d MMM yy h:mm tt", inv);
        var longForm = DateTime.ParseExact("Sunday, 11 October 2026", "dddd, dd MMMM yyyy", inv);
        var compact = DateTime.ParseExact("20261011", "yyyyMMdd", inv, DateTimeStyles.None);
        var oneOf = DateTime.ParseExact("11.10.2026", new[] { "yyyy-MM-dd", "dd.MM.yyyy" }, inv, DateTimeStyles.None);
        var fraction = DateTime.ParseExact("2026-10-11T08:30:05.125", "yyyy-MM-ddTHH:mm:ss.fff", inv);
        var wrongShape = DateTime.TryParseExact("2026-10-11", "yyyy/MM/dd", inv, DateTimeStyles.None, out var none);
        var trailing = DateTime.TryParseExact("2026-10-11 ", "yyyy-MM-dd", inv, DateTimeStyles.None, out _);
        var impossible = DateTime.TryParseExact("2026-02-30", "yyyy-MM-dd", inv, DateTimeStyles.None, out _);
        var good = DateTime.TryParseExact("2026-02-28", "yyyy-MM-dd", inv, DateTimeStyles.None, out var feb);
        var timeOnly = DateTime.ParseExact("14:05", "HH:mm", inv);
        var threw = "";
        try { DateTime.ParseExact("tomorrow", "yyyy-MM-dd", inv); }
        catch (FormatException e) { threw = e.GetType().Name; }
        CheckStr("DateTime.ParseExact with custom and invariant formats",
            $"{Show(iso)}|{Show(dmy)}|{Show(named)}|{Show(longForm)}|{Show(compact)}|{Show(oneOf)}|{Show(fraction)}|{wrongShape}|{none.Ticks}|{trailing}|{impossible}|{good}|{feb.Day}|{timeOnly.Date == DateTime.Today}|{timeOnly.Hour}:{timeOnly.Minute}|{threw}",
            "2026-10-11 00:00:00.000|2026-10-11 07:05:00.000|2026-03-09 15:04:00.000|2026-10-11 00:00:00.000|2026-10-11 00:00:00.000|2026-10-11 00:00:00.000|2026-10-11 08:30:05.125|False|0|False|False|True|28|True|14:5|FormatException");
    }
}
