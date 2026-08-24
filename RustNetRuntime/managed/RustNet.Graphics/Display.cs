using System.Runtime.CompilerServices;

namespace RustNet.Graphics;

/// <summary>
/// RGB565 colours, packed into an <c>int</c>.
/// </summary>
/// <remarks>
/// Not a struct: a struct crossing the runtime boundary once per pixel would
/// allocate, and a fill loop is exactly where a device spends its drawing
/// time. The packing is done here in C# rather than by a runtime call for the
/// same reason — <see cref="FromRgb"/> in a loop should not be a transition.
/// <para>
/// The panel takes 5 bits of red, 6 of green and 5 of blue, so the low bits of
/// what you pass are lost. That is the format, not a rounding choice made
/// here.
/// </para>
/// </remarks>
public static class Color
{
    public const int Black = 0x0000;
    public const int White = 0xFFFF;
    public const int Red = 0xF800;
    public const int Green = 0x07E0;
    public const int Blue = 0x001F;
    public const int Yellow = 0xFFE0;
    public const int Cyan = 0x07FF;
    public const int Magenta = 0xF81F;

    public static int FromRgb(int r, int g, int b)
        => ((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3);
}

/// <summary>
/// Which panel controller the display is wired to.
/// </summary>
/// <remarks>
/// Advisory on a board whose framebuffer *is* the panel; on real silicon it
/// selects the SPI command set. <see cref="Display.Init"/> uses
/// <see cref="Auto"/>, which is what the board's own HAL reports.
/// </remarks>
public enum PanelDriver
{
    Auto = 0,
    Ssd1306 = 1,   // I2C/SPI OLED
    St7735 = 2,    // SPI TFT
    Ili9341 = 3,   // SPI TFT
    Generic = 4,   // parallel/RGB
}

/// <summary>
/// The drawing surface: one RGB565 canvas, and the primitives a device UI
/// needs.
/// </summary>
/// <remarks>
/// <para>
/// There is one canvas, not one per surface. A 240×240 RGB565 buffer is
/// 115 KB — on most boards this runs on, that is the entire heap budget, so a
/// second one is not a feature, it is an out-of-memory. <see cref="Init"/>
/// creates it and its size is your choice.
/// </para>
/// <para>
/// Drawing works without a board. <see cref="Present"/> is the only call that
/// needs one — which is what lets a UI be tested headlessly, and why the
/// device fixture can check a rendered pixel on a desktop.
/// </para>
/// </remarks>
public static class Display
{
    /// <summary>Creates the canvas. Sizes are in pixels.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Init(int width, int height);

    /// <summary>
    /// Creates the canvas and names the panel controller.
    /// </summary>
    /// <remarks>
    /// The driver is currently advisory: the board's HAL decides how a frame
    /// reaches its panel, and <see cref="Present"/> hands it the pixels
    /// whatever is passed here. Stated rather than silently ignored.
    /// </remarks>
    public static void Configure(PanelDriver driver, int width, int height, int rotation = 0)
    {
        Init(width, height);
        if (rotation != 0)
        {
            SetRotation(rotation);
        }
    }

    /// <summary>Releases the canvas and its memory.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Close();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool IsOpen();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int Width();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int Height();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Clear(int color);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void SetPixel(int x, int y, int color);

    /// <summary>
    /// The colour at a point, or <c>-1</c> outside the canvas. Not black:
    /// black is a colour a program might have drawn, and "outside" is not.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int GetPixel(int x, int y);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void DrawLine(int x0, int y0, int x1, int y1, int color);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void DrawRect(int x, int y, int width, int height, int color);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void FillRect(int x, int y, int width, int height, int color);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void DrawCircle(int cx, int cy, int r, int color);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void FillCircle(int cx, int cy, int r, int color);

    /// <summary>
    /// Fills a rectangle with a linear blend between two colours.
    /// <paramref name="vertical"/> runs the blend top to bottom rather than
    /// left to right.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void FillGradient(int x, int y, int width, int height,
        int from, int to, bool vertical);

    /// <summary>
    /// Blits RGB565 pixels the caller already has, two bytes per pixel,
    /// little-endian.
    /// </summary>
    /// <remarks>
    /// There is no image decoder in the runtime — no PNG, no JPEG. A device
    /// that had one would be spending flash on a format its panel cannot show
    /// without converting anyway, so conversion happens on the host and the
    /// device blits.
    /// </remarks>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void DrawImage(int x, int y, int width, int height, byte[] rgb565);

    /// <summary>
    /// Draws text in the built-in 8×8 font at an integer scale. Non-ASCII
    /// draws as a gap rather than a substitute glyph — a device showing `?`
    /// where a degree sign belongs is showing a wrong reading.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void DrawText(int x, int y, string text, int color, int scale);

    /// <summary>How wide <see cref="DrawText"/> would draw this, in pixels.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int MeasureText(string text, int scale);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void SetClip(int x, int y, int width, int height);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void ClearClip();

    /// <summary>0, 90, 180 or 270. Anything else throws.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void SetRotation(int degrees);

    /// <summary>
    /// Pushes the canvas to the board's panel. Needs a board; a board with no
    /// wired panel accepts the frame and does nothing with it, so the same
    /// code runs headless.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Present();
}
