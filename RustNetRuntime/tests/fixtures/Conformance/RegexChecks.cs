using System;
using System.Collections.Generic;
using System.Linq;
using System.Text.RegularExpressions;

namespace Conformance;

// System.Text.RegularExpressions, as programs use it: validation, extraction,
// replacement and splitting. Every expected value here was produced by dotnet.
static partial class Program
{
    static void RegexChecks()
    {
        var email = new Regex(@"^[\w.+-]+@[\w-]+(\.[\w-]+)+$", RegexOptions.IgnoreCase);
        CheckStr("Regex.IsMatch for validation",
            $"{email.IsMatch("ada@example.com")}|{email.IsMatch("not an email")}|{Regex.IsMatch("2026-10-11", @"^\d{4}-\d{2}-\d{2}$")}|{Regex.IsMatch("ABC", "abc", RegexOptions.IgnoreCase)}",
            "True|False|True|True");

        var m = Regex.Match("Order #1042 shipped 2026-10-11", @"#(?<id>\d+).*?(\d{4})-(\d{2})");
        CheckStr("Match, groups by number and name",
            $"{m.Success}|{m.Value}|{m.Index}|{m.Length}|{m.Groups["id"].Value}|{m.Groups[1].Value}|{m.Groups[2].Value}|{m.Groups.Count}|{m.Groups["nope"].Success}|{m.Groups[3].Name}",
            "True|#1042 shipped 2026-10|6|21|1042|2026|10|4|False|id");

        var words = Regex.Matches("the cat sat on the mat", @"\b\w*at\b");
        var joined = "";
        foreach (Match w in words) joined += w.Value + "@" + w.Index + " ";
        CheckStr("Matches, foreach and LINQ",
            $"{words.Count}|{joined.Trim()}|{string.Join(",", words.Select(x => x.Value.ToUpper()))}|{words[2].Value}",
            "3|cat@4 sat@8 mat@19|CAT,SAT,MAT|mat");

        CheckStr("Replace with $-substitutions and an evaluator",
            $"{Regex.Replace("Lovelace, Ada", @"(\w+), (\w+)", "$2 $1")}|{Regex.Replace("a1b22c333", @"\d+", mm => (mm.Value.Length * 10).ToString())}|{new Regex(@"\s+").Replace("  too   many  spaces ", " ").Trim()}|{Regex.Replace("x", "x", "$$1")}|{Regex.Replace("2026-10-11", @"(?<y>\d+)-(?<m>\d+)-(?<d>\d+)", "${d}/${m}/${y}")}",
            "Ada Lovelace|a10b20c30|too many spaces|$1|11/10/2026");

        CheckStr("Split, Escape and Multiline",
            $"{string.Join("|", Regex.Split("a, b;c ,d", @"\s*[,;]\s*"))}#{string.Join("|", Regex.Split("1-2-3", "(-)"))}#{Regex.Escape("1+1=2?")}#{Regex.Matches("a\nb\nc", @"^\w$", RegexOptions.Multiline).Count}",
            @"a|b|c|d#1|-|2|-|3#1\+1=2\?#3");

        var next = Regex.Match("k1=v1;k2=v2", @"(\w+)=(\w+)");
        var pairs = new List<string>();
        while (next.Success)
        {
            pairs.Add(next.Groups[1].Value + ":" + next.Groups[2].Value);
            next = next.NextMatch();
        }
        CheckStr("NextMatch, lookbehind, Unicode and Result",
            $"{string.Join(",", pairs)}|{Regex.Match("price: $42.50", @"(?<=\$)\d+(\.\d+)?").Value}|{Regex.Match("Grüße aus München", @"\p{Lu}\w+$").Value}|{Regex.Match("ab", "(?<x>a)").Result("[${x}]")}",
            "k1:v1,k2:v2|42.50|München|[a]");

        string err;
        try
        {
            _ = new Regex("(unclosed");
            err = "no error";
        }
        catch (ArgumentException)
        {
            err = "ArgumentException";
        }
        CheckStr("a bad pattern throws ArgumentException", err, "ArgumentException");
    }
}
