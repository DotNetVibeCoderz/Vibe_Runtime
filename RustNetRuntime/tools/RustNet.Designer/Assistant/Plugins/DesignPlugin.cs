using System;
using System.ComponentModel;
using System.Globalization;
using System.Text;
using Microsoft.SemanticKernel;
using RustNet.UI;

namespace RustNet.Designer.Assistant.Plugins;

/// <summary>
/// The functions that let the assistant actually build something: read the
/// contracts, inspect the canvas, validate a layout, put it on the canvas, and
/// drop generated C# into the code pane.
/// </summary>
public sealed class DesignPlugin
{
    private readonly IDesignerBridge _designer;

    public DesignPlugin(IDesignerBridge designer) => _designer = designer;

    // ---- reference -----------------------------------------------------

    [KernelFunction("get_ui_reference")]
    [Description("The RustNet.UI XML layout format: every element kind, its attributes, "
        + "the RGB565 colour encoding and the 8x8 font metrics. Read this before writing a layout.")]
    public string GetUiReference() => RustNetReference.UiMarkup;

    [KernelFunction("get_graphics_reference")]
    [Description("The RustNet.Graphics.Display drawing API: which calls are native intrinsics, "
        + "which are managed helpers, and the shape of a frame loop. Read this before writing drawing code.")]
    public string GetGraphicsReference() => RustNetReference.Graphics;

    [KernelFunction("get_language_limits")]
    [Description("What the RustNet IL interpreter accepts of C#, and the specific limits that break "
        + "otherwise-correct code (untyped catch clauses, partial reflection, same-frame ref). "
        + "Read this before using anything beyond the language core.")]
    public string GetLanguageLimits() => RustNetReference.LanguageLimits;

    // ---- inspect -------------------------------------------------------

    [KernelFunction("get_current_layout")]
    [Description("The layout currently open on the designer canvas, as RustNet.UI XML.")]
    public string GetCurrentLayout() => _designer.GetLayoutXml();

    [KernelFunction("describe_panel")]
    [Description("The panel the person is designing for: pixel size, how many characters fit per line "
        + "at each text scale, and what is selected right now.")]
    public string DescribePanel()
    {
        (int w, int h) = _designer.GetPanelSize();
        return $"""
            Panel: {w}x{h} px, RGB565.
            Text: 8x8 font, advances 8*scale px per character.
              scale 1 -> {w / 8} chars per line, {h / 8} lines
              scale 2 -> {w / 16} chars per line, {h / 16} lines
            Selected: {_designer.DescribeSelection()}
            """;
    }

    // ---- produce -------------------------------------------------------

    [KernelFunction("validate_layout_xml")]
    [Description("Parse RustNet.UI XML without applying it. Returns the element count and outline "
        + "when it is valid, or the parse error when it is not. Validate before applying.")]
    public string ValidateLayoutXml(
        [Description("The full RustNet.UI XML document, starting with <window ...>.")] string xml)
    {
        try
        {
            UiElement root = Ui.LoadXml(xml);
            var sb = new StringBuilder();
            sb.AppendLine($"Valid. Root <{root.Kind}> {Size(root)}, {Count(root)} elements.");
            Outline(root, sb, 0);
            string report = sb.ToString();
            string warnings = Warnings(root);
            return warnings.Length > 0 ? report + "\nWarnings:\n" + warnings : report;
        }
        catch (Exception ex)
        {
            return "Invalid: " + ex.Message;
        }
    }

    [KernelFunction("apply_layout_xml")]
    [Description("Replace the designer canvas with this RustNet.UI XML. The person sees the result "
        + "immediately. Returns the applied outline, or the parse error if nothing was applied.")]
    public string ApplyLayoutXml(
        [Description("The full RustNet.UI XML document, starting with <window ...>.")] string xml)
    {
        try
        {
            _designer.ApplyLayoutXml(xml);
        }
        catch (Exception ex)
        {
            return "Not applied: " + ex.Message;
        }
        return "Applied to the canvas.\n" + ValidateLayoutXml(xml);
    }

    [KernelFunction("set_generated_code")]
    [Description("Put generated source in the designer's code pane, where the person can read, edit "
        + "and save it. Use this for app code instead of only pasting it into the chat.")]
    public string SetGeneratedCode(
        [Description("File name including extension, e.g. Program.cs or ui.xml.")] string fileName,
        [Description("Language tag: csharp, xml, json, or text.")] string language,
        [Description("The complete file contents.")] string code)
    {
        _designer.SetGeneratedCode(fileName, language, code);
        int lines = code.Split('\n').Length;
        return $"{fileName} ({lines} lines) is in the code pane.";
    }

    [KernelFunction("get_generated_code")]
    [Description("Whatever is in the designer's code pane now, so you can revise it instead of rewriting it.")]
    public string GetGeneratedCode()
    {
        string code = _designer.GetGeneratedCode();
        return code.Length == 0 ? "(the code pane is empty)" : code;
    }

    // ---- colour --------------------------------------------------------

    [KernelFunction("rgb565")]
    [Description("Convert an 8-bit RGB colour to the four-hex-digit RGB565 value the layout format uses.")]
    public string Rgb565(
        [Description("Red 0-255.")] int r,
        [Description("Green 0-255.")] int g,
        [Description("Blue 0-255.")] int b)
    {
        int Clamp(int v) => v < 0 ? 0 : v > 255 ? 255 : v;
        r = Clamp(r); g = Clamp(g); b = Clamp(b);
        int packed = ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3);
        // Report the colour the panel will actually show: RGB565 quantises,
        // so a requested value and the displayed value are not the same.
        int br = ((packed >> 11) & 0x1F) << 3;
        int bg = ((packed >> 5) & 0x3F) << 2;
        int bb = (packed & 0x1F) << 3;
        return $"{packed:X4}  (rgb({r},{g},{b}) displays as rgb({br},{bg},{bb}))";
    }

    [KernelFunction("rgb565_from_hex")]
    [Description("Convert a web hex colour like #2A9D8F to the four-hex-digit RGB565 value.")]
    public string Rgb565FromHex(
        [Description("Hex colour, with or without the leading #, 3 or 6 digits.")] string hex)
    {
        string s = hex.Trim().TrimStart('#');
        if (s.Length == 3)
        {
            s = string.Concat(s[0], s[0], s[1], s[1], s[2], s[2]);
        }
        if (s.Length != 6 || !int.TryParse(s, NumberStyles.HexNumber, CultureInfo.InvariantCulture, out int v))
        {
            return $"Not a hex colour: {hex}";
        }
        return Rgb565((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF);
    }

    // ---- helpers -------------------------------------------------------

    private static string Size(UiElement e) => $"{(e.Width > 0 ? e.Width : 160)}x{(e.Height > 0 ? e.Height : 128)}";

    private static int Count(UiElement e)
    {
        int n = 1;
        foreach (UiElement c in e.Children)
        {
            n += Count(c);
        }
        return n;
    }

    private static void Outline(UiElement e, StringBuilder sb, int depth)
    {
        sb.Append(' ', depth * 2);
        sb.Append('<').Append(e.Kind);
        if (e.Id.Length > 0)
        {
            sb.Append(" #").Append(e.Id);
        }
        if (e.Text.Length > 0)
        {
            sb.Append(" \"").Append(e.Text.Length > 24 ? e.Text.Substring(0, 23) + "…" : e.Text).Append('"');
        }
        sb.AppendLine(">");
        foreach (UiElement c in e.Children)
        {
            Outline(c, sb, depth + 1);
        }
    }

    /// <summary>
    /// Mistakes the parser accepts but the panel shows: text wider than the
    /// panel, and coordinates on children a layout container will ignore.
    /// </summary>
    private static string Warnings(UiElement root)
    {
        var sb = new StringBuilder();
        int panelW = root.Width > 0 ? root.Width : 160;
        Walk(root, root.Kind == "canvas");
        return sb.ToString();

        void Walk(UiElement e, bool inCanvas)
        {
            if (e.Text.Length > 0 && (e.Kind == "label" || e.Kind == "textblock"))
            {
                int px = e.Text.Length * 8 * (e.Scale < 1 ? 1 : e.Scale);
                if (px > panelW)
                {
                    sb.AppendLine($"- {Name(e)}: \"{e.Text}\" needs {px}px at scale {e.Scale}; the panel is {panelW}px. "
                        + "Shorten it or drop the scale.");
                }
            }
            if (!inCanvas && (e.X != 0 || e.Y != 0) && e != root)
            {
                sb.AppendLine($"- {Name(e)}: x/y is set but the parent is not a canvas, so it is ignored.");
            }
            bool childrenAreFree = e.Kind == "canvas";
            foreach (UiElement c in e.Children)
            {
                Walk(c, childrenAreFree);
            }
        }

        static string Name(UiElement e) => e.Id.Length > 0 ? $"{e.Kind} #{e.Id}" : e.Kind;
    }
}
