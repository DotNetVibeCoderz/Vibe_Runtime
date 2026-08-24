using System;
using System.ComponentModel;
using System.Globalization;
using Microsoft.SemanticKernel;

namespace RustNet.Designer.Assistant.Plugins;

/// <summary>
/// Dates and clocks. A model has no reliable sense of "now", so anything
/// involving today's date, elapsed time or a schedule should come from here.
/// </summary>
public sealed class TimePlugin
{
    [KernelFunction("get_current_datetime")]
    [Description("The current date and time, on this machine and in UTC, with the day of week, "
        + "ISO week number and the machine's time zone.")]
    public string GetCurrentDateTime(
        [Description("Optional IANA or Windows time zone id, e.g. \"Asia/Jakarta\". Blank = this machine's zone.")]
        string timeZone = "")
    {
        DateTimeOffset now = DateTimeOffset.Now;
        string zoneLine = TimeZoneInfo.Local.DisplayName;

        if (timeZone.Trim().Length > 0)
        {
            try
            {
                TimeZoneInfo tz = TimeZoneInfo.FindSystemTimeZoneById(timeZone.Trim());
                now = TimeZoneInfo.ConvertTime(DateTimeOffset.UtcNow, tz);
                zoneLine = tz.DisplayName;
            }
            catch (Exception ex)
            {
                return $"Unknown time zone \"{timeZone}\": {ex.Message}. Reporting this machine's zone instead.\n"
                    + Describe(DateTimeOffset.Now, TimeZoneInfo.Local.DisplayName);
            }
        }
        return Describe(now, zoneLine);
    }

    private static string Describe(DateTimeOffset now, string zone) => $"""
        local   {now:yyyy-MM-dd HH:mm:ss} {now:zzz}
        utc     {now.UtcDateTime:yyyy-MM-dd HH:mm:ss}Z
        weekday {now.DayOfWeek}
        iso     {ISOWeek(now.DateTime)}
        zone    {zone}
        epoch   {now.ToUnixTimeSeconds()}
        """;

    private static string ISOWeek(DateTime date)
        => $"{System.Globalization.ISOWeek.GetYear(date)}-W{System.Globalization.ISOWeek.GetWeekOfYear(date):00}";

    [KernelFunction("date_add")]
    [Description("Shift a date by a number of days, months or years and return the result with its weekday.")]
    public string DateAdd(
        [Description("Start date as yyyy-MM-dd, or \"today\".")] string date,
        [Description("How many units to add; negative moves backwards.")] int amount,
        [Description("Unit: days, weeks, months or years.")] string unit = "days")
    {
        if (!TryParseDate(date, out DateTime start))
        {
            return $"Cannot read \"{date}\" as a date. Use yyyy-MM-dd or \"today\".";
        }
        DateTime result = unit.Trim().ToLowerInvariant() switch
        {
            "day" or "days" => start.AddDays(amount),
            "week" or "weeks" => start.AddDays(amount * 7),
            "month" or "months" => start.AddMonths(amount),
            "year" or "years" => start.AddYears(amount),
            _ => DateTime.MinValue,
        };
        if (result == DateTime.MinValue)
        {
            return $"Unknown unit \"{unit}\". Use days, weeks, months or years.";
        }
        return $"{start:yyyy-MM-dd} {amount:+#;-#;0} {unit} = {result:yyyy-MM-dd} ({result.DayOfWeek})";
    }

    [KernelFunction("date_difference")]
    [Description("Days between two dates, and the same span expressed in weeks and months.")]
    public string DateDifference(
        [Description("First date as yyyy-MM-dd, or \"today\".")] string from,
        [Description("Second date as yyyy-MM-dd, or \"today\".")] string to)
    {
        if (!TryParseDate(from, out DateTime a))
        {
            return $"Cannot read \"{from}\" as a date.";
        }
        if (!TryParseDate(to, out DateTime b))
        {
            return $"Cannot read \"{to}\" as a date.";
        }
        int days = (int)(b - a).TotalDays;
        int months = (b.Year - a.Year) * 12 + b.Month - a.Month;
        return $"{a:yyyy-MM-dd} -> {b:yyyy-MM-dd}: {days} days ({days / 7} weeks {Math.Abs(days % 7)} days, ~{months} months)";
    }

    [KernelFunction("format_duration")]
    [Description("Turn a number of milliseconds into a human duration — for frame budgets, timeouts and uptimes.")]
    public string FormatDuration(
        [Description("Milliseconds.")] long milliseconds)
    {
        TimeSpan t = TimeSpan.FromMilliseconds(milliseconds);
        if (milliseconds < 1000)
        {
            return $"{milliseconds} ms";
        }
        if (t.TotalDays >= 1)
        {
            return $"{milliseconds} ms = {(int)t.TotalDays}d {t.Hours}h {t.Minutes}m {t.Seconds}s";
        }
        if (t.TotalHours >= 1)
        {
            return $"{milliseconds} ms = {(int)t.TotalHours}h {t.Minutes}m {t.Seconds}s";
        }
        return $"{milliseconds} ms = {(int)t.TotalMinutes}m {t.Seconds}.{t.Milliseconds:000}s";
    }

    private static bool TryParseDate(string s, out DateTime date)
    {
        string t = s.Trim();
        if (t.Equals("today", StringComparison.OrdinalIgnoreCase))
        {
            date = DateTime.Today;
            return true;
        }
        if (t.Equals("tomorrow", StringComparison.OrdinalIgnoreCase))
        {
            date = DateTime.Today.AddDays(1);
            return true;
        }
        if (t.Equals("yesterday", StringComparison.OrdinalIgnoreCase))
        {
            date = DateTime.Today.AddDays(-1);
            return true;
        }
        return DateTime.TryParse(t, CultureInfo.InvariantCulture, DateTimeStyles.None, out date);
    }
}
