using System;

namespace Conformance;

// IndexOf with a start index (and a count) and a StringComparison: what a Markdown
// renderer uses to find the closing "**" after an opening one.
// Expected values were produced by dotnet.
static partial class Program
{
    static void StringSearchChecks()
    {
        const string text = "a **bold** and **More** text";
        CheckStr("IndexOf(string, int, StringComparison)",
            $"{text.IndexOf("**", 4, StringComparison.Ordinal)}|{text.IndexOf("**", 10, StringComparison.Ordinal)}|{text.IndexOf("MORE", 0, StringComparison.OrdinalIgnoreCase)}|{text.IndexOf("MORE", 0, StringComparison.Ordinal)}|{text.IndexOf("x", text.Length, StringComparison.Ordinal)}",
            "8|15|17|-1|-1");
        CheckStr("IndexOf(string, int, int, StringComparison) and IndexOf(string, int, int)",
            $"{text.IndexOf("**", 4, 10, StringComparison.Ordinal)}|{text.IndexOf("**", 4, 3, StringComparison.Ordinal)}|{text.IndexOf("BOLD", 0, 10, StringComparison.OrdinalIgnoreCase)}|{text.IndexOf("**", 4, 10)}",
            "8|-1|4|8");
        CheckStr("IndexOf(char, int, int) is a count, not a comparison",
            $"{text.IndexOf('*', 4, 3)}|{text.IndexOf('*', 10, 10)}|{text.IndexOf('*', 3, 1)}|{text.IndexOf('t', 20, 4)}",
            "-1|15|3|-1");
        var threw = false;
        try { _ = text.IndexOf("a", 99, StringComparison.Ordinal); } catch (ArgumentOutOfRangeException) { threw = true; }
        CheckStr("IndexOf past the end throws ArgumentOutOfRangeException", threw.ToString(), "True");
    }
}
