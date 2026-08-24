using System.Collections.Generic;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Media;
using System.Windows.Shapes;
using RustNet.UI;

namespace RustNet.Designer;

/// <summary>
/// Renders a <see cref="UiElement"/> tree onto a WPF <see cref="Canvas"/>,
/// mirroring how RustNet.UI paints the device display, so the designer is
/// WYSIWYG. Uses the model's own two-pass layout (Measure/Arrange), then
/// draws each element's laid-out bounds. Records the element behind each
/// visual so clicks select the right node.
/// </summary>
public static class DesignRenderer
{
    /// <summary>Convert an RGB565 int to a WPF brush.</summary>
    public static Color FromRgb565(int v)
    {
        int r = ((v >> 11) & 0x1F) << 3;
        int g = ((v >> 5) & 0x3F) << 2;
        int b = (v & 0x1F) << 3;
        return Color.FromRgb((byte)r, (byte)g, (byte)b);
    }

    private static Brush Brush(int rgb565) => new SolidColorBrush(FromRgb565(rgb565));

    /// <summary>
    /// Lay out and draw <paramref name="root"/> onto <paramref name="canvas"/>.
    /// Returns a shape→element map for hit-selection. The canvas is sized to
    /// the root's bounds.
    /// </summary>
    public static Dictionary<FrameworkElement, UiElement> Render(Canvas canvas, UiElement root)
    {
        canvas.Children.Clear();
        var map = new Dictionary<FrameworkElement, UiElement>();

        int w = root.Width > 0 ? root.Width : 160;
        int h = root.Height > 0 ? root.Height : 128;
        canvas.Width = w;
        canvas.Height = h;
        canvas.Background = Brush(root.Background);

        root.Arrange(0, 0, w, h);
        Draw(canvas, root, map);
        return map;
    }

    private static void Draw(Canvas canvas, UiElement e, Dictionary<FrameworkElement, UiElement> map)
    {
        int x = e.LayoutX, y = e.LayoutY, w = e.LayoutW, h = e.LayoutH;

        switch (e.Kind)
        {
            case "label":
            case "textblock":
                AddText(canvas, map, e, x, y, e.Text, e.Foreground, e.Scale);
                break;

            case "button":
                AddBox(canvas, map, e, x, y, w, h, e.Background, e.Border);
                AddText(canvas, map, e, x + 4, y + 4, e.Text, e.Foreground, e.Scale);
                break;

            case "textbox":
                AddBox(canvas, map, e, x, y, w, h, UiColors.Black, e.Border);
                AddText(canvas, map, e, x + 3, y + 4, e.Text, e.Foreground, e.Scale);
                break;

            case "checkbox":
                AddBox(canvas, map, e, x, y + 1, 10, 10, UiColors.Black, e.Border);
                if (e.Checked)
                {
                    AddText(canvas, map, e, x + 1, y, "x", e.Foreground, 1);
                }
                AddText(canvas, map, e, x + 14, y + 2, e.Text, e.Foreground, e.Scale);
                break;

            case "radio":
                AddEllipse(canvas, map, e, x, y + 1, 10, 10, e.Border, e.Checked ? e.Foreground : -1);
                AddText(canvas, map, e, x + 14, y + 2, e.Text, e.Foreground, e.Scale);
                break;

            case "slider":
            {
                int midY = y + h / 2;
                AddLine(canvas, e, x, midY, x + w, midY, e.Border);
                int span = e.Max > e.Min ? e.Max - e.Min : 1;
                int knob = x + (w - 6) * (e.Value - e.Min) / span;
                AddBox(canvas, map, e, knob, y + 2, 6, h - 4, e.Foreground, e.Foreground);
                break;
            }

            case "progress":
                AddBox(canvas, map, e, x, y, w, h, UiColors.DarkGray, UiColors.DarkGray);
                int fillSpan = e.Max > e.Min ? e.Max - e.Min : 1;
                AddBox(canvas, map, e, x, y, w * (e.Value - e.Min) / fillSpan, h, e.Foreground, e.Foreground);
                break;

            case "listbox":
            {
                AddBox(canvas, map, e, x, y, w, h, UiColors.Black, e.Border);
                int rowH = 8 * e.Scale + 4;
                for (int i = 0; i < e.Items.Count; i++)
                {
                    int ry = y + 2 + i * rowH;
                    if (i == e.Selected)
                    {
                        AddFill(canvas, e, x + 1, ry, w - 2, rowH, UiColors.Accent);
                    }
                    AddText(canvas, map, e, x + 3, ry + 2, e.Items[i], e.Foreground, e.Scale);
                }
                break;
            }

            case "image":
            case "rect":
                AddBox(canvas, map, e, x, y, w, h, e.Background, e.Border);
                break;

            case "combobox":
            {
                AddBox(canvas, map, e, x, y, w, h, UiColors.Black, e.Border);
                string shown = e.Selected >= 0 && e.Selected < e.Items.Count
                    ? e.Items[e.Selected] : "";
                AddText(canvas, map, e, x + 4, y + 4, shown, e.Foreground, e.Scale);
                int cx = x + w - 12;
                int cy = y + h / 2 - 1;
                AddLine(canvas, e, cx, cy, cx + 4, cy + 4, e.Foreground);
                AddLine(canvas, e, cx + 4, cy + 4, cx + 8, cy, e.Foreground);
                break;
            }

            case "textflow":
            {
                var lines = UiElement.WrapText(e.Text, w - e.Padding * 2, e.Scale);
                int ly = y + e.Padding;
                foreach (string line in lines)
                {
                    AddText(canvas, map, e, x + e.Padding, ly, line, e.Foreground, e.Scale);
                    ly += UiElement.LineHeight(e.Scale) + 2;
                }
                break;
            }

            case "gauge":
            {
                // Segments, exactly as the device draws them — a smooth WPF
                // arc here would flatter the preview into a lie.
                int gcx = x + w / 2;
                int gcy = y + h - 6;
                int r = System.Math.Max(6, System.Math.Min(w, h * 2) / 2 - 4);
                int span = e.Max > e.Min ? e.Max - e.Min : 1;
                int filled = 16 * (e.Value - e.Min) / span;
                double start = System.Math.PI * 5.0 / 6.0;
                double sweep = System.Math.PI * 4.0 / 3.0;
                for (int i = 0; i < 16; i++)
                {
                    double a0 = start + sweep * i / 16;
                    double a1 = start + sweep * (i + 1) / 16;
                    AddLine(canvas, e,
                        gcx + (int)(System.Math.Cos(a0) * r), gcy - (int)(System.Math.Sin(a0) * r),
                        gcx + (int)(System.Math.Cos(a1) * r), gcy - (int)(System.Math.Sin(a1) * r),
                        i < filled ? e.Foreground : UiColors.DarkGray);
                }
                double na = start + sweep * (e.Value - e.Min) / span;
                AddLine(canvas, e, gcx, gcy,
                    gcx + (int)(System.Math.Cos(na) * (r - 4)),
                    gcy - (int)(System.Math.Sin(na) * (r - 4)), UiColors.Accent);
                AddText(canvas, map, e, gcx - 8, gcy - 10, e.Value.ToString(), e.Foreground, e.Scale);
                break;
            }

            case "chart":
            {
                AddBox(canvas, map, e, x, y, w, h, e.Background, e.Border);
                if (e.Series.Count == 0)
                {
                    break;
                }
                int lo = e.Series[0], hi = e.Series[0];
                foreach (int v in e.Series)
                {
                    if (v < lo) lo = v;
                    if (v > hi) hi = v;
                }
                int range = System.Math.Max(1, hi - lo);
                int px0 = x + 2, py0 = y + 2, pw = w - 4, ph = h - 4;
                if (e.Horizontal)
                {
                    int barW = System.Math.Max(1, pw / e.Series.Count);
                    for (int i = 0; i < e.Series.Count; i++)
                    {
                        int bh = ph * (e.Series[i] - lo) / range;
                        AddFill(canvas, e, px0 + i * barW, py0 + ph - bh,
                            System.Math.Max(1, barW - 1), bh, e.Foreground);
                    }
                    break;
                }
                int prevX = px0;
                int prevY = py0 + ph - ph * (e.Series[0] - lo) / range;
                for (int i = 1; i < e.Series.Count; i++)
                {
                    int cx2 = px0 + pw * i / System.Math.Max(1, e.Series.Count - 1);
                    int cy2 = py0 + ph - ph * (e.Series[i] - lo) / range;
                    AddLine(canvas, e, prevX, prevY, cx2, cy2, e.Foreground);
                    prevX = cx2;
                    prevY = cy2;
                }
                break;
            }

            case "datagrid":
            {
                AddBox(canvas, map, e, x, y, w, h, UiColors.Black, e.Border);
                int cols = System.Math.Max(1, e.Columns);
                int colW = (w - 2) / cols;
                int rowH = UiElement.LineHeight(e.Scale) + 4;
                for (int r = 0; r < e.Items.Count; r++)
                {
                    int ry = y + 1 + r * rowH;
                    if (ry + rowH > y + h) break;
                    if (r == 0)
                    {
                        AddFill(canvas, e, x + 1, ry, w - 2, rowH, UiColors.DarkGray);
                    }
                    else if (r == e.Selected)
                    {
                        AddFill(canvas, e, x + 1, ry, w - 2, rowH, UiColors.Accent);
                    }
                    string[] cells = e.Items[r].Split('|');
                    for (int c2 = 0; c2 < cols && c2 < cells.Length; c2++)
                    {
                        AddText(canvas, map, e, x + 3 + c2 * colW, ry + 2, cells[c2],
                            e.Foreground, e.Scale);
                    }
                }
                for (int c2 = 1; c2 < cols; c2++)
                {
                    AddLine(canvas, e, x + c2 * colW, y + 1, x + c2 * colW, y + h - 2, e.Border);
                }
                break;
            }

            case "calendar":
            {
                AddBox(canvas, map, e, x, y, w, h, e.Background, e.Border);
                int cellW = w / 7;
                int rowH = UiElement.LineHeight(e.Scale) + 4;
                string[] days = { "S", "M", "T", "W", "T", "F", "S" };
                for (int i = 0; i < 7; i++)
                {
                    AddText(canvas, map, e, x + i * cellW + cellW / 2 - 4, y + 2,
                        days[i], UiColors.Gray, e.Scale);
                }
                int first = UiElement.FirstWeekday(e.Year, e.Month);
                int count = UiElement.DaysInMonth(e.Year, e.Month);
                for (int d = 1; d <= count; d++)
                {
                    int index = first + d - 1;
                    int cx3 = x + index % 7 * cellW;
                    int cy3 = y + rowH + index / 7 * rowH;
                    if (cy3 + rowH > y + h) break;
                    if (d == e.Value)
                    {
                        AddFill(canvas, e, cx3 + 1, cy3, cellW - 2, rowH, UiColors.Accent);
                    }
                    AddText(canvas, map, e, cx3 + cellW / 2 - 4, cy3 + 2,
                        d.ToString(), e.Foreground, e.Scale);
                }
                break;
            }

            case "ellipse":
            {
                int r = System.Math.Min(w, h) / 2;
                AddEllipse(canvas, map, e, x + w / 2 - r, y + h / 2 - r, r * 2, r * 2,
                    e.Border, e.Background);
                break;
            }

            case "line":
                AddLine(canvas, e, x, y, x + (e.X2 - e.X), y + (e.Y2 - e.Y), e.Foreground);
                AddHitRect(canvas, map, e, x, y, System.Math.Max(4, w), System.Math.Max(4, h));
                break;

            case "polygon":
            {
                int count = e.Points.Count / 2;
                for (int i = 0; i < count; i++)
                {
                    int j = (i + 1) % count;
                    AddLine(canvas, e, x + e.Points[i * 2], y + e.Points[i * 2 + 1],
                        x + e.Points[j * 2], y + e.Points[j * 2 + 1], e.Foreground);
                }
                AddHitRect(canvas, map, e, x, y, System.Math.Max(4, w), System.Math.Max(4, h));
                break;
            }

            case "groupbox":
            {
                int top = y + UiElement.LineHeight(e.Scale) / 2;
                AddBox(canvas, map, e, x, top, w, h - (top - y), e.Background, e.Border);
                AddFill(canvas, e, x + 8, y, e.Text.Length * UiElement.CharWidth(e.Scale) + 8,
                    UiElement.LineHeight(e.Scale), e.Background);
                AddText(canvas, map, e, x + 12, y, e.Text, e.Foreground, e.Scale);
                for (int i = 0; i < e.Children.Count; i++)
                {
                    Draw(canvas, e.Children[i], map);
                }
                break;
            }

            case "expander":
            {
                int headerH = UiElement.LineHeight(e.Scale) + 8;
                AddBox(canvas, map, e, x, y, w, headerH, e.Background, e.Border);
                AddText(canvas, map, e, x + 6, y + 4, e.Checked ? "-" : "+",
                    UiColors.Accent, e.Scale);
                AddText(canvas, map, e, x + 6 + UiElement.CharWidth(e.Scale) + 4, y + 4,
                    e.Text, e.Foreground, e.Scale);
                if (e.Checked)
                {
                    for (int i = 0; i < e.Children.Count; i++)
                    {
                        Draw(canvas, e.Children[i], map);
                    }
                }
                break;
            }

            case "tabcontrol":
            {
                int strip = UiElement.LineHeight(e.Scale) + 8;
                AddHitRect(canvas, map, e, x, y, w, h);
                int tx = x;
                for (int i = 0; i < e.Children.Count; i++)
                {
                    string header = e.Children[i].Text;
                    int tw = header.Length * UiElement.CharWidth(e.Scale) + 12;
                    AddFill(canvas, e, tx, y, tw, strip,
                        i == System.Math.Max(0, e.Selected) ? UiColors.Accent : UiColors.DarkGray);
                    AddText(canvas, map, e, tx + 6, y + 4, header, e.Foreground, e.Scale);
                    tx += tw + 2;
                }
                AddLine(canvas, e, x, y + strip, x + w, y + strip, e.Border);
                UiElement? page = e.SelectedPage();
                if (page != null)
                {
                    Draw(canvas, page, map);
                }
                break;
            }

            case "treeview":
            {
                AddHitRect(canvas, map, e, x, y, w, h);
                DrawNodes(canvas, e, map);
                break;
            }

            case "messagebox":
            {
                int boxW = w * 3 / 4;
                int boxH = h / 3;
                int bx = x + (w - boxW) / 2;
                int by = y + (h - boxH) / 2;
                AddFill(canvas, e, bx + 3, by + 3, boxW, boxH, UiColors.Black);
                AddBox(canvas, map, e, bx, by, boxW, boxH, e.Background, UiColors.Accent);
                var msg = UiElement.WrapText(e.Text, boxW - e.Padding * 2, e.Scale);
                int my = by + e.Padding;
                foreach (string line in msg)
                {
                    AddText(canvas, map, e, bx + e.Padding, my, line, e.Foreground, e.Scale);
                    my += UiElement.LineHeight(e.Scale) + 2;
                }
                for (int i = 0; i < e.Children.Count; i++)
                {
                    Draw(canvas, e.Children[i], map);
                }
                break;
            }

            default: // containers
                if (e.Kind == "border" || e.Kind == "panel" || e.Background != UiColors.Black)
                {
                    AddBox(canvas, map, e, x, y, w, h, e.Background,
                        e.Kind == "border" ? e.Border : e.Background);
                }
                else
                {
                    // Invisible container: a transparent hit rect for selection.
                    AddHitRect(canvas, map, e, x, y, w, h);
                }
                for (int i = 0; i < e.Children.Count; i++)
                {
                    Draw(canvas, e.Children[i], map);
                }
                break;
        }
    }

    /// <summary>Tree rows, indented, with a marker on anything expandable.</summary>
    private static void DrawNodes(Canvas canvas, UiElement node,
        Dictionary<FrameworkElement, UiElement> map)
    {
        for (int i = 0; i < node.Children.Count; i++)
        {
            UiElement c = node.Children[i];
            if (c.Children.Count > 0)
            {
                AddText(canvas, map, c, c.LayoutX, c.LayoutY + 2, c.Checked ? "-" : "+",
                    UiColors.Accent, c.Scale);
            }
            AddText(canvas, map, c, c.LayoutX + UiElement.CharWidth(c.Scale) + 2,
                c.LayoutY + 2, c.Text, c.Foreground, c.Scale);
            if (c.Checked)
            {
                DrawNodes(canvas, c, map);
            }
        }
    }

    private static void AddBox(Canvas c, Dictionary<FrameworkElement, UiElement> map, UiElement e,
        int x, int y, int w, int h, int fill, int stroke)
    {
        var r = new Rectangle
        {
            Width = System.Math.Max(1, w),
            Height = System.Math.Max(1, h),
            Fill = Brush(fill),
            Stroke = Brush(stroke),
            StrokeThickness = 1,
        };
        Canvas.SetLeft(r, x);
        Canvas.SetTop(r, y);
        c.Children.Add(r);
        map[r] = e;
    }

    private static void AddFill(Canvas c, UiElement e, int x, int y, int w, int h, int fill)
    {
        var r = new Rectangle { Width = System.Math.Max(1, w), Height = System.Math.Max(1, h), Fill = Brush(fill) };
        Canvas.SetLeft(r, x);
        Canvas.SetTop(r, y);
        c.Children.Add(r);
    }

    private static void AddHitRect(Canvas c, Dictionary<FrameworkElement, UiElement> map, UiElement e,
        int x, int y, int w, int h)
    {
        var r = new Rectangle
        {
            Width = System.Math.Max(1, w),
            Height = System.Math.Max(1, h),
            Fill = Brushes.Transparent,
        };
        Canvas.SetLeft(r, x);
        Canvas.SetTop(r, y);
        c.Children.Add(r);
        map[r] = e;
    }

    private static void AddEllipse(Canvas c, Dictionary<FrameworkElement, UiElement> map, UiElement e,
        int x, int y, int w, int h, int stroke, int fill)
    {
        var el = new Ellipse
        {
            Width = w,
            Height = h,
            Stroke = Brush(stroke),
            StrokeThickness = 1,
            Fill = fill >= 0 ? Brush(fill) : Brushes.Transparent,
        };
        Canvas.SetLeft(el, x);
        Canvas.SetTop(el, y);
        c.Children.Add(el);
        map[el] = e;
    }

    private static void AddLine(Canvas c, UiElement e, int x0, int y0, int x1, int y1, int stroke)
    {
        var l = new Line { X1 = x0, Y1 = y0, X2 = x1, Y2 = y1, Stroke = Brush(stroke), StrokeThickness = 1 };
        c.Children.Add(l);
    }

    private static void AddText(Canvas c, Dictionary<FrameworkElement, UiElement> map, UiElement e,
        int x, int y, string text, int color, int scale)
    {
        var t = new TextBlock
        {
            Text = text,
            Foreground = Brush(color),
            FontFamily = new FontFamily("Consolas"),
            FontSize = 7 * (scale < 1 ? 1 : scale),
        };
        Canvas.SetLeft(t, x);
        Canvas.SetTop(t, y);
        c.Children.Add(t);
        map[t] = e;
    }
}
