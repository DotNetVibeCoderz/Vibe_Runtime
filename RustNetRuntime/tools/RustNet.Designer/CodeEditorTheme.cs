using System;
using System.Windows.Media;
using ICSharpCode.AvalonEdit;
using ICSharpCode.AvalonEdit.Highlighting;

namespace RustNet.Designer;

/// <summary>
/// AvalonEdit ships syntax colours tuned for a white page — navy tags and
/// crimson strings, which are close to invisible on this tool's near-black
/// editors. This remaps the definitions we use onto the same token palette the
/// chat transcript uses, so a snippet reads identically whether Jack wrote it or
/// the editor is showing it.
///
/// Colours are matched by the definition's own colour names rather than
/// enumerated per language, so a definition we have not thought about still
/// comes out readable.
/// </summary>
public static class CodeEditorTheme
{
    private static readonly Color Comment = FromHex("#6A7482");
    private static readonly Color Str = FromHex("#B6D99B");
    private static readonly Color Number = FromHex("#DFC184");
    private static readonly Color Keyword = FromHex("#7FB2EA");
    private static readonly Color TypeName = FromHex("#6FD3C0");
    private static readonly Color Attribute = FromHex("#E8A33D");
    private static readonly Color Punctuation = FromHex("#8A93A1");

    private static bool _applied;

    /// <summary>Recolour the shared highlighting definitions. Idempotent.</summary>
    public static void ApplyDarkPalette()
    {
        if (_applied)
        {
            return;
        }
        _applied = true;

        foreach (string language in new[] { "C#", "XML", "Json", "C++", "JavaScript", "TSQL", "MarkDown" })
        {
            IHighlightingDefinition? definition = HighlightingManager.Instance.GetDefinition(language);
            if (definition == null)
            {
                continue;
            }
            foreach (HighlightingColor color in definition.NamedHighlightingColors)
            {
                color.Foreground = new SimpleHighlightingBrush(PickFor(color.Name));
                // The light definitions use background fills for a few tokens
                // (CDATA, doc comments); on a dark page they read as damage.
                color.Background = null;
            }
        }
    }

    private static Color PickFor(string name)
    {
        if (name.Contains("Comment", StringComparison.OrdinalIgnoreCase))
        {
            return Comment;
        }
        if (name.Contains("String", StringComparison.OrdinalIgnoreCase)
            || name.Contains("Char", StringComparison.OrdinalIgnoreCase)
            || name.Contains("AttributeValue", StringComparison.OrdinalIgnoreCase)
            || name.Contains("CData", StringComparison.OrdinalIgnoreCase))
        {
            return Str;
        }
        if (name.Contains("Number", StringComparison.OrdinalIgnoreCase)
            || name.Contains("Digit", StringComparison.OrdinalIgnoreCase))
        {
            return Number;
        }
        if (name.Contains("AttributeName", StringComparison.OrdinalIgnoreCase)
            || name.Contains("Property", StringComparison.OrdinalIgnoreCase))
        {
            return Attribute;
        }
        if (name.Contains("Punctuation", StringComparison.OrdinalIgnoreCase)
            || name.Contains("Bracket", StringComparison.OrdinalIgnoreCase))
        {
            return Punctuation;
        }
        if (name.Contains("Type", StringComparison.OrdinalIgnoreCase)
            || name.Contains("MethodCall", StringComparison.OrdinalIgnoreCase)
            || name.Contains("Class", StringComparison.OrdinalIgnoreCase))
        {
            return TypeName;
        }
        // Everything left is a keyword family (Keywords, Modifiers, Visibility,
        // XmlTag, Entity, Preprocessor …).
        return Keyword;
    }

    /// <summary>
    /// The highlighting definition for a language tag, or null for plain text.
    /// AvalonEdit ships no Rust definition, so Rust borrows C++ — close enough
    /// for keywords, strings and comments.
    /// </summary>
    public static IHighlightingDefinition? DefinitionFor(string language)
    {
        ApplyDarkPalette();
        string name = Assistant.CodeHighlighter.Normalize(language) switch
        {
            "csharp" => "C#",
            "xml" => "XML",
            "json" => "Json",
            "rust" => "C++",
            _ => "",
        };
        return name.Length == 0 ? null : HighlightingManager.Instance.GetDefinition(name);
    }

    /// <summary>Selection, caret and current-line colours for one editor.</summary>
    public static void Style(TextEditor editor)
    {
        editor.TextArea.SelectionBrush = new SolidColorBrush(FromHex("#2F5F70")) { Opacity = 0.55 };
        editor.TextArea.SelectionBorder = null;
        editor.TextArea.SelectionForeground = null;
        editor.TextArea.Caret.CaretBrush = new SolidColorBrush(FromHex("#4FC3E8"));
        editor.Options.HighlightCurrentLine = true;
        editor.TextArea.TextView.CurrentLineBackground = new SolidColorBrush(FromHex("#161A21"));
        editor.TextArea.TextView.CurrentLineBorder = null;
        editor.Options.ConvertTabsToSpaces = true;
        editor.Options.IndentationSize = 4;
        editor.Options.EnableHyperlinks = false;
        editor.Options.EnableEmailHyperlinks = false;
    }

    private static Color FromHex(string hex) => (Color)ColorConverter.ConvertFromString(hex);
}
