using System.Net;
using System.Text.RegularExpressions;

namespace RustNet.Designer.Assistant;

/// <summary>
/// Colours fenced code blocks in the chat transcript. Deliberately small: one
/// ordered alternation per language, comments and strings first so a keyword
/// inside a string is not re-coloured. It classifies, it does not parse — good
/// enough to read, and it cannot fail on syntactically invalid snippets, which
/// is what a chat transcript is full of.
/// </summary>
public static class CodeHighlighter
{
    private const RegexOptions Opts = RegexOptions.Singleline | RegexOptions.Compiled;

    private static readonly Regex CSharp = new(
        @"(?<cmt>//[^\n]*|/\*.*?\*/)"
        + @"|(?<str>@?\$?""(?:[^""\\\n]|\\.|"""")*""|'(?:[^'\\\n]|\\.)')"
        + @"|(?<num>\b0[xX][0-9a-fA-F_]+\b|\b\d[\d_]*(?:\.\d+)?(?:[eE][+-]?\d+)?[fFdDmMuUlL]*\b)"
        + @"|(?<kw>\b(?:abstract|as|async|await|base|bool|break|byte|case|catch|char|checked|class|const"
        + @"|continue|decimal|default|delegate|do|double|else|enum|event|explicit|extern|false|finally|fixed"
        + @"|float|for|foreach|get|goto|if|implicit|in|int|interface|internal|is|lock|long|namespace|new|null"
        + @"|object|operator|out|override|params|private|protected|public|readonly|record|ref|return|sbyte"
        + @"|sealed|set|short|sizeof|stackalloc|static|string|struct|switch|this|throw|true|try|typeof|uint"
        + @"|ulong|unchecked|unsafe|ushort|using|var|virtual|void|volatile|when|where|while|yield)\b)"
        + @"|(?<type>\b[A-Z][A-Za-z0-9_]*\b)", Opts);

    private static readonly Regex Xml = new(
        @"(?<cmt><!--.*?-->)"
        + @"|(?<tag></?[A-Za-z_][\w.:-]*)"
        + @"|(?<str>""[^""]*""|'[^']*')"
        + @"|(?<attr>[A-Za-z_][\w.:-]*(?=\s*=))"
        + @"|(?<punct>/?>)", Opts);

    private static readonly Regex Json = new(
        @"(?<key>""(?:[^""\\]|\\.)*"")(?=\s*:)"
        + @"|(?<str>""(?:[^""\\]|\\.)*"")"
        + @"|(?<num>-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b)"
        + @"|(?<kw>\b(?:true|false|null)\b)", Opts);

    private static readonly Regex Shell = new(
        @"(?<cmt>#[^\n]*)"
        + @"|(?<str>""(?:[^""\\]|\\.)*""|'[^']*')"
        + @"|(?<kw>\b(?:cargo|dotnet|rustnet|espflash|probe-rs|dfu-util|git|npm|if|then|else|fi|for|do|done|export)\b)", Opts);

    private static readonly Regex Rust = new(
        @"(?<cmt>//[^\n]*|/\*.*?\*/)"
        + @"|(?<str>b?""(?:[^""\\]|\\.)*"")"
        + @"|(?<num>\b0[xXbo][0-9a-fA-F_]+\b|\b\d[\d_]*(?:\.\d+)?(?:[uif](?:8|16|32|64|size))?\b)"
        + @"|(?<kw>\b(?:as|async|await|break|const|continue|crate|dyn|else|enum|extern|false|fn|for|if|impl"
        + @"|in|let|loop|match|mod|move|mut|pub|ref|return|self|Self|static|struct|super|trait|true|type"
        + @"|unsafe|use|where|while)\b)"
        + @"|(?<type>\b[A-Z][A-Za-z0-9_]*\b)", Opts);

    /// <summary>
    /// HTML for <paramref name="code"/>, span-wrapped per token class. Anything
    /// not matched is HTML-escaped and passed through, so unknown languages
    /// simply render as plain text.
    /// </summary>
    public static string Highlight(string code, string language)
    {
        Regex? rules = Normalize(language) switch
        {
            "csharp" => CSharp,
            "xml" => Xml,
            "json" => Json,
            "bash" => Shell,
            "rust" => Rust,
            _ => null,
        };
        if (rules == null)
        {
            return WebUtility.HtmlEncode(code);
        }

        var sb = new System.Text.StringBuilder(code.Length + 64);
        int last = 0;
        foreach (Match m in rules.Matches(code))
        {
            if (m.Index > last)
            {
                sb.Append(WebUtility.HtmlEncode(code.Substring(last, m.Index - last)));
            }
            sb.Append("<span class=\"t-").Append(ClassOf(m)).Append("\">")
              .Append(WebUtility.HtmlEncode(m.Value))
              .Append("</span>");
            last = m.Index + m.Length;
        }
        if (last < code.Length)
        {
            sb.Append(WebUtility.HtmlEncode(code.Substring(last)));
        }
        return sb.ToString();
    }

    private static string ClassOf(Match m)
    {
        foreach (string name in new[] { "cmt", "str", "num", "kw", "type", "attr", "tag", "key", "punct" })
        {
            if (m.Groups[name].Success)
            {
                return name;
            }
        }
        return "txt";
    }

    /// <summary>Map the fence's language tag onto one of the known rule sets.</summary>
    public static string Normalize(string language) => language.Trim().ToLowerInvariant() switch
    {
        "cs" or "c#" or "csharp" or "dotnet" => "csharp",
        "xml" or "xaml" or "html" or "svg" or "rnx" => "xml",
        "json" or "jsonc" => "json",
        "sh" or "bash" or "shell" or "console" or "powershell" or "ps1" or "cmd" => "bash",
        "rs" or "rust" => "rust",
        _ => language.Trim().ToLowerInvariant(),
    };
}
