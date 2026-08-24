namespace RustNet.Designer.Assistant;

/// <summary>
/// What the assistant is allowed to do to the Designer. The window implements
/// it and marshals to the UI thread; the plugins only see this surface, so a
/// tool call can never reach further into the editor than these operations.
/// </summary>
public interface IDesignerBridge
{
    /// <summary>The layout currently on the canvas, as RustNet.UI XML.</summary>
    string GetLayoutXml();

    /// <summary>
    /// Replace the canvas contents. Throws when the XML does not parse, so the
    /// model gets the parser's complaint back as the function result.
    /// </summary>
    void ApplyLayoutXml(string xml);

    /// <summary>Root window size in pixels — what the model should design for.</summary>
    (int Width, int Height) GetPanelSize();

    /// <summary>The selected element as "kind #id at x,y 80x24", or "none".</summary>
    string DescribeSelection();

    /// <summary>Put code in the editor's code tab and focus it.</summary>
    void SetGeneratedCode(string fileName, string language, string code);

    /// <summary>Whatever is in the code tab now.</summary>
    string GetGeneratedCode();
}
