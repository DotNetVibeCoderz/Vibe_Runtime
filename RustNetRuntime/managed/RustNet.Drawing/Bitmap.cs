namespace RustNet.Drawing;

/// <summary>
/// An in-memory image as a display-ready RGB565 surface (System.Drawing's
/// Bitmap, sized for the device). Decoders fill it; the pixel buffer blits
/// to the display in one call via <c>Display.DrawImage</c>.
/// </summary>
public class Bitmap
{
    public int Width { get; }
    public int Height { get; }

    /// <summary>RGB565 pixels, row-major (length = Width*Height).</summary>
    private readonly ushort[] _pixels;

    public Bitmap(int width, int height)
    {
        Width = width;
        Height = height;
        _pixels = new ushort[width * height];
    }

    /// <summary>
    /// Per-pixel coverage, 0 transparent and 255 opaque, or null when the
    /// image is fully opaque.
    /// </summary>
    /// <remarks>
    /// Null rather than an array of 255s: most images on a device have no
    /// transparency, and an alpha channel nobody set would add 50% to every
    /// bitmap in memory for nothing. Callers ask <see cref="HasAlpha"/>.
    /// </remarks>
    private byte[] _alpha;

    /// <summary>Does this image carry coverage information?</summary>
    public bool HasAlpha => _alpha != null;

    public ushort GetPixel(int x, int y) => _pixels[y * Width + x];

    /// <summary>Coverage at (x, y). Fully opaque unless a channel exists.</summary>
    public byte GetAlpha(int x, int y)
    {
        if (_alpha == null || x < 0 || y < 0 || x >= Width || y >= Height)
        {
            return 255;
        }
        return _alpha[y * Width + x];
    }

    /// <summary>
    /// Set coverage at (x, y), creating the channel on first use.
    /// </summary>
    /// <remarks>
    /// The channel starts fully opaque rather than empty. An image that grows
    /// an alpha channel because one pixel was made transparent must not have
    /// every other pixel disappear.
    /// </remarks>
    public void SetAlpha(int x, int y, byte alpha)
    {
        if (x < 0 || y < 0 || x >= Width || y >= Height)
        {
            return;
        }
        if (_alpha == null)
        {
            if (alpha == 255)
            {
                return;
            }
            _alpha = new byte[Width * Height];
            for (int i = 0; i < _alpha.Length; i++)
            {
                _alpha[i] = 255;
            }
        }
        _alpha[y * Width + x] = alpha;
    }

    /// <summary>Set colour and coverage together.</summary>
    public void SetPixel(int x, int y, ushort rgb565, byte alpha)
    {
        SetPixel(x, y, rgb565);
        SetAlpha(x, y, alpha);
    }

    /// <summary>Drop the alpha channel, making the image fully opaque.</summary>
    public void ClearAlpha()
    {
        _alpha = null;
    }

    public void SetPixel(int x, int y, ushort rgb565)
    {
        if (x >= 0 && y >= 0 && x < Width && y < Height)
        {
            _pixels[y * Width + x] = rgb565;
        }
    }

    public void Clear(ushort rgb565)
    {
        for (int i = 0; i < _pixels.Length; i++)
        {
            _pixels[i] = rgb565;
        }
    }

    /// <summary>Pack an 8-8-8 color into RGB565.</summary>
    public static ushort Rgb565(int r, int g, int b)
    {
        return (ushort)(((r & 0xF8) << 8) | ((g & 0xFC) << 3) | (b >> 3));
    }

    /// <summary>
    /// The coverage channel as bytes, or an empty array when the image is
    /// opaque — the shape <c>Display.DrawImageAlpha</c> takes.
    /// </summary>
    /// <remarks>
    /// This library stays independent of <c>RustNet.Graphics</c>: an image is
    /// data, and a decoder that reaches for the display drags the display into
    /// every program that only wanted to read a file. So drawing is two calls
    /// on the caller's side, and the choice between them is
    /// <see cref="HasAlpha"/>:
    /// <code>
    /// if (bmp.HasAlpha)
    ///     Display.DrawImageAlpha(x, y, bmp.Width, bmp.Height,
    ///         bmp.ToRgb565Bytes(), bmp.ToAlphaBytes());
    /// else
    ///     Display.DrawImage(x, y, bmp.Width, bmp.Height, bmp.ToRgb565Bytes());
    /// </code>
    /// </remarks>
    public byte[] ToAlphaBytes()
    {
        if (_alpha == null)
        {
            return new byte[0];
        }
        byte[] copy = new byte[_alpha.Length];
        for (int i = 0; i < _alpha.Length; i++)
        {
            copy[i] = _alpha[i];
        }
        return copy;
    }

    /// <summary>Little-endian RGB565 byte buffer for Display.DrawImage.</summary>
    public byte[] ToRgb565Bytes()
    {
        byte[] bytes = new byte[_pixels.Length * 2];
        for (int i = 0; i < _pixels.Length; i++)
        {
            bytes[i * 2] = (byte)_pixels[i];
            bytes[i * 2 + 1] = (byte)(_pixels[i] >> 8);
        }
        return bytes;
    }

    /// <summary>Decode an image, sniffing the format from its header.</summary>
    public static Bitmap Decode(byte[] data)
    {
        if (data.Length >= 2 && data[0] == (byte)'B' && data[1] == (byte)'M')
        {
            return Bmp.Decode(data);
        }
        if (data.Length >= 3 && data[0] == (byte)'G' && data[1] == (byte)'I' && data[2] == (byte)'F')
        {
            return Gif.Decode(data);
        }
        // PNG (89 50 4E 47) and JPEG (FF D8 FF) decode in the runtime.
        bool isPng = data.Length >= 4 && data[0] == 0x89 && data[1] == 0x50
            && data[2] == 0x4E && data[3] == 0x47;
        bool isJpeg = data.Length >= 3 && data[0] == 0xFF && data[1] == 0xD8 && data[2] == 0xFF;
        if (isPng || isJpeg)
        {
            return FromRgb565Blob(Native.DecodeRgb565(data));
        }
        throw new System.Exception("unsupported image format (need BMP, GIF, PNG or JPEG)");
    }

    /// <summary>Build a Bitmap from a runtime decode blob
    /// (<c>[width:u16 LE][height:u16 LE][rgb565 LE ...]</c>).</summary>
    private static Bitmap FromRgb565Blob(byte[] blob)
    {
        int w = blob[0] | (blob[1] << 8);
        int h = blob[2] | (blob[3] << 8);
        var bmp = new Bitmap(w, h);
        for (int i = 0; i < w * h; i++)
        {
            int at = 4 + i * 2;
            bmp._pixels[i] = (ushort)(blob[at] | (blob[at + 1] << 8));
        }
        return bmp;
    }
}
