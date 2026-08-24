namespace RustNet.Drawing;

/// <summary>
/// Windows BMP decoder — uncompressed 24-bit and 32-bit (BITMAPINFOHEADER),
/// bottom-up or top-down. The common export format for embedded assets.
/// </summary>
public static class Bmp
{
    public static Bitmap Decode(byte[] d)
    {
        if (d.Length < 54 || d[0] != 'B' || d[1] != 'M')
        {
            throw new System.Exception("not a BMP");
        }
        int dataOffset = ReadI32(d, 10);
        int headerSize = ReadI32(d, 14);
        int width = ReadI32(d, 18);
        int rawHeight = ReadI32(d, 22);
        int bpp = ReadU16(d, 28);
        int compression = ReadI32(d, 30);

        if (compression != 0)
        {
            throw new System.Exception("compressed BMP not supported");
        }
        if (bpp != 24 && bpp != 32)
        {
            throw new System.Exception("BMP must be 24- or 32-bit");
        }

        bool topDown = rawHeight < 0;
        int height = topDown ? -rawHeight : rawHeight;
        var bmp = new Bitmap(width, height);

        int bytesPerPixel = bpp / 8;
        bool anyCoverage = false;
        // Rows are padded to a 4-byte boundary.
        int rowSize = ((width * bytesPerPixel + 3) / 4) * 4;

        for (int row = 0; row < height; row++)
        {
            int srcRow = topDown ? row : (height - 1 - row);
            int rowStart = dataOffset + srcRow * rowSize;
            for (int x = 0; x < width; x++)
            {
                int p = rowStart + x * bytesPerPixel;
                if (p + 2 >= d.Length)
                {
                    continue;
                }
                int b = d[p];
                int g = d[p + 1];
                int r = d[p + 2];
                bmp.SetPixel(x, row, Bitmap.Rgb565(r, g, b));
                // The fourth byte of a 32-bit BMP is coverage. It used to be
                // stepped over, so a file with transparency decoded into a
                // fully opaque image and the loss was silent.
                //
                // Not every 32-bit BMP means it: plenty of writers set the
                // byte to zero for "unused", and honouring that literally
                // turns a whole image invisible. So an image whose alpha is
                // zero everywhere is read as having none — the interpretation
                // that can be recovered from, since a caller can still set
                // coverage itself.
                if (bytesPerPixel == 4 && p + 3 < d.Length)
                {
                    bmp.SetAlpha(x, row, d[p + 3]);
                    if (d[p + 3] != 0)
                    {
                        anyCoverage = true;
                    }
                }
            }
        }
        if (bytesPerPixel == 4 && !anyCoverage)
        {
            // Every pixel claimed to be fully transparent, which no writer
            // means. Treat the channel as absent rather than draw nothing.
            bmp.ClearAlpha();
        }
        _ = headerSize;
        return bmp;
    }

    private static int ReadI32(byte[] d, int o)
        => d[o] | (d[o + 1] << 8) | (d[o + 2] << 16) | (d[o + 3] << 24);

    private static int ReadU16(byte[] d, int o) => d[o] | (d[o + 1] << 8);
}
