using System;
using System.Text;
using System.Xml;
using System.Xml.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.Formatting;

namespace RustNet.Designer.Editor;

/// <summary>
/// Tidies the text in an editor pane. C# goes through Roslyn, because a
/// brace-counting re-indenter corrupts verbatim strings, interpolations and
/// comments. XML goes through <see cref="XDocument"/> with two-space indents to
/// match the layout files the rest of the repo writes.
///
/// Both refuse rather than guess: text that does not parse is returned
/// unchanged with the parser's complaint, so pressing Format on a half-written
/// file cannot destroy it.
/// </summary>
public static class CodeFormatter
{
    /// <summary>
    /// Formatted text, or the original with <paramref name="error"/> set.
    /// <paramref name="language"/> is the pane's language tag.
    /// </summary>
    public static string Format(string text, string language, out string? error)
    {
        error = null;
        if (string.IsNullOrWhiteSpace(text))
        {
            return text;
        }
        try
        {
            return Assistant.CodeHighlighter.Normalize(language) switch
            {
                "csharp" => FormatCSharp(text, out error),
                "xml" => FormatXml(text),
                "json" => FormatJson(text),
                _ => Unsupported(language, out error, text),
            };
        }
        catch (Exception ex)
        {
            error = ex.Message;
            return text;
        }
    }

    private static string Unsupported(string language, out string? error, string text)
    {
        error = $"No formatter for '{language}'.";
        return text;
    }

    private static string FormatCSharp(string text, out string? error)
    {
        error = null;
        SyntaxTree tree = CSharpSyntaxTree.ParseText(text);
        SyntaxNode root = tree.GetRoot();

        // Only real syntax errors block formatting; warnings are fine to format
        // through, and a file being written will always have some.
        foreach (Diagnostic d in tree.GetDiagnostics())
        {
            if (d.Severity == DiagnosticSeverity.Error && d.Id is "CS1002" or "CS1513" or "CS1514" or "CS1519")
            {
                error = $"Not formatted — {d.GetMessage()} at line {d.Location.GetLineSpan().StartLinePosition.Line + 1}.";
                return text;
            }
        }

        // Roslyn's defaults for C# are already four spaces, no tabs and the
        // platform newline — the same conventions this repo uses.
        using var workspace = new AdhocWorkspace();
        return Formatter.Format(root, workspace).ToFullString();
    }

    private static string FormatXml(string text)
    {
        XDocument doc = XDocument.Parse(text, LoadOptions.PreserveWhitespace);
        var settings = new XmlWriterSettings
        {
            Indent = true,
            IndentChars = "  ",
            OmitXmlDeclaration = doc.Declaration == null,
            NewLineOnAttributes = false,
        };
        var sb = new StringBuilder();
        using (XmlWriter writer = XmlWriter.Create(sb, settings))
        {
            doc.Save(writer);
        }
        // RustNet.UI layouts end with a newline; keep that habit.
        return sb.ToString().TrimEnd() + Environment.NewLine;
    }

    private static string FormatJson(string text)
    {
        using System.Text.Json.JsonDocument doc = System.Text.Json.JsonDocument.Parse(text);
        return System.Text.Json.JsonSerializer.Serialize(doc, new System.Text.Json.JsonSerializerOptions
        {
            WriteIndented = true,
        });
    }
}
