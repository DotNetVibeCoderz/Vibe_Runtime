using RustNet.Graphics;
using RustNet.Threading;

namespace __NAME__;

/// <summary>
/// Low-level graphics primitives showcase (a richer take on the nanoFramework
/// "Primitives" sample). Cycles through scenes that exercise every built-in
/// drawing call — pixels, lines, rectangles, circles, gradients, clipping,
/// text at several scales — plus app-side primitives the base library does not
/// ship (filled circles, triangles, ellipses, rounded rectangles) and a
/// double-buffered "matrix rain" finale.
///
/// Adapts to whatever panel size Display reports, so it fills a 320x240 M5
/// Tough or a 160x128 TFT alike. Each scene also logs a marker to the console
/// so progress is visible over `rustnet logs` even without eyes on the panel.
/// </summary>
public static class Program
{
    private static int W;
    private static int H;
    private static Random _rng = new Random(0x5A17);

    public static void Main()
    {
        Console.WriteLine("__NAME__ graphics primitives demo");
        Console.WriteLine("by Gravicode Studios, led by Kang Fadhil");
        Display.Init(320, 240);
        W = Display.Width();
        H = Display.Height();
        Console.WriteLine(string.Concat("panel ", W.ToString(), "x", H.ToString()));

        while (true)
        {
            SceneTitle();
            SceneLines();
            SceneRects();
            SceneCirclesEllipses();
            SceneTriangles();
            SceneGradients();
            SceneText();
            SceneCube();
            SceneBouncingBalls();
            SceneMatrixRain();
        }
    }

    // ---- Scenes ---------------------------------------------------------

    private static void SceneTitle()
    {
        Console.WriteLine("scene: title");
        Display.FillGradient(0, 0, W, H, Color.FromRgb(0, 0, 40), Color.FromRgb(0, 0, 8), true);
        // Color swatches across the top.
        int n = 8;
        int sw = W / n;
        for (int i = 0; i < n; i++)
        {
            Display.FillRect(i * sw, 8, sw - 2, 24, Hue(i * 255 / n));
        }
        Border(Color.FromRgb(60, 60, 90));
        CenterText(H / 2 - 28, "RustNet", Color.White, 4);
        CenterText(H / 2 + 8, "Graphics Primitives", Color.Cyan, 2);
        CenterText(H - 30, "low-level drawing showcase", Color.FromRgb(140, 140, 160), 1);
        CenterText(H - 16, "by Gravicode Studios - Kang Fadhil", Color.FromRgb(110, 190, 255), 1);
        Display.Present();
        Sleep.Ms(2200);
    }

    private static void SceneLines()
    {
        Console.WriteLine("scene: lines");
        Display.Clear(Color.Black);
        Header("Lines & pixels");
        int top = 32;
        int bot = H - 6;
        int left = 6;
        int right = W - 6;
        int steps = 16;
        // String art: fan lines across two corners, rainbow-hued (integer only).
        for (int i = 0; i <= steps; i++)
        {
            int hue = i * 255 / steps;
            int x1 = left + (right - left) * i / steps;
            int y2 = top + (bot - top) * i / steps;
            Display.DrawLine(x1, top, left, y2, Hue(hue));
            Display.DrawLine(right - (x1 - left), bot, right, bot - (y2 - top), Hue(hue + 128));
        }
        // A scatter of individual pixels (star field).
        for (int i = 0; i < 140; i++)
        {
            Display.SetPixel(left + _rng.Next(right - left), top + _rng.Next(bot - top),
                Color.FromRgb(130, 130, 150));
        }
        Display.Present();
        Sleep.Ms(2200);
    }

    private static void SceneRects()
    {
        Console.WriteLine("scene: rectangles");
        Display.Clear(Color.FromRgb(8, 8, 16));
        Header("Rectangles & clipping");
        // Nested outlined rectangles.
        for (int i = 0; i < 6; i++)
        {
            Display.DrawRect(10 + i * 6, 34 + i * 6, 90 - i * 12, 90 - i * 12, Hue(i * 40));
        }
        // Filled + rounded rectangles.
        Display.FillRect(120, 40, 70, 40, Color.FromRgb(0, 120, 200));
        FillRoundRect(120, 90, 70, 40, 10, Color.FromRgb(200, 80, 0));
        DrawRoundRect(120, 90, 70, 40, 10, Color.White);
        // Clipping: a diagonal hatch confined to a window.
        int clx = 210;
        int cly = 40;
        int clw = W - clx - 10;
        int clh = 90;
        Display.DrawRect(clx - 1, cly - 1, clw + 2, clh + 2, Color.FromRgb(120, 120, 120));
        Display.SetClip(clx, cly, clw, clh);
        for (int d = -clh; d < clw; d += 8)
        {
            Display.DrawLine(clx + d, cly, clx + d + clh, cly + clh, Color.Green);
        }
        Display.ClearClip();
        CenterText(H - 18, "hatch above is clipped to its window", Color.FromRgb(150, 150, 150), 1);
        Display.Present();
        Sleep.Ms(2600);
    }

    private static void SceneCirclesEllipses()
    {
        Console.WriteLine("scene: circles/ellipses");
        Display.Clear(Color.Black);
        Header("Circles & ellipses");
        // Concentric outlined circles.
        int cx = W / 4;
        int cy = H / 2 + 6;
        for (int r = 6; r < H / 2 - 12; r += 8)
        {
            Display.DrawCircle(cx, cy, r, Hue(r * 6));
        }
        // Filled circles: a traffic light.
        int tx = W / 2 + 4;
        FillRoundRect(tx - 16, 34, 32, H - 60, 12, Color.FromRgb(30, 30, 30));
        FillCircle(tx, 52, 12, Color.Red);
        FillCircle(tx, 52 + 34, 12, Color.Yellow);
        FillCircle(tx, 52 + 68, 12, Color.Green);
        // Ellipses: outlined and filled.
        int ex = 3 * W / 4;
        FillEllipse(ex, cy, W / 8, H / 6, Color.FromRgb(120, 0, 160));
        DrawEllipse(ex, cy, W / 8, H / 6, Color.White);
        DrawEllipse(ex, cy, W / 10, H / 4, Color.Cyan);
        Display.Present();
        Sleep.Ms(2600);
    }

    private static void SceneTriangles()
    {
        Console.WriteLine("scene: triangles");
        Display.Clear(Color.FromRgb(6, 10, 6));
        Header("Triangles");
        // A filled "triforce".
        int s = H - 70;
        int bx = W / 2 - s / 2;
        int by = H - 20;
        FillTriangle(bx + s / 4, by - s / 2, bx + s / 4 - s / 4, by, bx + s / 4 + s / 4, by, Hue(20));
        FillTriangle(bx + 3 * s / 4, by - s / 2, bx + 3 * s / 4 - s / 4, by, bx + 3 * s / 4 + s / 4, by, Hue(90));
        FillTriangle(bx + s / 2, by - s, bx + s / 2 - s / 4, by - s / 2, bx + s / 2 + s / 4, by - s / 2, Hue(160));
        // Outlined fan on the left.
        for (int i = 0; i < 6; i++)
        {
            DrawTriangle(10, 40, 10 + 70, 40 + i * 14, 10 + 40, 40 + 80, Hue(i * 40 + 200));
        }
        Display.Present();
        Sleep.Ms(2400);
    }

    private static void SceneGradients()
    {
        Console.WriteLine("scene: gradients");
        Display.Clear(Color.Black);
        Header("Gradients");
        int third = (H - 44) / 3;
        Display.FillGradient(8, 34, W - 16, third - 4, Color.Red, Color.Blue, false);
        Display.FillGradient(8, 34 + third, W - 16, third - 4, Color.Green, Color.Magenta, false);
        // A 2D-ish blend: vertical bands with a horizontal gradient each.
        int y3 = 34 + 2 * third;
        int bands = 6;
        int bw = (W - 16) / bands;
        for (int i = 0; i < bands; i++)
        {
            Display.FillGradient(8 + i * bw, y3, bw, third - 4, Hue(i * 42), Color.Black, true);
        }
        Display.Present();
        Sleep.Ms(2400);
    }

    private static void SceneText()
    {
        Console.WriteLine("scene: text");
        Display.Clear(Color.FromRgb(0, 12, 24));
        Header("Text scales");
        Display.DrawText(8, 40, "scale 1: the quick brown fox", Color.White, 1);
        Display.DrawText(8, 58, "scale 2: RustNet", Color.Yellow, 2);
        Display.DrawText(8, 84, "scale 3: 0123", Color.Cyan, 3);
        Display.DrawText(8, 120, "scale 4", Color.Green, 4);
        // A row of colored labels.
        int[] cols = new int[6];
        cols[0] = Color.Red; cols[1] = Color.Green; cols[2] = Color.Blue;
        cols[3] = Color.Yellow; cols[4] = Color.Cyan; cols[5] = Color.Magenta;
        for (int i = 0; i < 6; i++)
        {
            Display.DrawText(8 + i * 52, H - 20, "COLOR", cols[i], 1);
        }
        Display.Present();
        Sleep.Ms(2400);
    }

    private static void SceneCube()
    {
        Console.WriteLine("scene: 3d cube");
        int cx = W / 2;
        int cy = H / 2 + 8;
        int s = (H < W ? H : W) / 5; // half edge length

        // 8 cube corners at (±s, ±s, ±s); index bits = (xi<<2)|(yi<<1)|zi.
        int[] vx = new int[8];
        int[] vy = new int[8];
        int[] vz = new int[8];
        int idx = 0;
        for (int xi = -1; xi <= 1; xi += 2)
            for (int yi = -1; yi <= 1; yi += 2)
                for (int zi = -1; zi <= 1; zi += 2)
                {
                    vx[idx] = xi * s;
                    vy[idx] = yi * s;
                    vz[idx] = zi * s;
                    idx++;
                }

        // 12 edges: each pair of corners differing in exactly one axis.
        int[] ea = new int[12];
        int[] eb = new int[12];
        ea[0] = 0; eb[0] = 1; ea[1] = 2; eb[1] = 3; ea[2] = 4; eb[2] = 5; ea[3] = 6; eb[3] = 7;
        ea[4] = 0; eb[4] = 2; ea[5] = 1; eb[5] = 3; ea[6] = 4; eb[6] = 6; ea[7] = 5; eb[7] = 7;
        ea[8] = 0; eb[8] = 4; ea[9] = 1; eb[9] = 5; ea[10] = 2; eb[10] = 6; ea[11] = 3; eb[11] = 7;

        int[] px = new int[8];
        int[] py = new int[8];
        int dist = 4 * s; // perspective viewer distance

        for (int f = 0; f < 180; f++)
        {
            int cosY = Cos(f * 4);
            int sinY = Sin(f * 4);
            int cosX = Cos(f * 3);
            int sinX = Sin(f * 3);
            for (int i = 0; i < 8; i++)
            {
                int x = vx[i];
                int y = vy[i];
                int z = vz[i];
                // Rotate about Y, then X (fixed-point, /1000).
                int x1 = (x * cosY + z * sinY) / 1000;
                int z1 = (z * cosY - x * sinY) / 1000;
                int y1 = (y * cosX - z1 * sinX) / 1000;
                int z2 = (y * sinX + z1 * cosX) / 1000;
                // Perspective projection.
                int denom = dist + z2;
                if (denom < 1) denom = 1;
                px[i] = cx + x1 * dist / denom;
                py[i] = cy + y1 * dist / denom;
            }
            Display.Clear(Color.Black);
            Header("3D wireframe cube");
            for (int e = 0; e < 12; e++)
            {
                Display.DrawLine(px[ea[e]], py[ea[e]], px[eb[e]], py[eb[e]], Hue(e * 21));
            }
            Display.Present();
            Sleep.Ms(3); // yield to the device service between frames
        }
    }

    private static void SceneBouncingBalls()
    {
        Console.WriteLine("scene: bouncing balls");
        // Fewer, smaller circles (each FillCircle is an interpreted scanline)
        // moving faster — livelier without the per-frame cost of big discs.
        int count = 4;
        int[] bx = new int[count];
        int[] by = new int[count];
        int[] dx = new int[count];
        int[] dy = new int[count];
        int[] br = new int[count];
        int[] bc = new int[count];
        for (int i = 0; i < count; i++)
        {
            bx[i] = 20 + _rng.Next(W - 40);
            by[i] = 40 + _rng.Next(H - 60);
            dx[i] = 4 + _rng.Next(5);
            dy[i] = 3 + _rng.Next(4);
            br[i] = 6 + _rng.Next(6);
            bc[i] = Hue(_rng.Next(255));
        }
        for (int f = 0; f < 90; f++)
        {
            Display.Clear(Color.Black);
            Header("Animation: bouncing balls");
            for (int i = 0; i < count; i++)
            {
                FillCircle(bx[i], by[i], br[i], bc[i]);
                bx[i] += dx[i];
                by[i] += dy[i];
                if (bx[i] < br[i] || bx[i] > W - br[i]) dx[i] = -dx[i];
                if (by[i] < 34 + br[i] || by[i] > H - br[i]) dy[i] = -dy[i];
            }
            Display.Present();
            Sleep.Ms(3); // yield to the device service between frames
        }
    }

    private static void SceneMatrixRain()
    {
        Console.WriteLine("scene: matrix rain");
        // Keep per-frame interpreted work low: few columns, short trails,
        // scale-1 glyphs (a quarter the pixels of scale 2), and a trail-colour
        // palette computed once instead of a FromRgb call per cell.
        int cw = 20;
        int ch = 16;
        int trail = 4;
        int cols = W / cw;
        int rows = H / ch;
        int[] head = new int[cols];
        for (int i = 0; i < cols; i++) head[i] = _rng.Next(rows) - rows;
        int[] pal = new int[trail];
        pal[0] = Color.White;
        for (int t = 1; t < trail; t++)
        {
            int g = 255 - t * 55;
            if (g < 70) g = 70;
            pal[t] = Color.FromRgb(0, g, g / 4);
        }
        for (int f = 0; f < 80; f++)
        {
            Display.Clear(Color.Black);
            for (int c = 0; c < cols; c++)
            {
                for (int t = 0; t < trail; t++)
                {
                    int r = head[c] - t;
                    if (r < 0 || r >= rows) continue;
                    string ch1 = _rng.Next(2) == 0 ? "0" : "1";
                    Display.DrawText(c * cw, r * ch, ch1, pal[t], 1);
                }
                head[c]++;
                if (head[c] - trail > rows && _rng.Next(4) == 0) head[c] = 0;
            }
            Display.Present();
            Sleep.Ms(3); // yield to the device service between frames
        }
    }

    // ---- Chrome helpers -------------------------------------------------

    private static void Header(string title)
    {
        Display.FillRect(0, 0, W, 26, Color.FromRgb(20, 20, 40));
        Display.DrawLine(0, 26, W - 1, 26, Color.FromRgb(80, 80, 120));
        Display.DrawText(8, 8, title, Color.White, 2);
    }

    private static void Border(int color)
    {
        Display.DrawRect(2, 2, W - 4, H - 4, color);
        Display.DrawRect(3, 3, W - 6, H - 6, color);
    }

    private static void CenterText(int y, string text, int color, int scale)
    {
        int tw = text.Length * 6 * scale;
        int x = (W - tw) / 2;
        if (x < 0) x = 0;
        Display.DrawText(x, y, text, color, scale);
    }

    // ---- App-side primitives (integer math only) ------------------------

    // Native filled circle — the runtime does the scanline in Rust, so it stays
    // cheap even when called every animation frame.
    private static void FillCircle(int cx, int cy, int r, int color)
    {
        Display.FillCircle(cx, cy, r, color);
    }

    private static void FillEllipse(int cx, int cy, int a, int b, int color)
    {
        if (a <= 0 || b <= 0) return;
        for (int dy = -b; dy <= b; dy++)
        {
            int hw = Isqrt(a * a * (b * b - dy * dy)) / b;
            Display.FillRect(cx - hw, cy + dy, 2 * hw + 1, 1, color);
        }
    }

    private static void DrawEllipse(int cx, int cy, int a, int b, int color)
    {
        if (a <= 0 || b <= 0) return;
        // Left/right endpoints per row, plus top/bottom caps, for a clean outline.
        int prevHw = 0;
        for (int dy = -b; dy <= b; dy++)
        {
            int hw = Isqrt(a * a * (b * b - dy * dy)) / b;
            Display.SetPixel(cx - hw, cy + dy, color);
            Display.SetPixel(cx + hw, cy + dy, color);
            if (dy == -b || dy == b)
            {
                Display.FillRect(cx - hw, cy + dy, 2 * hw + 1, 1, color);
            }
            else if (hw > prevHw + 1)
            {
                // Bridge horizontal caps where the curve moves fast.
                Display.FillRect(cx - hw, cy + dy, hw - prevHw, 1, color);
                Display.FillRect(cx + prevHw, cy + dy, hw - prevHw, 1, color);
            }
            prevHw = hw;
        }
    }

    private static void DrawTriangle(int x0, int y0, int x1, int y1, int x2, int y2, int color)
    {
        Display.DrawLine(x0, y0, x1, y1, color);
        Display.DrawLine(x1, y1, x2, y2, color);
        Display.DrawLine(x2, y2, x0, y0, color);
    }

    private const int NoCross = -2000000000;

    private static void FillTriangle(int x0, int y0, int x1, int y1, int x2, int y2, int color)
    {
        int ymin = Min3(y0, y1, y2);
        int ymax = Max3(y0, y1, y2);
        for (int y = ymin; y <= ymax; y++)
        {
            int lo = 1000000;
            int hi = -1000000;
            // EdgeX returns the crossing x (or NoCross) — no ref params, since
            // the interpreter's byref is same-frame only.
            int e0 = EdgeX(x0, y0, x1, y1, y);
            int e1 = EdgeX(x1, y1, x2, y2, y);
            int e2 = EdgeX(x2, y2, x0, y0, y);
            if (e0 != NoCross) { if (e0 < lo) lo = e0; if (e0 > hi) hi = e0; }
            if (e1 != NoCross) { if (e1 < lo) lo = e1; if (e1 > hi) hi = e1; }
            if (e2 != NoCross) { if (e2 < lo) lo = e2; if (e2 > hi) hi = e2; }
            if (hi >= lo)
            {
                Display.FillRect(lo, y, hi - lo + 1, 1, color);
            }
        }
    }

    private static int EdgeX(int ax, int ay, int bx, int by, int y)
    {
        if (ay == by) return NoCross;
        if ((y < ay || y > by) && (y < by || y > ay)) return NoCross;
        return ax + (bx - ax) * (y - ay) / (by - ay);
    }

    private static void DrawRoundRect(int x, int y, int w, int h, int r, int color)
    {
        Display.DrawLine(x + r, y, x + w - r - 1, y, color);
        Display.DrawLine(x + r, y + h - 1, x + w - r - 1, y + h - 1, color);
        Display.DrawLine(x, y + r, x, y + h - r - 1, color);
        Display.DrawLine(x + w - 1, y + r, x + w - 1, y + h - r - 1, color);
        Corner(x + r, y + r, r, 2, color);
        Corner(x + w - r - 1, y + r, r, 1, color);
        Corner(x + r, y + h - r - 1, r, 3, color);
        Corner(x + w - r - 1, y + h - r - 1, r, 0, color);
    }

    private static void FillRoundRect(int x, int y, int w, int h, int r, int color)
    {
        Display.FillRect(x + r, y, w - 2 * r, h, color);
        Display.FillRect(x, y + r, r, h - 2 * r, color);
        Display.FillRect(x + w - r, y + r, r, h - 2 * r, color);
        // Rounded corners as filled quarter discs.
        for (int dy = 0; dy <= r; dy++)
        {
            int hw = Isqrt(r * r - dy * dy);
            Display.FillRect(x + r - hw, y + r - dy, hw, 1, color);
            Display.FillRect(x + w - r, y + r - dy, hw, 1, color);
            Display.FillRect(x + r - hw, y + h - r - 1 + dy, hw, 1, color);
            Display.FillRect(x + w - r, y + h - r - 1 + dy, hw, 1, color);
        }
    }

    // Draw one quarter-circle outline. quadrant: 0=BR,1=TR,2=TL,3=BL.
    private static void Corner(int cx, int cy, int r, int quadrant, int color)
    {
        int x = r;
        int y = 0;
        int d = 1 - r;
        while (x >= y)
        {
            Plot(cx, cy, x, y, quadrant, color);
            Plot(cx, cy, y, x, quadrant, color);
            y++;
            if (d < 0) { d += 2 * y + 1; }
            else { x--; d += 2 * (y - x) + 1; }
        }
    }

    private static void Plot(int cx, int cy, int dx, int dy, int quadrant, int color)
    {
        if (quadrant == 0) Display.SetPixel(cx + dx, cy + dy, color);
        else if (quadrant == 1) Display.SetPixel(cx + dx, cy - dy, color);
        else if (quadrant == 2) Display.SetPixel(cx - dx, cy - dy, color);
        else Display.SetPixel(cx - dx, cy + dy, color);
    }

    // ---- Math -----------------------------------------------------------

    private static int Isqrt(int n)
    {
        if (n <= 0) return 0;
        int x = n;
        int y = (x + 1) / 2;
        while (y < x)
        {
            x = y;
            y = (x + n / x) / 2;
        }
        return x;
    }

    private static int Min3(int a, int b, int c)
    {
        int m = a < b ? a : b;
        return m < c ? m : c;
    }

    private static int Max3(int a, int b, int c)
    {
        int m = a > b ? a : b;
        return m > c ? m : c;
    }

    // HSV → RGB565 with full saturation/value; hue in 0..255 (wraps).
    private static int Hue(int h)
    {
        h = ((h % 256) + 256) % 256;
        int region = h / 43;      // 0..5
        int f = (h % 43) * 6;     // 0..252 ramp within the region
        int q = 255 - f;
        int r;
        int g;
        int b;
        if (region == 0) { r = 255; g = f; b = 0; }
        else if (region == 1) { r = q; g = 255; b = 0; }
        else if (region == 2) { r = 0; g = 255; b = f; }
        else if (region == 3) { r = 0; g = q; b = 255; }
        else if (region == 4) { r = f; g = 0; b = 255; }
        else { r = 255; g = 0; b = q; }
        return Color.FromRgb(r, g, b);
    }

    // Sine scaled to ±1000, degrees in — Bhaskara I approximation (no lookup
    // table, so it avoids the interpreter's array-initializer limitation).
    private static int Sin(int deg)
    {
        deg = ((deg % 360) + 360) % 360;
        int sign = 1;
        if (deg >= 180)
        {
            deg -= 180;
            sign = -1;
        }
        int t = deg * (180 - deg);
        return sign * (4000 * t) / (40500 - t);
    }

    private static int Cos(int deg)
    {
        return Sin(deg + 90);
    }
}
