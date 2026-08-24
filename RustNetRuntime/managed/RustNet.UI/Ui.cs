using System.Collections.Generic;
using RustNet.Graphics;
using RustNet.Serialization;

namespace RustNet.UI;

/// <summary>RGB565 colors for UI markup.</summary>
public static class UiColors
{
    public const int Black = 0x0000;
    public const int White = 0xFFFF;
    public const int Red = 0xF800;
    public const int Green = 0x07E0;
    public const int Blue = 0x001F;
    public const int Yellow = 0xFFE0;
    public const int Cyan = 0x07FF;
    public const int Gray = 0x8410;
    public const int DarkGray = 0x4208;
    public const int LightGray = 0xC618;
    public const int Accent = 0x05BF; // blue-cyan highlight
}

/// <summary>
/// One node of the UI tree. A single concrete class carries every control
/// kind (uniform tree — convenient for the XML format and the designer).
///
/// Containers: window, stack, panel, border, canvas, grid.
/// Controls: label, button, textblock, textbox, checkbox, radio, slider,
/// progress, listbox, image, rect.
///
/// Layout is two-pass: <see cref="Measure"/> gives a desired height at a
/// width, then <see cref="Arrange"/> assigns absolute bounds (LayoutX/Y/W/H)
/// used by both <see cref="Render"/> and hit-testing.
/// </summary>
public class UiElement
{
    public string Kind = "label";
    public string Id = "";
    public string Text = "";

    /// <summary>Explicit size; 0 = auto.</summary>
    public int Width;
    public int Height;

    /// <summary>Absolute position inside a canvas parent.</summary>
    public int X;
    public int Y;

    public int Foreground = UiColors.White;
    public int Background = UiColors.Black;
    public int Border = UiColors.Gray;
    public int Scale = 1;

    /// <summary>Slider/progress value in [Min, Max].</summary>
    public int Value;
    public int Min;
    public int Max = 100;

    /// <summary>Checkbox/radio state; radios share a <see cref="Group"/>.</summary>
    public bool Checked;
    public string Group = "";

    /// <summary>Listbox rows + selected index (-1 = none).</summary>
    public List<string> Items = new List<string>();
    public int Selected = -1;

    /// <summary>Stack/grid layout knobs.</summary>
    public bool Horizontal;
    public int Columns = 1;
    public int Padding = 2;
    public int Gap = 2;

    /// <summary>ScrollViewer vertical scroll position in pixels (clamped to
    /// content on layout).</summary>
    public int ScrollOffset;

    /// <summary>Which edge a child takes in a <c>dockpanel</c>:
    /// "left", "top", "right", "bottom", or "" to fill what is left.</summary>
    public string Dock = "";

    /// <summary>Second endpoint, for <c>line</c>. Relative to the element's
    /// own origin like <see cref="X"/>, so a line moves with its parent.</summary>
    public int X2;
    public int Y2;

    /// <summary>Data for <c>chart</c>, in the order it is plotted.</summary>
    public List<int> Series = new List<int>();

    /// <summary>Vertices for <c>polygon</c>, as x,y pairs laid end to end.
    /// A flat list rather than a point type: the interpreter erases generics,
    /// and two ints cost less than an object per vertex.</summary>
    public List<int> Points = new List<int>();

    /// <summary>Month shown by <c>calendar</c>. <see cref="Value"/> is the
    /// selected day, or 0 for none.</summary>
    public int Year = 2026;
    public int Month = 1;

    public List<UiElement> Children = new List<UiElement>();

    // Assigned by Arrange; read by Render and HitTest.
    public int LayoutX;
    public int LayoutY;
    public int LayoutW;
    public int LayoutH;

    /// <summary>ScrollViewer content height, computed during Arrange.</summary>
    public int ContentH;

    /// <summary>Pixel width of one character. The device font is an 8x8
    /// cell and the renderer advances by exactly that, so a caller can lay
    /// text out without asking the device anything.</summary>
    public static int CharWidth(int scale)
    {
        return 8 * (scale > 0 ? scale : 1);
    }

    /// <summary>Pixel height of one line, same font.</summary>
    public static int LineHeight(int scale)
    {
        return 8 * (scale > 0 ? scale : 1);
    }

    /// <summary>
    /// Break <paramref name="text"/> into lines that fit <paramref name="width"/>.
    /// </summary>
    /// <remarks>
    /// Words first, characters only when a single word cannot fit — the
    /// alternative, breaking mid-word whenever the line runs out, turns a
    /// sensor label into confetti on a 320-pixel screen.
    /// </remarks>
    public static List<string> WrapText(string text, int width, int scale)
    {
        List<string> lines = new List<string>();
        int cw = CharWidth(scale);
        int perLine = width / (cw > 0 ? cw : 1);
        if (perLine < 1)
        {
            perLine = 1;
        }
        string[] words = text.Split(' ');
        string line = "";
        for (int i = 0; i < words.Length; i++)
        {
            string word = words[i];
            while (word.Length > perLine)
            {
                if (line.Length > 0)
                {
                    lines.Add(line);
                    line = "";
                }
                lines.Add(word.Substring(0, perLine));
                word = word.Substring(perLine);
            }
            string candidate = line.Length == 0 ? word : line + " " + word;
            if (candidate.Length > perLine)
            {
                lines.Add(line);
                line = word;
            }
            else
            {
                line = candidate;
            }
        }
        if (line.Length > 0 || lines.Count == 0)
        {
            lines.Add(line);
        }
        return lines;
    }

    public static UiElement Make(string kind)
    {
        UiElement e = new UiElement();
        e.Kind = kind;
        return e;
    }

    public static UiElement Label(string text)
    {
        UiElement e = Make("label");
        e.Text = text;
        return e;
    }

    public UiElement Add(UiElement child)
    {
        Children.Add(child);
        return this;
    }

    /// <summary>Depth-first search by id ("" never matches).</summary>
    public UiElement FindById(string id)
    {
        if (Id == id && id != "")
        {
            return this;
        }
        for (int i = 0; i < Children.Count; i++)
        {
            UiElement hit = Children[i].FindById(id);
            if (hit != null)
            {
                return hit;
            }
        }
        return null;
    }

    private bool IsContainer()
    {
        return Kind == "window" || Kind == "stack" || Kind == "panel"
            || Kind == "border" || Kind == "canvas" || Kind == "grid"
            || Kind == "scrollviewer" || Kind == "dockpanel"
            || Kind == "groupbox" || Kind == "expander"
            || Kind == "tabcontrol" || Kind == "tabitem"
            || Kind == "treeview" || Kind == "messagebox";
    }

    /// <summary>Desired content height at the given width.</summary>
    public int Measure(int width)
    {
        if (Height > 0)
        {
            return Height;
        }
        if (Kind == "label" || Kind == "textblock")
        {
            return 8 * Scale + 2;
        }
        if (Kind == "button" || Kind == "textbox")
        {
            return 8 * Scale + 8;
        }
        if (Kind == "checkbox" || Kind == "radio")
        {
            return 12;
        }
        if (Kind == "combobox")
        {
            return LineHeight(Scale) + 8;
        }
        if (Kind == "textflow")
        {
            List<string> lines = WrapText(Text, width - Padding * 2, Scale);
            return lines.Count * (LineHeight(Scale) + 2) + Padding * 2;
        }
        if (Kind == "gauge")
        {
            // Half again as wide as it is tall would waste the panel; a gauge
            // is read at a glance, so it gets a square-ish default.
            return 64;
        }
        if (Kind == "chart")
        {
            return 56;
        }
        if (Kind == "datagrid")
        {
            int rowH = LineHeight(Scale) + 4;
            return (Items.Count + 1) * rowH + 2;
        }
        if (Kind == "treeview")
        {
            return CountNodes() * (LineHeight(Scale) + 4) + Padding * 2;
        }
        if (Kind == "calendar")
        {
            // A weekday header plus six week rows: every month fits six rows,
            // and a grid that changes height as the user pages through months
            // makes the panel under it jump.
            return 7 * (LineHeight(Scale) + 4) + Padding * 2;
        }
        if (Kind == "ellipse" || Kind == "line" || Kind == "polygon")
        {
            return Height > 0 ? Height : 24;
        }
        if (Kind == "groupbox")
        {
            int boxed = Padding * 2 + LineHeight(Scale) + 4;
            for (int i = 0; i < Children.Count; i++)
            {
                if (i > 0)
                {
                    boxed = boxed + Gap;
                }
                boxed = boxed + Children[i].Measure(width - Padding * 2);
            }
            return boxed;
        }
        if (Kind == "expander")
        {
            int header = LineHeight(Scale) + 8;
            if (!Checked)
            {
                return header;
            }
            int body = Padding;
            for (int i = 0; i < Children.Count; i++)
            {
                body = body + Children[i].Measure(width - Padding * 2) + Gap;
            }
            return header + body;
        }
        if (Kind == "tabcontrol")
        {
            int strip = LineHeight(Scale) + 8;
            UiElement page = SelectedPage();
            int body = page == null ? 0 : page.Measure(width);
            return strip + body + Padding * 2;
        }
        if (Kind == "messagebox")
        {
            // An overlay covers whatever it is over; its height is the
            // screen's, not its content's.
            return Height > 0 ? Height : 100;
        }
        if (Kind == "dockpanel")
        {
            return MeasureDock(width);
        }
        if (Kind == "slider")
        {
            return 12;
        }
        if (Kind == "progress")
        {
            return 10;
        }
        if (Kind == "scrollviewer")
        {
            // A viewport needs an explicit height (Height>0 returned above);
            // otherwise fall back to a sensible default.
            return 40;
        }
        if (Kind == "rect" || Kind == "image")
        {
            return 12;
        }
        if (Kind == "listbox")
        {
            int rows = Items.Count > 0 ? Items.Count : 1;
            return rows * (8 * Scale + 4) + 4;
        }
        if (Kind == "canvas")
        {
            // Canvas height = the farthest child bottom.
            int bottom = 0;
            for (int i = 0; i < Children.Count; i++)
            {
                UiElement c = Children[i];
                int cb = c.Y + c.Measure(width);
                if (cb > bottom)
                {
                    bottom = cb;
                }
            }
            return bottom + Padding * 2;
        }
        if (Kind == "grid")
        {
            int cols = Columns > 0 ? Columns : 1;
            int cellW = (width - Padding * 2 - Gap * (cols - 1)) / cols;
            int rowH = 0;
            int total = Padding * 2;
            for (int i = 0; i < Children.Count; i++)
            {
                int h = Children[i].Measure(cellW);
                if (h > rowH)
                {
                    rowH = h;
                }
                if ((i + 1) % cols == 0)
                {
                    total = total + rowH + Gap;
                    rowH = 0;
                }
            }
            if (rowH > 0)
            {
                total = total + rowH;
            }
            return total;
        }
        // window / stack / panel / border
        int inner = width - Padding * 2;
        if (Horizontal)
        {
            int maxH = 0;
            for (int i = 0; i < Children.Count; i++)
            {
                int h = Children[i].Measure(inner);
                if (h > maxH)
                {
                    maxH = h;
                }
            }
            return maxH + Padding * 2;
        }
        int sum = Padding * 2;
        for (int i = 0; i < Children.Count; i++)
        {
            sum = sum + Children[i].Measure(inner);
            if (i > 0)
            {
                sum = sum + Gap;
            }
        }
        return sum;
    }

    /// <summary>The tab page a <c>tabcontrol</c> is showing, or null.</summary>
    public UiElement SelectedPage()
    {
        if (Children.Count == 0)
        {
            return null;
        }
        int i = Selected;
        if (i < 0 || i >= Children.Count)
        {
            i = 0;
        }
        return Children[i];
    }

    /// <summary>Visible rows in a <c>treeview</c>: a collapsed node hides its
    /// subtree but is itself still a row.</summary>
    private int CountNodes()
    {
        int rows = 0;
        for (int i = 0; i < Children.Count; i++)
        {
            rows = rows + 1;
            if (Children[i].Checked)
            {
                rows = rows + Children[i].CountNodes();
            }
        }
        return rows > 0 ? rows : 1;
    }

    /// <summary>
    /// Height a <c>dockpanel</c> needs: the docked edges stack inward and the
    /// undocked child takes what is left.
    /// </summary>
    private int MeasureDock(int width)
    {
        int inner = width - Padding * 2;
        int stacked = 0;
        int fill = 0;
        for (int i = 0; i < Children.Count; i++)
        {
            UiElement c = Children[i];
            if (c.Dock == "top" || c.Dock == "bottom")
            {
                stacked = stacked + c.Measure(inner);
            }
            else if (c.Dock != "left" && c.Dock != "right")
            {
                int h = c.Measure(inner);
                if (h > fill)
                {
                    fill = h;
                }
            }
            else
            {
                int h = c.Measure(c.Width > 0 ? c.Width : inner);
                if (h > fill)
                {
                    fill = h;
                }
            }
        }
        return stacked + fill + Padding * 2;
    }

    /// <summary>Assign absolute bounds to this element and its subtree.</summary>
    public void Arrange(int x, int y, int width, int height)
    {
        LayoutX = x;
        LayoutY = y;
        LayoutW = Width > 0 ? Width : width;
        LayoutH = Height > 0 ? Height : height;

        if (!IsContainer())
        {
            return;
        }

        int innerX = x + Padding;
        int innerY = y + Padding;
        int innerW = LayoutW - Padding * 2;

        if (Kind == "canvas")
        {
            for (int i = 0; i < Children.Count; i++)
            {
                UiElement c = Children[i];
                int cw = c.Width > 0 ? c.Width : innerW;
                c.Arrange(innerX + c.X, innerY + c.Y, cw, c.Measure(cw));
            }
            return;
        }

        if (Kind == "scrollviewer")
        {
            // Stack children at full content height, offset by the (clamped)
            // scroll position. Rendering clips to the viewport.
            int contentH = 0;
            for (int i = 0; i < Children.Count; i++)
            {
                if (i > 0)
                {
                    contentH = contentH + Gap;
                }
                contentH = contentH + Children[i].Measure(innerW);
            }
            ContentH = contentH;
            int viewInner = LayoutH - Padding * 2;
            int maxOff = contentH - viewInner;
            if (maxOff < 0)
            {
                maxOff = 0;
            }
            if (ScrollOffset < 0)
            {
                ScrollOffset = 0;
            }
            if (ScrollOffset > maxOff)
            {
                ScrollOffset = maxOff;
            }
            int sy = innerY - ScrollOffset;
            for (int i = 0; i < Children.Count; i++)
            {
                UiElement c = Children[i];
                int ch = c.Measure(innerW);
                c.Arrange(innerX, sy, innerW, ch);
                sy = sy + ch + Gap;
            }
            return;
        }

        if (Kind == "dockpanel")
        {
            // Docked edges are taken in declaration order and eat into the
            // remaining rectangle, so a top strip declared before a left rail
            // spans the full width and the rail starts below it. That order
            // dependence is the point of a dock panel, not a quirk of it.
            int dx = innerX;
            int dy = innerY;
            int dw = innerW;
            int dh = LayoutH - Padding * 2;
            for (int i = 0; i < Children.Count; i++)
            {
                UiElement c = Children[i];
                if (c.Dock == "top")
                {
                    int ch = c.Height > 0 ? c.Height : c.Measure(dw);
                    c.Arrange(dx, dy, dw, ch);
                    dy = dy + ch + Gap;
                    dh = dh - ch - Gap;
                }
                else if (c.Dock == "bottom")
                {
                    int ch = c.Height > 0 ? c.Height : c.Measure(dw);
                    c.Arrange(dx, dy + dh - ch, dw, ch);
                    dh = dh - ch - Gap;
                }
                else if (c.Dock == "left")
                {
                    int cw = c.Width > 0 ? c.Width : dw / 3;
                    c.Arrange(dx, dy, cw, dh);
                    dx = dx + cw + Gap;
                    dw = dw - cw - Gap;
                }
                else if (c.Dock == "right")
                {
                    int cw = c.Width > 0 ? c.Width : dw / 3;
                    c.Arrange(dx + dw - cw, dy, cw, dh);
                    dw = dw - cw - Gap;
                }
                else
                {
                    c.Arrange(dx, dy, dw, dh);
                }
            }
            return;
        }

        if (Kind == "groupbox")
        {
            // The title sits on the frame, so the content starts below it.
            int gy = innerY + LineHeight(Scale) + 4;
            for (int i = 0; i < Children.Count; i++)
            {
                UiElement c = Children[i];
                int ch = c.Measure(innerW);
                c.Arrange(innerX, gy, innerW, ch);
                gy = gy + ch + Gap;
            }
            return;
        }

        if (Kind == "expander")
        {
            if (!Checked)
            {
                // Collapsed: children keep their last bounds but are not
                // drawn or hit-tested, so nothing below them shifts.
                return;
            }
            int ey = innerY + LineHeight(Scale) + 8;
            for (int i = 0; i < Children.Count; i++)
            {
                UiElement c = Children[i];
                int ch = c.Measure(innerW);
                c.Arrange(innerX, ey, innerW, ch);
                ey = ey + ch + Gap;
            }
            return;
        }

        if (Kind == "tabcontrol")
        {
            int strip = LineHeight(Scale) + 8;
            UiElement page = SelectedPage();
            if (page != null)
            {
                page.Arrange(innerX, innerY + strip, innerW,
                    LayoutH - Padding * 2 - strip);
            }
            return;
        }

        if (Kind == "treeview")
        {
            int ty = innerY;
            ty = ArrangeNodes(innerX, ty, innerW, 0);
            return;
        }

        if (Kind == "messagebox")
        {
            // Centred over whatever it covers, at a readable share of it.
            int boxW = innerW * 3 / 4;
            int boxH = LayoutH / 3;
            int bx = x + (LayoutW - boxW) / 2;
            int by = y + (LayoutH - boxH) / 2;
            int cy = by + Padding + LineHeight(Scale) * 2;
            for (int i = 0; i < Children.Count; i++)
            {
                UiElement c = Children[i];
                int ch = c.Measure(boxW - Padding * 2);
                c.Arrange(bx + Padding, cy, boxW - Padding * 2, ch);
                cy = cy + ch + Gap;
            }
            return;
        }

        if (Kind == "grid")
        {
            int cols = Columns > 0 ? Columns : 1;
            int cellW = (innerW - Gap * (cols - 1)) / cols;
            int cx = innerX;
            int cy = innerY;
            int rowH = 0;
            for (int i = 0; i < Children.Count; i++)
            {
                UiElement c = Children[i];
                int ch = c.Measure(cellW);
                c.Arrange(cx, cy, cellW, ch);
                if (ch > rowH)
                {
                    rowH = ch;
                }
                if ((i + 1) % cols == 0)
                {
                    cx = innerX;
                    cy = cy + rowH + Gap;
                    rowH = 0;
                }
                else
                {
                    cx = cx + cellW + Gap;
                }
            }
            return;
        }

        // stack / window / panel / border
        int px = innerX;
        int py = innerY;
        for (int i = 0; i < Children.Count; i++)
        {
            UiElement c = Children[i];
            if (Horizontal)
            {
                int cw = c.Width > 0 ? c.Width : 40;
                c.Arrange(px, py, cw, c.Measure(cw));
                px = px + cw + Gap;
            }
            else
            {
                int ch = c.Measure(innerW);
                c.Arrange(px, py, innerW, ch);
                py = py + ch + Gap;
            }
        }
    }

    /// <summary>Lay out tree nodes, indenting each level. Returns the next
    /// free y.</summary>
    private int ArrangeNodes(int x, int y, int width, int depth)
    {
        int rowH = LineHeight(Scale) + 4;
        int indent = 10;
        for (int i = 0; i < Children.Count; i++)
        {
            UiElement c = Children[i];
            c.Arrange(x + depth * indent, y, width - depth * indent, rowH);
            y = y + rowH;
            if (c.Checked)
            {
                y = c.ArrangeNodes(x, y, width, depth + 1);
            }
        }
        return y;
    }

    /// <summary>Draw using the bounds assigned by <see cref="Arrange"/>.</summary>
    public void Render()
    {
        int x = LayoutX, y = LayoutY, w = LayoutW, h = LayoutH;

        if (Kind == "label" || Kind == "textblock")
        {
            Display.DrawText(x, y, Text, Foreground, Scale);
        }
        else if (Kind == "button")
        {
            Display.FillRect(x, y, w, h, Background);
            Display.DrawRect(x, y, w, h, Border);
            Display.DrawText(x + 4, y + 4, Text, Foreground, Scale);
        }
        else if (Kind == "textbox")
        {
            Display.FillRect(x, y, w, h, UiColors.Black);
            Display.DrawRect(x, y, w, h, Border);
            Display.DrawText(x + 3, y + 4, Text, Foreground, Scale);
        }
        else if (Kind == "checkbox")
        {
            Display.DrawRect(x, y + 1, 10, 10, Border);
            if (Checked)
            {
                Display.DrawLine(x + 2, y + 6, x + 4, y + 8, Foreground);
                Display.DrawLine(x + 4, y + 8, x + 8, y + 2, Foreground);
            }
            Display.DrawText(x + 14, y + 2, Text, Foreground, Scale);
        }
        else if (Kind == "radio")
        {
            Display.DrawRect(x, y + 1, 10, 10, Border);
            if (Checked)
            {
                Display.FillRect(x + 3, y + 4, 4, 4, Foreground);
            }
            Display.DrawText(x + 14, y + 2, Text, Foreground, Scale);
        }
        else if (Kind == "slider")
        {
            int midY = y + h / 2;
            Display.DrawLine(x, midY, x + w - 1, midY, Border);
            int span = Max > Min ? Max - Min : 1;
            int knob = x + (w - 6) * (Value - Min) / span;
            Display.FillRect(knob, y + 2, 6, h - 4, Foreground);
        }
        else if (Kind == "progress")
        {
            Display.FillRect(x, y, w, h, UiColors.DarkGray);
            int span = Max > Min ? Max - Min : 1;
            int fill = w * (Value - Min) / span;
            Display.FillRect(x, y, fill, h, Foreground);
        }
        else if (Kind == "listbox")
        {
            Display.FillRect(x, y, w, h, UiColors.Black);
            Display.DrawRect(x, y, w, h, Border);
            int rowH = 8 * Scale + 4;
            for (int i = 0; i < Items.Count; i++)
            {
                int ry = y + 2 + i * rowH;
                if (i == Selected)
                {
                    Display.FillRect(x + 1, ry, w - 2, rowH, UiColors.Accent);
                }
                Display.DrawText(x + 3, ry + 2, Items[i], Foreground, Scale);
            }
        }
        else if (Kind == "combobox")
        {
            // Closed, always. A device with one touch point and no popup
            // layer has nowhere to put a dropdown that would not cover the
            // thing being configured, so a tap advances the selection and the
            // caret says there is more than one.
            Display.FillRect(x, y, w, h, UiColors.Black);
            Display.DrawRect(x, y, w, h, Border);
            string shown = Selected >= 0 && Selected < Items.Count ? Items[Selected] : "";
            Display.DrawText(x + 4, y + 4, shown, Foreground, Scale);
            int cx = x + w - 12;
            int cy = y + h / 2 - 1;
            Display.DrawLine(cx, cy, cx + 4, cy + 4, Foreground);
            Display.DrawLine(cx + 4, cy + 4, cx + 8, cy, Foreground);
        }
        else if (Kind == "textflow")
        {
            List<string> lines = WrapText(Text, w - Padding * 2, Scale);
            int ly = y + Padding;
            for (int i = 0; i < lines.Count; i++)
            {
                Display.DrawText(x + Padding, ly, lines[i], Foreground, Scale);
                ly = ly + LineHeight(Scale) + 2;
            }
        }
        else if (Kind == "gauge")
        {
            RenderGauge(x, y, w, h);
        }
        else if (Kind == "chart")
        {
            RenderChart(x, y, w, h);
        }
        else if (Kind == "datagrid")
        {
            RenderDataGrid(x, y, w, h);
        }
        else if (Kind == "calendar")
        {
            RenderCalendar(x, y, w, h);
        }
        else if (Kind == "ellipse")
        {
            int rx = w / 2;
            int ry = h / 2;
            int r = rx < ry ? rx : ry;
            if (Background != UiColors.Black)
            {
                Display.FillCircle(x + rx, y + ry, r, Background);
            }
            Display.DrawCircle(x + rx, y + ry, r, Border);
        }
        else if (Kind == "line")
        {
            Display.DrawLine(x, y, x + (X2 - X), y + (Y2 - Y), Foreground);
        }
        else if (Kind == "polygon")
        {
            // Closed by construction: the last vertex joins the first, which
            // is what separates a polygon from a polyline.
            int count = Points.Count / 2;
            for (int i = 0; i < count; i++)
            {
                int j = (i + 1) % count;
                Display.DrawLine(x + Points[i * 2], y + Points[i * 2 + 1],
                    x + Points[j * 2], y + Points[j * 2 + 1], Foreground);
            }
        }
        else if (Kind == "image")
        {
            Display.FillRect(x, y, w, h, Background);
            Display.DrawRect(x, y, w, h, Border);
        }
        else if (Kind == "rect")
        {
            Display.FillRect(x, y, w, h, Background);
        }
        else if (Kind == "scrollviewer")
        {
            Display.FillRect(x, y, w, h, Background);
            // Clip children to the viewport so scrolled content can't overdraw.
            Display.SetClip(x, y, w, h);
            for (int i = 0; i < Children.Count; i++)
            {
                Children[i].Render();
            }
            Display.ClearClip();
            // Scrollbar thumb on the right edge when content overflows.
            int viewInner = h - Padding * 2;
            if (ContentH > viewInner && ContentH > 0)
            {
                int thumbH = viewInner * viewInner / ContentH;
                if (thumbH < 4)
                {
                    thumbH = 4;
                }
                int range = ContentH - viewInner;
                int thumbY = y + Padding;
                if (range > 0)
                {
                    thumbY = thumbY + ScrollOffset * (viewInner - thumbH) / range;
                }
                Display.FillRect(x + w - 3, y + Padding, 2, viewInner, UiColors.DarkGray);
                Display.FillRect(x + w - 3, thumbY, 2, thumbH, UiColors.Accent);
            }
        }
        else if (Kind == "groupbox")
        {
            int top = y + LineHeight(Scale) / 2;
            Display.FillRect(x, y, w, h, Background);
            Display.DrawRect(x, top, w, h - (top - y), Border);
            // The title interrupts the frame rather than sitting inside it,
            // so a group reads as one thing even when packed against another.
            int labelW = Text.Length * CharWidth(Scale) + 8;
            Display.FillRect(x + 8, y, labelW, LineHeight(Scale), Background);
            Display.DrawText(x + 12, y, Text, Foreground, Scale);
            for (int i = 0; i < Children.Count; i++)
            {
                Children[i].Render();
            }
        }
        else if (Kind == "expander")
        {
            int headerH = LineHeight(Scale) + 8;
            Display.FillRect(x, y, w, headerH, Background);
            Display.DrawRect(x, y, w, headerH, Border);
            Display.DrawText(x + 6, y + 4, Checked ? "-" : "+", UiColors.Accent, Scale);
            Display.DrawText(x + 6 + CharWidth(Scale) + 4, y + 4, Text, Foreground, Scale);
            if (Checked)
            {
                for (int i = 0; i < Children.Count; i++)
                {
                    Children[i].Render();
                }
            }
        }
        else if (Kind == "tabcontrol")
        {
            int strip = LineHeight(Scale) + 8;
            Display.FillRect(x, y, w, h, Background);
            int tx = x;
            for (int i = 0; i < Children.Count; i++)
            {
                string header = Children[i].Text;
                int tw = header.Length * CharWidth(Scale) + 12;
                bool active = i == (Selected < 0 ? 0 : Selected);
                Display.FillRect(tx, y, tw, strip, active ? UiColors.Accent : UiColors.DarkGray);
                Display.DrawText(tx + 6, y + 4, header, Foreground, Scale);
                tx = tx + tw + 2;
            }
            Display.DrawLine(x, y + strip, x + w - 1, y + strip, Border);
            UiElement page = SelectedPage();
            if (page != null)
            {
                page.Render();
            }
        }
        else if (Kind == "tabitem")
        {
            for (int i = 0; i < Children.Count; i++)
            {
                Children[i].Render();
            }
        }
        else if (Kind == "treeview")
        {
            RenderNodes();
        }
        else if (Kind == "messagebox")
        {
            // No dimming: the framebuffer has no alpha channel, and filling
            // the screen with a flat colour to fake one would erase the very
            // thing the message is usually about. A shadow and a bright edge
            // lift the box instead.
            int boxW = w * 3 / 4;
            int boxH = h / 3;
            int bx = x + (w - boxW) / 2;
            int by = y + (h - boxH) / 2;
            Display.FillRect(bx + 3, by + 3, boxW, boxH, UiColors.Black);
            Display.FillRect(bx, by, boxW, boxH, Background);
            Display.DrawRect(bx, by, boxW, boxH, UiColors.Accent);
            List<string> lines = WrapText(Text, boxW - Padding * 2, Scale);
            int ly = by + Padding;
            for (int i = 0; i < lines.Count; i++)
            {
                Display.DrawText(bx + Padding, ly, lines[i], Foreground, Scale);
                ly = ly + LineHeight(Scale) + 2;
            }
            for (int i = 0; i < Children.Count; i++)
            {
                Children[i].Render();
            }
        }
        else
        {
            // Containers: border/panel paint a frame; then children.
            if (Background != UiColors.Black || Kind == "border" || Kind == "panel")
            {
                Display.FillRect(x, y, w, h, Background);
            }
            if (Kind == "border")
            {
                Display.DrawRect(x, y, w, h, Border);
            }
            for (int i = 0; i < Children.Count; i++)
            {
                Children[i].Render();
            }
        }
    }

    /// <summary>
    /// A 240-degree arc with a needle, the way a panel meter reads.
    /// </summary>
    /// <remarks>
    /// The arc is drawn as straight segments because the device has no arc
    /// primitive and a per-pixel curve in managed code would cost more than
    /// the rest of the screen put together — a host call is around 220 µs on
    /// a K210. Sixteen segments is smooth enough at gauge sizes and costs
    /// about three milliseconds.
    /// </remarks>
    private void RenderGauge(int x, int y, int w, int h)
    {
        int cx = x + w / 2;
        int cy = y + h - 6;
        int r = (w < h * 2 ? w : h * 2) / 2 - 4;
        if (r < 6)
        {
            r = 6;
        }
        int segments = 16;
        double start = 3.14159265 * 5.0 / 6.0;
        double sweep = 3.14159265 * 4.0 / 3.0;
        int span = Max > Min ? Max - Min : 1;
        int filled = segments * (Value - Min) / span;
        for (int i = 0; i < segments; i++)
        {
            double a0 = start + sweep * i / segments;
            double a1 = start + sweep * (i + 1) / segments;
            int x0 = cx + (int)(Math.Cos(a0) * r);
            int y0 = cy - (int)(Math.Sin(a0) * r);
            int x1 = cx + (int)(Math.Cos(a1) * r);
            int y1 = cy - (int)(Math.Sin(a1) * r);
            Display.DrawLine(x0, y0, x1, y1, i < filled ? Foreground : UiColors.DarkGray);
        }
        double na = start + sweep * (Value - Min) / span;
        Display.DrawLine(cx, cy, cx + (int)(Math.Cos(na) * (r - 4)),
            cy - (int)(Math.Sin(na) * (r - 4)), UiColors.Accent);
        string label = Value.ToString();
        Display.DrawText(cx - label.Length * CharWidth(Scale) / 2, cy - 10, label,
            Foreground, Scale);
    }

    /// <summary>
    /// The series as a line, or as bars when <see cref="Horizontal"/> is set.
    /// </summary>
    /// <remarks>
    /// Scaled to the series' own range rather than to Min/Max, because the
    /// point of a sensor trace is the shape of the change: a temperature that
    /// moves between 21 and 23 degrees is a flat line against a 0..100 axis
    /// and a legible curve against its own.
    /// </remarks>
    private void RenderChart(int x, int y, int w, int h)
    {
        Display.FillRect(x, y, w, h, Background);
        Display.DrawRect(x, y, w, h, Border);
        if (Series.Count == 0)
        {
            return;
        }
        int lo = Series[0];
        int hi = Series[0];
        for (int i = 1; i < Series.Count; i++)
        {
            if (Series[i] < lo) lo = Series[i];
            if (Series[i] > hi) hi = Series[i];
        }
        int range = hi - lo;
        if (range == 0)
        {
            range = 1;
        }
        int plotX = x + 2;
        int plotY = y + 2;
        int plotW = w - 4;
        int plotH = h - 4;
        if (Horizontal)
        {
            int barW = plotW / Series.Count;
            if (barW < 1)
            {
                barW = 1;
            }
            for (int i = 0; i < Series.Count; i++)
            {
                int bh = plotH * (Series[i] - lo) / range;
                Display.FillRect(plotX + i * barW, plotY + plotH - bh,
                    barW > 1 ? barW - 1 : 1, bh, Foreground);
            }
            return;
        }
        int prevX = plotX;
        int prevY = plotY + plotH - plotH * (Series[0] - lo) / range;
        for (int i = 1; i < Series.Count; i++)
        {
            int px = plotX + plotW * i / (Series.Count - 1 > 0 ? Series.Count - 1 : 1);
            int py = plotY + plotH - plotH * (Series[i] - lo) / range;
            Display.DrawLine(prevX, prevY, px, py, Foreground);
            prevX = px;
            prevY = py;
        }
    }

    /// <summary>
    /// Rows of cells. Each entry in <see cref="Items"/> is one row and cells
    /// are separated by '|'; the first row is the header.
    /// </summary>
    /// <remarks>
    /// A flat list of delimited strings rather than a row/cell object graph:
    /// it survives the XML round trip as one attribute, and an application
    /// building a table from sensor readings is already formatting strings.
    /// </remarks>
    private void RenderDataGrid(int x, int y, int w, int h)
    {
        Display.FillRect(x, y, w, h, UiColors.Black);
        Display.DrawRect(x, y, w, h, Border);
        int cols = Columns > 0 ? Columns : 1;
        int colW = (w - 2) / cols;
        int rowH = LineHeight(Scale) + 4;
        for (int r = 0; r < Items.Count; r++)
        {
            int ry = y + 1 + r * rowH;
            if (ry + rowH > y + h)
            {
                break;
            }
            bool header = r == 0;
            if (header)
            {
                Display.FillRect(x + 1, ry, w - 2, rowH, UiColors.DarkGray);
            }
            else if (r == Selected)
            {
                Display.FillRect(x + 1, ry, w - 2, rowH, UiColors.Accent);
            }
            string[] cells = Items[r].Split('|');
            for (int c = 0; c < cols && c < cells.Length; c++)
            {
                Display.DrawText(x + 3 + c * colW, ry + 2, cells[c], Foreground, Scale);
            }
        }
        for (int c = 1; c < cols; c++)
        {
            Display.DrawLine(x + c * colW, y + 1, x + c * colW, y + h - 2, Border);
        }
    }

    /// <summary>A month grid, with <see cref="Value"/> as the selected day.</summary>
    /// <remarks>
    /// The weekday of the first is computed with Zeller's congruence rather
    /// than by asking the device for a DateTime: a calendar is often shown
    /// for a month the device is not currently in, and a control that needs a
    /// real-time clock to draw a grid would be unusable on a board whose RTC
    /// has never been set.
    /// </remarks>
    private void RenderCalendar(int x, int y, int w, int h)
    {
        Display.FillRect(x, y, w, h, Background);
        Display.DrawRect(x, y, w, h, Border);
        int cellW = w / 7;
        int rowH = LineHeight(Scale) + 4;
        string[] days = new string[] { "S", "M", "T", "W", "T", "F", "S" };
        for (int i = 0; i < 7; i++)
        {
            Display.DrawText(x + i * cellW + cellW / 2 - CharWidth(Scale) / 2, y + 2,
                days[i], UiColors.Gray, Scale);
        }
        int first = FirstWeekday(Year, Month);
        int count = DaysInMonth(Year, Month);
        for (int d = 1; d <= count; d++)
        {
            int index = first + d - 1;
            int col = index % 7;
            int row = index / 7;
            int cx = x + col * cellW;
            int cy = y + rowH + row * rowH;
            if (cy + rowH > y + h)
            {
                break;
            }
            if (d == Value)
            {
                Display.FillRect(cx + 1, cy, cellW - 2, rowH, UiColors.Accent);
            }
            string label = d.ToString();
            Display.DrawText(cx + cellW / 2 - label.Length * CharWidth(Scale) / 2, cy + 2,
                label, Foreground, Scale);
        }
    }

    /// <summary>Weekday of the first of the month, 0 = Sunday (Zeller).</summary>
    public static int FirstWeekday(int year, int month)
    {
        int m = month;
        int yy = year;
        if (m < 3)
        {
            m = m + 12;
            yy = yy - 1;
        }
        int k = yy % 100;
        int j = yy / 100;
        int hh = (1 + 13 * (m + 1) / 5 + k + k / 4 + j / 4 + 5 * j) % 7;
        // Zeller counts Saturday as 0; shift so Sunday is 0.
        return (hh + 6) % 7;
    }

    public static int DaysInMonth(int year, int month)
    {
        if (month == 2)
        {
            bool leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            return leap ? 29 : 28;
        }
        if (month == 4 || month == 6 || month == 9 || month == 11)
        {
            return 30;
        }
        return 31;
    }

    /// <summary>Draw tree rows, with a marker on any node that has children.</summary>
    private void RenderNodes()
    {
        for (int i = 0; i < Children.Count; i++)
        {
            UiElement c = Children[i];
            int marker = c.Children.Count > 0 ? 1 : 0;
            if (marker == 1)
            {
                Display.DrawText(c.LayoutX, c.LayoutY + 2, c.Checked ? "-" : "+",
                    UiColors.Accent, c.Scale);
            }
            Display.DrawText(c.LayoutX + CharWidth(c.Scale) + 2, c.LayoutY + 2,
                c.Text, c.Foreground, c.Scale);
            if (c.Checked)
            {
                c.RenderNodes();
            }
        }
    }

    /// <summary>Topmost element whose laid-out bounds contain (px, py).</summary>
    public UiElement HitTest(int px, int py)
    {
        // A scrollviewer clips its content: points outside the viewport can't
        // hit scrolled-away children.
        if (Kind == "scrollviewer"
            && (px < LayoutX || px >= LayoutX + LayoutW
                || py < LayoutY || py >= LayoutY + LayoutH))
        {
            return null;
        }
        // Deepest child first (last drawn wins).
        for (int i = Children.Count - 1; i >= 0; i--)
        {
            UiElement hit = Children[i].HitTest(px, py);
            if (hit != null)
            {
                return hit;
            }
        }
        if (px >= LayoutX && px < LayoutX + LayoutW
            && py >= LayoutY && py < LayoutY + LayoutH)
        {
            return this;
        }
        return null;
    }
}

/// <summary>
/// UI entry points: load a tree from XML, render to the display, and route
/// touch/pointer input.
///
/// Markup example:
/// <code>
/// &lt;window width="160" height="128" bg="0000" pad="4" gap="4"&gt;
///   &lt;label id="title" text="Thermostat" scale="2" fg="FFFF"/&gt;
///   &lt;slider id="setpoint" min="10" max="30" value="21" fg="F800"/&gt;
///   &lt;checkbox id="eco" text="Eco mode" checked="true"/&gt;
///   &lt;listbox id="zones" items="Kitchen;Garage;Attic" selected="0"/&gt;
///   &lt;button id="apply" text="Apply" bg="4208"/&gt;
/// &lt;/window&gt;
/// </code>
/// </summary>
public static class Ui
{
    private static bool _inited;
    private static int _width = 160;
    private static int _height = 128;

    /// <summary>Build a UI tree from XML markup (colors are RGB565 hex).</summary>
    public static UiElement LoadXml(string xml)
    {
        XmlNode root = Xml.Parse(xml);
        return FromNode(root);
    }

    private static UiElement FromNode(XmlNode node)
    {
        UiElement e = UiElement.Make(node.Name);
        if (node.HasAttr("id"))
        {
            e.Id = node.GetAttr("id");
        }
        e.Text = node.HasAttr("text") ? node.GetAttr("text") : node.Text;
        SetInt(node, "width", ref e.Width);
        SetInt(node, "height", ref e.Height);
        SetInt(node, "x", ref e.X);
        SetInt(node, "y", ref e.Y);
        SetInt(node, "scale", ref e.Scale);
        SetInt(node, "value", ref e.Value);
        SetInt(node, "min", ref e.Min);
        SetInt(node, "max", ref e.Max);
        SetInt(node, "pad", ref e.Padding);
        SetInt(node, "gap", ref e.Gap);
        SetInt(node, "columns", ref e.Columns);
        SetInt(node, "selected", ref e.Selected);
        SetInt(node, "scroll", ref e.ScrollOffset);
        if (node.HasAttr("fg"))
        {
            e.Foreground = ParseHex(node.GetAttr("fg"));
        }
        if (node.HasAttr("bg"))
        {
            e.Background = ParseHex(node.GetAttr("bg"));
        }
        if (node.HasAttr("border"))
        {
            e.Border = ParseHex(node.GetAttr("border"));
        }
        if (node.HasAttr("checked"))
        {
            e.Checked = node.GetAttr("checked") == "true";
        }
        if (node.HasAttr("group"))
        {
            e.Group = node.GetAttr("group");
        }
        if (node.HasAttr("orient"))
        {
            e.Horizontal = node.GetAttr("orient") == "horizontal";
        }
        SetInt(node, "x2", ref e.X2);
        SetInt(node, "y2", ref e.Y2);
        SetInt(node, "year", ref e.Year);
        SetInt(node, "month", ref e.Month);
        if (node.HasAttr("dock"))
        {
            e.Dock = node.GetAttr("dock");
        }
        if (node.HasAttr("series"))
        {
            string[] values = node.GetAttr("series").Split(',');
            for (int i = 0; i < values.Length; i++)
            {
                e.Series.Add(int.Parse(values[i].Trim()));
            }
        }
        if (node.HasAttr("points"))
        {
            string[] values = node.GetAttr("points").Split(',');
            for (int i = 0; i < values.Length; i++)
            {
                e.Points.Add(int.Parse(values[i].Trim()));
            }
        }
        if (node.HasAttr("items"))
        {
            string[] parts = node.GetAttr("items").Split(';');
            for (int i = 0; i < parts.Length; i++)
            {
                e.Items.Add(parts[i]);
            }
        }
        for (int i = 0; i < node.Children.Count; i++)
        {
            e.Add(FromNode(node.Children[i]));
        }
        return e;
    }

    private static void SetInt(XmlNode node, string attr, ref int target)
    {
        if (node.HasAttr(attr))
        {
            target = int.Parse(node.GetAttr(attr));
        }
    }

    /// <summary>Serialize a tree back to the RustNet.UI XML format
    /// (the designer's save path; round-trips with LoadXml).</summary>
    public static string ToXml(UiElement root)
    {
        System.Text.StringBuilder sb = new System.Text.StringBuilder();
        WriteNode(root, sb, 0);
        return sb.ToString();
    }

    private static void WriteNode(UiElement e, System.Text.StringBuilder sb, int depth)
    {
        for (int i = 0; i < depth; i++)
        {
            sb.Append("  ");
        }
        sb.Append('<');
        sb.Append(e.Kind);
        WriteAttr(sb, "id", e.Id, "");
        WriteAttrRaw(sb, "text", e.Text, "");
        WriteAttrInt(sb, "x", e.X, 0);
        WriteAttrInt(sb, "y", e.Y, 0);
        WriteAttrInt(sb, "width", e.Width, 0);
        WriteAttrInt(sb, "height", e.Height, 0);
        WriteAttrInt(sb, "scale", e.Scale, 1);
        WriteAttrInt(sb, "value", e.Value, 0);
        WriteAttrInt(sb, "min", e.Min, 0);
        WriteAttrInt(sb, "max", e.Max, 100);
        WriteAttrInt(sb, "columns", e.Columns, 1);
        WriteAttrInt(sb, "selected", e.Selected, -1);
        WriteAttrInt(sb, "scroll", e.ScrollOffset, 0);
        WriteAttrInt(sb, "pad", e.Padding, 2);
        WriteAttrInt(sb, "gap", e.Gap, 2);
        WriteAttr(sb, "fg", Hex(e.Foreground), Hex(UiColors.White));
        WriteAttr(sb, "bg", Hex(e.Background), Hex(UiColors.Black));
        WriteAttr(sb, "border", Hex(e.Border), Hex(UiColors.Gray));
        WriteAttr(sb, "group", e.Group, "");
        WriteAttr(sb, "dock", e.Dock, "");
        WriteAttrInt(sb, "x2", e.X2, 0);
        WriteAttrInt(sb, "y2", e.Y2, 0);
        WriteAttrInt(sb, "year", e.Year, 2026);
        WriteAttrInt(sb, "month", e.Month, 1);
        if (e.Horizontal)
        {
            sb.Append(" orient=\"horizontal\"");
        }
        if (e.Checked)
        {
            sb.Append(" checked=\"true\"");
        }
        if (e.Items.Count > 0)
        {
            sb.Append(" items=\"");
            sb.Append(string.Join(";", e.Items));
            sb.Append('"');
        }
        if (e.Series.Count > 0)
        {
            sb.Append(" series=\"");
            sb.Append(JoinInts(e.Series));
            sb.Append('"');
        }
        if (e.Points.Count > 0)
        {
            sb.Append(" points=\"");
            sb.Append(JoinInts(e.Points));
            sb.Append('"');
        }
        if (e.Children.Count == 0)
        {
            sb.Append("/>\n");
            return;
        }
        sb.Append(">\n");
        for (int i = 0; i < e.Children.Count; i++)
        {
            WriteNode(e.Children[i], sb, depth + 1);
        }
        for (int i = 0; i < depth; i++)
        {
            sb.Append("  ");
        }
        sb.Append("</");
        sb.Append(e.Kind);
        sb.Append(">\n");
    }

    private static void WriteAttr(System.Text.StringBuilder sb, string name, string val, string def)
    {
        if (val != def && val.Length > 0)
        {
            sb.Append(' ');
            sb.Append(name);
            sb.Append("=\"");
            sb.Append(val);
            sb.Append('"');
        }
    }

    private static void WriteAttrRaw(System.Text.StringBuilder sb, string name, string val, string def)
    {
        WriteAttr(sb, name, val, def);
    }

    private static void WriteAttrInt(System.Text.StringBuilder sb, string name, int val, int def)
    {
        if (val != def)
        {
            sb.Append(' ');
            sb.Append(name);
            sb.Append("=\"");
            sb.Append(val.ToString());
            sb.Append('"');
        }
    }

    /// <summary>Comma-separated integers, for the series and points
    /// attributes. Written by hand because the interpreter's LINQ subset does
    /// not cover projecting a value type to string.</summary>
    private static string JoinInts(List<int> values)
    {
        System.Text.StringBuilder sb = new System.Text.StringBuilder();
        for (int i = 0; i < values.Count; i++)
        {
            if (i > 0)
            {
                sb.Append(',');
            }
            sb.Append(values[i].ToString());
        }
        return sb.ToString();
    }

    private static string Hex(int v)
    {
        string s = "";
        for (int i = 3; i >= 0; i--)
        {
            int nib = (v >> (i * 4)) & 0xF;
            s = string.Concat(s, HexDigit(nib));
        }
        return s;
    }

    private static string HexDigit(int n)
    {
        if (n < 10)
        {
            return ((char)('0' + n)).ToString();
        }
        return ((char)('A' + (n - 10))).ToString();
    }

    private static int ParseHex(string s)
    {
        int v = 0;
        for (int i = 0; i < s.Length; i++)
        {
            char c = s[i];
            int d;
            if (c >= '0' && c <= '9')
            {
                d = c - '0';
            }
            else if (c >= 'a' && c <= 'f')
            {
                d = c - 'a' + 10;
            }
            else
            {
                d = c - 'A' + 10;
            }
            v = v * 16 + d;
        }
        return v;
    }

    /// <summary>Lay out and draw the whole tree.</summary>
    public static void Render(UiElement root)
    {
        int w = root.Width > 0 ? root.Width : _width;
        int h = root.Height > 0 ? root.Height : _height;
        if (!_inited || w != _width || h != _height)
        {
            Display.Init(w, h);
            _width = w;
            _height = h;
            _inited = true;
        }
        root.Arrange(0, 0, w, h);
        Display.Clear(root.Background);
        root.Render();
        Display.Present();
    }

    /// <summary>
    /// Route a tap/click at (px, py): updates the hit control's state
    /// (toggle checkbox, select radio, move slider, choose listbox row) and
    /// returns it, or null. Call <see cref="Render"/> afterwards to redraw.
    /// </summary>
    public static UiElement Tap(UiElement root, int px, int py)
    {
        UiElement hit = root.HitTest(px, py);
        if (hit == null)
        {
            return null;
        }
        if (hit.Kind == "checkbox")
        {
            hit.Checked = !hit.Checked;
        }
        else if (hit.Kind == "radio")
        {
            ClearGroup(root, hit.Group);
            hit.Checked = true;
        }
        else if (hit.Kind == "slider")
        {
            // Track spans (LayoutW - knobWidth), matching Render's knob math.
            int track = hit.LayoutW - 6;
            if (track < 1)
            {
                track = 1;
            }
            int rel = px - hit.LayoutX;
            if (rel < 0)
            {
                rel = 0;
            }
            if (rel > track)
            {
                rel = track;
            }
            int span = hit.Max > hit.Min ? hit.Max - hit.Min : 1;
            hit.Value = hit.Min + rel * span / track;
        }
        else if (hit.Kind == "listbox")
        {
            int rowH = 8 * hit.Scale + 4;
            int idx = (py - hit.LayoutY - 2) / rowH;
            if (idx >= 0 && idx < hit.Items.Count)
            {
                hit.Selected = idx;
            }
        }
        else if (hit.Kind == "combobox")
        {
            // Advance rather than open. See the note in Render: there is
            // nowhere to put a dropdown on a screen this size that would not
            // cover the setting being changed.
            if (hit.Items.Count > 0)
            {
                hit.Selected = (hit.Selected + 1) % hit.Items.Count;
            }
        }
        else if (hit.Kind == "expander")
        {
            // Only the header toggles. A tap inside an expanded body belongs
            // to whatever is in the body, and HitTest has already returned
            // that instead.
            if (py < hit.LayoutY + UiElement.LineHeight(hit.Scale) + 8)
            {
                hit.Checked = !hit.Checked;
            }
        }
        else if (hit.Kind == "tabcontrol")
        {
            int strip = UiElement.LineHeight(hit.Scale) + 8;
            if (py < hit.LayoutY + strip)
            {
                int tx = hit.LayoutX;
                for (int i = 0; i < hit.Children.Count; i++)
                {
                    int tw = hit.Children[i].Text.Length * UiElement.CharWidth(hit.Scale) + 12;
                    if (px >= tx && px < tx + tw)
                    {
                        hit.Selected = i;
                        break;
                    }
                    tx = tx + tw + 2;
                }
            }
        }
        else if (hit.Kind == "datagrid")
        {
            int rowH = UiElement.LineHeight(hit.Scale) + 4;
            int idx = (py - hit.LayoutY - 1) / rowH;
            // Row 0 is the header and is not selectable.
            if (idx > 0 && idx < hit.Items.Count)
            {
                hit.Selected = idx;
            }
        }
        else if (hit.Kind == "calendar")
        {
            int cellW = hit.LayoutW / 7;
            int rowH = UiElement.LineHeight(hit.Scale) + 4;
            int col = (px - hit.LayoutX) / (cellW > 0 ? cellW : 1);
            int row = (py - hit.LayoutY - rowH) / rowH;
            if (row >= 0 && col >= 0 && col < 7)
            {
                int day = row * 7 + col - UiElement.FirstWeekday(hit.Year, hit.Month) + 1;
                if (day >= 1 && day <= UiElement.DaysInMonth(hit.Year, hit.Month))
                {
                    hit.Value = day;
                }
            }
        }
        else if (hit.Kind == "treeview" || hit.Kind == "tabitem")
        {
            // Nothing: a tree's rows are its children, and HitTest returns
            // the child that was actually touched.
        }
        return hit;
    }

    /// <summary>Scroll a ScrollViewer (by id) by <paramref name="delta"/>
    /// pixels (positive = down). The offset is clamped to the content on the
    /// next <see cref="Render"/>. Returns the new (pre-clamp) offset.</summary>
    public static int Scroll(UiElement root, string id, int delta)
    {
        UiElement sv = root.FindById(id);
        if (sv == null || sv.Kind != "scrollviewer")
        {
            return 0;
        }
        sv.ScrollOffset = sv.ScrollOffset + delta;
        if (sv.ScrollOffset < 0)
        {
            sv.ScrollOffset = 0;
        }
        return sv.ScrollOffset;
    }

    private static void ClearGroup(UiElement node, string group)
    {
        if (node.Kind == "radio" && node.Group == group)
        {
            node.Checked = false;
        }
        for (int i = 0; i < node.Children.Count; i++)
        {
            ClearGroup(node.Children[i], group);
        }
    }
}
