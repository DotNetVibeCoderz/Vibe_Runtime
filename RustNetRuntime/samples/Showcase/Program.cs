using RustNet.Graphics;
using RustNet.IO;
using RustNet.Sys;
using RustNet.Threading;

namespace Showcase;

/// <summary>
/// A graphics showcase for the Maix Go's 320x240 panel: a 2D starfield, a
/// rotating 3D solid, and a closing title that catches fire.
///
/// <para>
/// Everything here is C#, interpreted on-chip. That is the constraint the whole
/// design bends around: a host call into <see cref="Display"/> runs native Rust
/// over the framebuffer and is cheap, while a C# loop over 76 800 pixels is
/// not. So the rules are — shapes and lines go through the native primitives,
/// per-pixel work happens on a small grid and is blitted, and nothing is
/// recomputed per frame that can be computed once.
/// </para>
///
/// <para>
/// Timings are printed to the console (visible with <c>rustnet logs</c>),
/// because the honest way to tune an animation on hardware nobody has profiled
/// is to make it report on itself rather than to guess.
/// </para>
/// </summary>
internal static class Program
{
    private const int W = 320;
    private const int H = 240;

    private static void Main()
    {
        Display.Init(W, H);

        int run = RecordThisRun();

        while (true)
        {
            Intro(run);
            Solid3D();
            Burn();
        }
    }

    // -- Filesystem ------------------------------------------------------
    //
    // Also the on-device proof that the flash filesystem works: the counter has
    // to survive a power cycle, and it is shown on screen, so a wrong answer is
    // visible rather than buried in a log.

    private const string RunFile = "/showcase/runs.txt";

    private static int RecordThisRun()
    {
        // Storage is a board feature, not a runtime one: a Maix Go booted into
        // SRAM has no filesystem bound and says so rather than pretending to
        // write. That is a fact about the hardware, not a failure of the demo -
        // the scenes are the point, and they do not need a counter. So the
        // counter is skipped, loudly, and the run goes on.
        try
        {
            return CountThisRun();
        }
        catch (IOException e)
        {
            Console.WriteLine("[fs] no counter on this board: " + e.Message);
            return 0;
        }
    }

    private static int CountThisRun()
    {
        FileSystem.CreateDirectory("/showcase");

        int previous = 0;
        if (FileSystem.Exists(RunFile))
        {
            string text = FileSystem.ReadAllText(RunFile).Trim();
            if (text.Length > 0)
            {
                previous = int.Parse(text);
            }
        }

        int run = previous + 1;
        FileSystem.WriteAllText(RunFile, run.ToString());
        // `List` returns the names rather than one joined blob: a caller that
        // wants a string can join it, and a caller that wants the entries
        // cannot split one reliably when a name contains the separator.
        Console.WriteLine("[fs] run #" + run + ", files under /showcase: "
            + string.Join(", ", FileSystem.List("/showcase")));
        return run;
    }

    // -- Shared maths ----------------------------------------------------
    //
    // A sine table in fixed point, built once. Trigonometry per vertex per
    // frame would be host calls into libm through the interpreter; a table
    // lookup is an array index.

    private const int Steps = 256;
    private const int One = 1024;          // fixed-point scale
    private static readonly int[] Sin = BuildSine();

    private static int[] BuildSine()
    {
        int[] table = new int[Steps];
        for (int i = 0; i < Steps; i++)
        {
            table[i] = (int)(Math.Sin(i * 2.0 * Math.PI / Steps) * One);
        }
        return table;
    }

    private static int SinOf(int a) => Sin[((a % Steps) + Steps) % Steps];
    private static int CosOf(int a) => SinOf(a + Steps / 4);

    // -- Scene 1: starfield ----------------------------------------------

    private static void Intro(int run)
    {
        const int Stars = 90;
        int[] sx = new int[Stars];
        int[] sy = new int[Stars];
        int[] sz = new int[Stars];
        Random rng = new Random(7);
        for (int i = 0; i < Stars; i++)
        {
            sx[i] = rng.Next(-W, W);
            sy[i] = rng.Next(-H, H);
            sz[i] = rng.Next(16, 256);
        }

        long started = Uptime.Ms();
        int frame = 0;
        while (true)
        {
            // Every scene is bounded by wall-clock time, not by a frame count.
            // The interpreter runs at wildly different speeds on the virtual
            // device and on a 390 MHz K210 — measured at roughly forty to one —
            // so a fixed frame count would make each scene run for seconds on
            // one and minutes on the other. Progress is a permille of the
            // scene's duration, so the choreography is identical either way and
            // only the smoothness differs.
            int progress = (int)((Uptime.Ms() - started) * 1000 / IntroMs);
            if (progress >= 1000)
            {
                break;
            }
            frame++;

            // A night gradient rather than a flat clear: one native call, and
            // it stops the starfield looking like static on black.
            Display.FillGradient(0, 0, W, H, Color.FromRgb(4, 6, 28), Color.Black, true);

            for (int i = 0; i < Stars; i++)
            {
                sz[i] -= 3;
                if (sz[i] < 16)
                {
                    sz[i] = 255;
                    sx[i] = rng.Next(-W, W);
                    sy[i] = rng.Next(-H, H);
                }
                int px = W / 2 + sx[i] * 64 / sz[i];
                int py = H / 2 + sy[i] * 64 / sz[i];
                if (px < 0 || px >= W || py < 0 || py >= H)
                {
                    continue;
                }
                // Nearer stars are brighter and bigger — the only depth cue a
                // starfield has. The floor matters: without it the far stars
                // land on near-black and the field reads as an empty screen
                // with a few dots in the middle.
                int bright = 70 + (255 - sz[i]) * 3 / 4;
                int colour = Color.FromRgb(bright, bright, 255);
                if (sz[i] < 90)
                {
                    Display.FillRect(px, py, 2, 2, colour);
                }
                else
                {
                    Display.SetPixel(px, py, colour);
                }
            }

            if (progress > 150)
            {
                int fade = Math.Min(255, (progress - 150) * 255 / 350);
                int grey = Color.FromRgb(fade, fade, fade);
                Centre("R U S T N E T", 92, grey, 3);
                Centre("bare metal RISC-V", 130, Color.FromRgb(90, 140, fade), 1);
                Centre("run #" + run, 210, Color.FromRgb(60, 90, 120), 1);
            }

            Display.Present();
        }
        Report("intro", frame, Uptime.Ms() - started);
    }

    // -- Scene 2: the 3D solid -------------------------------------------
    //
    // An octahedron inside a cube, both wireframe, over a 2D grid floor. Only
    // 14 vertices and 24 edges get transformed in C#; every line is drawn by
    // the native primitive.

    // Both solids are generated rather than written out as coordinate tables.
    // Partly because it is shorter, and partly because a C# array initializer
    // of constants compiles to `ldtoken` + `InitializeArray`, which this
    // runtime does not support — so a literal table would not have loaded at
    // all. Generating is the way to spell a constant array here.
    //
    // A cube's eight corners are the three-bit numbers with 0 read as -1 and 1
    // as +1, and its twelve edges are exactly the pairs differing in one bit.
    private static readonly int[] CubeX = CubeAxis(1);
    private static readonly int[] CubeY = CubeAxis(2);
    private static readonly int[] CubeZ = CubeAxis(4);
    private static readonly int[] CubeEdgeA = CubeEdges(true);
    private static readonly int[] CubeEdgeB = CubeEdges(false);

    private static int[] CubeAxis(int bit)
    {
        int[] axis = new int[8];
        for (int i = 0; i < 8; i++)
        {
            axis[i] = (i & bit) != 0 ? 1 : -1;
        }
        return axis;
    }

    private static int[] CubeEdges(bool wantFrom)
    {
        int[] edges = new int[12];
        int n = 0;
        for (int i = 0; i < 8; i++)
        {
            for (int bit = 1; bit <= 4; bit <<= 1)
            {
                if ((i & bit) == 0)
                {
                    edges[n] = wantFrom ? i : (i | bit);
                    n++;
                }
            }
        }
        return edges;
    }

    // An octahedron is the six points one step along each axis, in both
    // directions; every pair of vertices on *different* axes is an edge, which
    // is the eight triangle sides.
    private static readonly int[] OctX = OctAxis(0);
    private static readonly int[] OctY = OctAxis(1);
    private static readonly int[] OctZ = OctAxis(2);
    private static readonly int[] OctEdgeA = OctEdges(true);
    private static readonly int[] OctEdgeB = OctEdges(false);

    private static int[] OctAxis(int axis)
    {
        int[] values = new int[6];
        for (int i = 0; i < 6; i++)
        {
            values[i] = i / 2 == axis ? (i % 2 == 0 ? -1 : 1) : 0;
        }
        return values;
    }

    private static int[] OctEdges(bool wantFrom)
    {
        int[] edges = new int[12];
        int n = 0;
        for (int a = 0; a < 6; a++)
        {
            for (int b = a + 1; b < 6; b++)
            {
                if (a / 2 != b / 2)
                {
                    edges[n] = wantFrom ? a : b;
                    n++;
                }
            }
        }
        return edges;
    }

    private static void Solid3D()
    {
        int[] px = new int[8];
        int[] py = new int[8];
        int[] ox = new int[6];
        int[] oy = new int[6];

        long started = Uptime.Ms();
        int frames = 0;

        while (true)
        {
            long elapsed = Uptime.Ms() - started;
            if (elapsed >= Solid3DMs)
            {
                break;
            }
            frames++;

            // Angles come from elapsed milliseconds, so the solid turns at the
            // same rate however fast the frames arrive.
            int tick = (int)(elapsed / 24);

            Display.FillGradient(0, 0, W, H, Color.FromRgb(2, 2, 16), Color.FromRgb(20, 4, 34), true);
            Horizon(tick);

            int ax = tick * 2;
            int ay = tick * 3;
            // Breathing scale, so the solid is never static even head-on.
            int scale = 58 + SinOf(tick * 2) * 12 / One;

            Project(CubeX, CubeY, CubeZ, ax, ay, scale, px, py);
            Project(OctX, OctY, OctZ, ay, ax, scale * 3 / 2, ox, oy);

            for (int e = 0; e < CubeEdgeA.Length; e++)
            {
                int a = CubeEdgeA[e];
                int b = CubeEdgeB[e];
                Display.DrawLine(px[a], py[a], px[b], py[b], Color.FromRgb(40, 190, 255));
            }
            for (int e = 0; e < OctEdgeA.Length; e++)
            {
                int a = OctEdgeA[e];
                int b = OctEdgeB[e];
                Display.DrawLine(ox[a], oy[a], ox[b], oy[b], Color.FromRgb(255, 120, 30));
            }
            // Vertices as dots: cheap, and they read as a solid's corners
            // rather than as line ends.
            for (int i = 0; i < 8; i++)
            {
                Display.FillCircle(px[i], py[i], 2, Color.White);
            }

            Centre("3D over 2D, interpreted", 214, Color.FromRgb(120, 160, 200), 1);
            Display.Present();
        }
        Report("3d", frames, Uptime.Ms() - started);
    }

    /// <summary>Rotate about Y then X, project, and centre on screen.</summary>
    private static void Project(int[] vx, int[] vy, int[] vz, int ax, int ay, int scale,
                                int[] outX, int[] outY)
    {
        int sinY = SinOf(ay);
        int cosY = CosOf(ay);
        int sinX = SinOf(ax);
        int cosX = CosOf(ax);

        for (int i = 0; i < vx.Length; i++)
        {
            int x = vx[i] * scale;
            int y = vy[i] * scale;
            int z = vz[i] * scale;

            int x1 = (x * cosY + z * sinY) / One;
            int z1 = (z * cosY - x * sinY) / One;
            int y1 = (y * cosX - z1 * sinX) / One;
            int z2 = (z1 * cosX + y * sinX) / One;

            // Weak perspective: the divisor never approaches zero, so no
            // clipping is needed and a vertex behind the camera cannot fling a
            // line across the screen.
            int depth = 320 + z2;
            outX[i] = W / 2 + x1 * 260 / depth;
            outY[i] = H / 2 + y1 * 260 / depth;
        }
    }

    /// <summary>A receding grid floor — the 2D half of the composition.</summary>
    private static void Horizon(int frame)
    {
        const int Sky = 150;
        int colour = Color.FromRgb(60, 20, 90);
        for (int i = 0; i < 9; i++)
        {
            // Rows bunch up towards the horizon, and scroll so the floor moves.
            int t = (i * 16 + frame * 2) % 144;
            int y = Sky + t * t / 144;
            if (y < H)
            {
                Display.DrawLine(0, y, W, y, colour);
            }
        }
        for (int x = -6; x <= 6; x++)
        {
            Display.DrawLine(W / 2 + x * 8, Sky, W / 2 + x * 60, H, colour);
        }
    }

    // -- Scene 3: the burning title --------------------------------------
    //
    // The classic cellular fire: a hot row at the bottom, each cell above it
    // taking a randomly-shifted, slightly cooled sample of the cell below.
    //
    // The grid is deliberately coarse. Fire is the one thing here that is
    // genuinely per-pixel, and 320x96 pixels of C# per frame would take
    // seconds. At 4x4 pixels per cell it is 1 920 cells, and the coarseness
    // reads as flame rather than as low resolution.

    // Measured, not guessed. The first version built a 320-pixel strip per grid
    // row in C# and blitted it — 15 000 byte-array writes a frame — and came in
    // at 121 ms per frame *on the virtual device*, which is roughly forty times
    // faster than the K210. That is seconds per frame on the board.
    //
    // So no per-pixel work in managed code at all: each cell is one native
    // `FillRect`, and a cell that is cold is not drawn, because the background
    // behind it is already the right colour. A frame costs a few hundred host
    // calls instead of tens of thousands of interpreted array writes.
    private const int FireW = 40;
    private const int FireH = 14;
    private const int CellW = 8;
    private const int CellH = 7;
    private const int FireTop = H - FireH * CellH;

    private const int MaxHeat = 35;

    /// <summary>
    /// Heat values collapse to this many drawing shades. Coarser than the
    /// simulation on purpose: neighbouring cells then land on the same colour
    /// often enough to merge into one rectangle, and at these cell sizes the
    /// eye cannot tell the missing steps from dithering anyway.
    /// </summary>
    private const int Shades = 12;

    private static readonly int[] Palette = BuildPalette();

    /// <summary>
    /// Black through red and orange to white — the heat ramp.
    ///
    /// The thresholds matter more than they look. Reach full red too early and
    /// every cell above the base saturates, which is not a fire, it is an
    /// orange rectangle; that is exactly what the first version of this drew.
    /// Red completes around two thirds of the range, yellow near the top, and
    /// white only in the last few steps, so white stays rare enough to read as
    /// the hottest part rather than as most of it.
    /// </summary>
    private static int[] BuildPalette()
    {
        int[] p = new int[Shades + 1];
        for (int s = 0; s <= Shades; s++)
        {
            int i = s * MaxHeat / Shades;
            int r = Math.Min(255, i * 14);
            int g = Math.Max(0, Math.Min(255, (i - 14) * 16));
            int b = Math.Max(0, Math.Min(255, (i - 28) * 32));
            p[s] = Color.FromRgb(r, g, b);
        }
        return p;
    }

    private static void Burn()
    {
        int[] heat = new int[FireW * FireH];
        // The drawing shade for every cell, computed as the heat is, so the
        // render pass is array reads and nothing else. Calling a helper per
        // cell instead cost 65 µs a time on this interpreter — a static method
        // call here is only about three times cheaper than a host call, which
        // is not the ratio one expects and is worth designing around.
        int[] shade = new int[FireW * FireH];
        Random rng = new Random(11);

        long started = Uptime.Ms();
        int frames = 0;

        while (true)
        {
            long elapsed = Uptime.Ms() - started;
            if (elapsed >= BurnMs)
            {
                break;
            }
            frames++;
            int progress = (int)(elapsed * 1000 / BurnMs);
            int tick = (int)(elapsed / 24);

            // The fire builds, holds, then dies back, so the title is legible
            // before, during and after it burns.
            int fuel = progress < 200 ? progress * MaxHeat / 200
                     : progress < 750 ? MaxHeat
                     : Math.Max(0, MaxHeat - (progress - 750) * MaxHeat / 250);

            int bottom = (FireH - 1) * FireW;
            for (int x = 0; x < FireW; x++)
            {
                int v = fuel > 0 ? Math.Max(0, fuel - rng.Next(0, 6)) : 0;
                heat[bottom + x] = v;
                shade[bottom + x] = v * Shades / MaxHeat;
            }
            for (int y = FireH - 1; y > 0; y--)
            {
                int row = y * FireW;
                int above = row - FireW;
                for (int x = 0; x < FireW; x++)
                {
                    // The horizontal jitter is what makes flames lean and
                    // flicker instead of rising in straight columns.
                    int drift = rng.Next(0, 3);
                    int from = x + drift - 1;
                    if (from < 0) from = 0;
                    if (from >= FireW) from = FireW - 1;
                    // Cooling has to outrun the grid: losing an average of 1.5
                    // steps per row over 24 rows spends the full heat range
                    // just before the top, so the flames have somewhere to fade
                    // to. Cool by less and every row saturates.
                    int v = heat[row + from] - rng.Next(0, 4);
                    if (v < 0)
                    {
                        v = 0;
                    }
                    heat[above + x] = v;
                    shade[above + x] = v * Shades / MaxHeat;
                }
            }

            Display.FillGradient(0, 0, W, FireTop, Color.FromRgb(8, 0, 12), Color.FromRgb(40, 8, 0), true);

            // Native rectangles, one per *run* of same-coloured cells rather
            // than one per cell. Cold cells are skipped entirely: the
            // background behind them is already dark, so drawing black over
            // black is a host call spent on nothing.
            //
            // Merging matters more than it looks. A host call is dispatched by
            // matching its canonical name, which on the K210 measured around
            // 300 µs — so a frame of 600 single-cell rectangles was 194 ms, and
            // the finale ran at five frames a second. Runs cut the call count
            // by about three, and the heat is quantised into
            // `Shades` bands first precisely so that neighbouring cells
            // agree often enough for runs to form. Fewer colours, more fire.
            for (int y = 0; y < FireH; y++)
            {
                int row = y * FireW;
                int top = FireTop + y * CellH;
                int x = 0;
                while (x < FireW)
                {
                    int s = shade[row + x];
                    if (s <= 0)
                    {
                        x++;
                        continue;
                    }
                    int run = 1;
                    while (x + run < FireW && shade[row + x + run] == s)
                    {
                        run++;
                    }
                    Display.FillRect(x * CellW, top, run * CellW, CellH, Palette[s]);
                    x += run;
                }
            }

            // Embers, drawn above the flame front so the fire does not end in
            // a flat line. Cheap: two dozen native calls.
            for (int i = 0; i < 24; i++)
            {
                int ex = rng.Next(0, W);
                int ey = FireTop - rng.Next(0, 70);
                int hot = rng.Next(0, 3);
                Display.FillRect(ex, ey, 1 + hot / 2, 1 + hot / 2,
                    Palette[Shades - 2 - hot]);
            }

            // Two lines rather than one: "RustNet on RiscV" at a size worth
            // looking at is 384 pixels wide on a 320-pixel panel, which the
            // first version of this quietly drew off both edges. Split, it
            // fits at scale 4 and reads as a title rather than as a caption.
            //
            // The words take the fire's colour as it climbs past them: white
            // while the flames are still low, then ember, then charred.
            int titleColour =
                progress < 250 ? Color.White
                : progress < 400 ? Color.FromRgb(255, 230, 160)
                : progress < 600 ? Color.FromRgb(255, 150, 40)
                : Color.FromRgb(150, 55, 25);

            // A flicker behind the letters, offset by two pixels, so the words
            // move with the fire instead of sitting on top of it.
            int glow = 130 + SinOf(tick * 9) * 100 / One;
            int glowColour = Color.FromRgb(glow, glow / 3, 0);
            Centre("RustNet", 130, glowColour, 4);
            Centre("on RiscV", 174, glowColour, 4);
            Centre("RustNet", 128, titleColour, 4);
            Centre("on RiscV", 172, titleColour, 4);

            Display.Present();
        }
        Report("fire", frames, Uptime.Ms() - started);
    }

    // -- Helpers ---------------------------------------------------------

    /// <summary>
    /// How long each scene runs, in milliseconds.
    /// </summary>
    private const long IntroMs = 5000;
    private const long Solid3DMs = 9000;
    private const long BurnMs = 14000;

    /// <summary>
    /// Say how the scene actually performed. On hardware nobody has profiled,
    /// a line in `rustnet logs` is the difference between tuning the animation
    /// and guessing at it.
    /// </summary>
    private static void Report(string scene, int frames, long elapsed)
    {
        int per = frames > 0 ? (int)(elapsed / frames) : 0;
        int fps = per > 0 ? 1000 / per : 0;
        Console.WriteLine("[" + scene + "] " + frames + " frames in " + elapsed
            + " ms — " + per + " ms/frame, ~" + fps + " fps");
    }

    /// <summary>Draw text centred horizontally. The 8x8 font scales by whole
    /// numbers, so the width is exactly known.</summary>
    private static void Centre(string text, int y, int colour, int scale)
    {
        int width = text.Length * 8 * scale;
        Display.DrawText((W - width) / 2, y, text, colour, scale);
    }
}
