using System.Net;
using System.Text;
using System.Text.RegularExpressions;

namespace RustNet.Designer.Assistant;

/// <summary>
/// Turns fetched HTML into something a model can read: scripts, styles and
/// navigation chrome dropped, block elements turned into line breaks, entities
/// decoded, runs of blank lines collapsed. Deliberately regex-based — the goal
/// is legible text for a prompt, not a DOM.
/// </summary>
public static class HtmlToText
{
    private static readonly RegexOptions Opts =
        RegexOptions.Singleline | RegexOptions.IgnoreCase | RegexOptions.Compiled;

    private static readonly Regex Dropped = new(
        @"<(script|style|noscript|svg|template|head|nav|footer|form)\b[^>]*>.*?</\1\s*>", Opts);
    private static readonly Regex Comments = new(@"<!--.*?-->", Opts);
    private static readonly Regex Headings = new(@"</?h([1-6])\b[^>]*>", Opts);
    private static readonly Regex ListItems = new(@"<li\b[^>]*>", Opts);
    private static readonly Regex Breaks = new(
        @"</?(p|div|section|article|tr|ul|ol|table|br|hr|h[1-6]|blockquote|pre)\b[^>]*>", Opts);
    private static readonly Regex Cells = new(@"</(td|th)\s*>", Opts);
    private static readonly Regex Tags = new(@"<[^>]+>", Opts);
    private static readonly Regex Spaces = new(@"[ \t\f\v]{2,}", Opts);
    private static readonly Regex BlankRuns = new(@"(\r?\n\s*){3,}", Opts);

    public static string Convert(string html)
    {
        if (string.IsNullOrWhiteSpace(html))
        {
            return "";
        }

        string title = ExtractTitle(html);

        string s = Comments.Replace(html, " ");
        s = Dropped.Replace(s, "\n");
        // Keep the document's outline: headings become markdown headings, list
        // items become bullets, table cells stay on one line separated by pipes.
        s = Headings.Replace(s, m => m.Value.StartsWith("</", System.StringComparison.Ordinal)
            ? "\n"
            : "\n" + new string('#', int.Parse(m.Groups[1].Value)) + " ");
        s = ListItems.Replace(s, "\n- ");
        s = Cells.Replace(s, " | ");
        s = Breaks.Replace(s, "\n");
        s = Tags.Replace(s, "");
        s = WebUtility.HtmlDecode(s);
        s = s.Replace("\r\n", "\n").Replace('\r', '\n');
        s = Spaces.Replace(s, " ");

        var sb = new StringBuilder();
        foreach (string line in s.Split('\n'))
        {
            sb.Append(line.Trim()).Append('\n');
        }
        s = BlankRuns.Replace(sb.ToString(), "\n\n").Trim();

        return title.Length > 0 ? $"## {title}\n\n{s}" : s;
    }

    private static string ExtractTitle(string html)
    {
        Match m = Regex.Match(html, @"<title\b[^>]*>(.*?)</title\s*>", Opts);
        return m.Success ? WebUtility.HtmlDecode(m.Groups[1].Value).Trim() : "";
    }
}
