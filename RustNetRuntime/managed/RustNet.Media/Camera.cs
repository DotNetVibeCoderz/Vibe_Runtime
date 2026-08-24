using System.Runtime.CompilerServices;

namespace RustNet.Media;

/// <summary>Pixel format a camera frame is delivered in.</summary>
public enum PixelFormat
{
    Rgb565 = 0,
    Grayscale = 1,
}

/// <summary>
/// Image sensor capture. Chip-gated (v0.8): available where the board has a
/// camera interface. The virtual device provides a colour-bar test sensor so
/// apps and pipelines can be developed without optics. A captured RGB565
/// frame blits straight to the display via RustNet.Graphics' Display.DrawImage.
/// Named rather than referenced: a doc link would make every program that
/// captures an image also load RustNet.Graphics, and a loaded assembly
/// registers all of its types whether the program mentions them or not.
/// </summary>
public static class Camera
{
    /// <summary>Configure frame size and format. Call before <see cref="Capture"/>.</summary>
    public static void Configure(int width, int height, PixelFormat format = PixelFormat.Rgb565)
        => ConfigureRaw(width, height, (int)format);

    [MethodImpl(MethodImplOptions.InternalCall)]
    private static extern void ConfigureRaw(int width, int height, int format);

    /// <summary>Capture one frame as raw bytes in the configured format
    /// (RGB565 little-endian: width*height*2 bytes).</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] Capture();

    /// <summary>Configured frame width in pixels.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int Width();

    /// <summary>Configured frame height in pixels.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int Height();
}
