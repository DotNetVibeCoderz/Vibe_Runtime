using System.Collections.Generic;

namespace RustNet.Drawing;

/// <summary>
/// GIF decoder (87a/89a) — decodes the first image frame: global/local
/// color table + LZW-compressed indices. Interlaced frames are
/// de-interlaced. Sized for small embedded assets.
/// </summary>
public static class Gif
{
    public static Bitmap Decode(byte[] d)
    {
        if (d.Length < 13 || d[0] != 'G' || d[1] != 'I' || d[2] != 'F')
        {
            throw new System.Exception("not a GIF");
        }
        int screenW = d[6] | (d[7] << 8);
        int screenH = d[8] | (d[9] << 8);
        int packed = d[10];
        bool hasGct = (packed & 0x80) != 0;
        int gctSize = 2 << (packed & 0x07);

        int pos = 13;
        ushort[] gct = new ushort[0];
        if (hasGct)
        {
            gct = ReadColorTable(d, pos, gctSize);
            pos += gctSize * 3;
        }

        // -1 until a Graphic Control Extension says otherwise.
        int transparentIndex = -1;

        // Walk blocks until the first image descriptor (0x2C).
        while (pos < d.Length)
        {
            int b = d[pos];
            if (b == 0x2C)
            {
                return DecodeImage(d, pos, screenW, screenH, gct, transparentIndex);
            }
            if (b == 0x21)
            {
                // Graphic Control Extension (0xF9) carries the transparent
                // colour index; every other extension is metadata this
                // decoder has no use for. Skipping all of them, which is what
                // this did, is what made GIF transparency vanish.
                if (pos + 3 < d.Length && d[pos + 1] == 0xF9)
                {
                    int flags = d[pos + 3];
                    // Bit 0 says whether the index that follows means
                    // anything. Without that check, index 0 reads as
                    // transparent in every GIF that does not use it.
                    transparentIndex = (flags & 0x01) != 0 && pos + 6 < d.Length
                        ? d[pos + 6] : -1;
                }
                pos += 2;
                pos = SkipSubBlocks(d, pos);
            }
            else if (b == 0x3B)
            {
                break; // trailer
            }
            else
            {
                pos++;
            }
        }
        throw new System.Exception("no image frame in GIF");
    }

    private static Bitmap DecodeImage(byte[] d, int pos, int screenW, int screenH,
        ushort[] gct, int transparentIndex)
    {
        // Image descriptor.
        int imgW = d[pos + 5] | (d[pos + 6] << 8);
        int imgH = d[pos + 7] | (d[pos + 8] << 8);
        int packed = d[pos + 9];
        bool interlaced = (packed & 0x40) != 0;
        bool hasLct = (packed & 0x80) != 0;
        pos += 10;

        ushort[] colors = gct;
        if (hasLct)
        {
            int lctSize = 2 << (packed & 0x07);
            colors = ReadColorTable(d, pos, lctSize);
            pos += lctSize * 3;
        }

        int minCodeSize = d[pos];
        pos++;

        // Gather the LZW data sub-blocks into one buffer.
        List<byte> lzw = new List<byte>();
        while (pos < d.Length && d[pos] != 0)
        {
            int len = d[pos];
            pos++;
            for (int i = 0; i < len && pos < d.Length; i++)
            {
                lzw.Add(d[pos]);
                pos++;
            }
        }

        byte[] indices = LzwDecode(lzw, minCodeSize, imgW * imgH);
        int w = imgW > 0 ? imgW : screenW;
        int h = imgH > 0 ? imgH : screenH;
        var bmp = new Bitmap(w, h);

        if (interlaced)
        {
            int src = 0;
            for (int pass = 0; pass < 4; pass++)
            {
                // GIF interlace passes: (start, step) = (0,8)(4,8)(2,4)(1,2).
                int start = pass == 0 ? 0 : pass == 1 ? 4 : pass == 2 ? 2 : 1;
                int step = pass < 2 ? 8 : pass == 2 ? 4 : 2;
                for (int y = start; y < h; y += step)
                {
                    for (int x = 0; x < w; x++)
                    {
                        SetIndex(bmp, colors, indices, src, x, y, transparentIndex);
                        src++;
                    }
                }
            }
        }
        else
        {
            int src = 0;
            for (int y = 0; y < h; y++)
            {
                for (int x = 0; x < w; x++)
                {
                    SetIndex(bmp, colors, indices, src, x, y, transparentIndex);
                    src++;
                }
            }
        }
        return bmp;
    }

    private static void SetIndex(Bitmap bmp, ushort[] colors, byte[] indices, int src,
        int x, int y, int transparentIndex)
    {
        if (src < indices.Length)
        {
            int idx = indices[src];
            if (idx < colors.Length)
            {
                bmp.SetPixel(x, y, colors[idx]);
            }
            if (idx == transparentIndex)
            {
                // The colour is still written: a transparent pixel in a GIF
                // has a palette entry, and keeping it means an application
                // that flattens the image later gets the author's background
                // rather than black.
                bmp.SetAlpha(x, y, 0);
            }
        }
    }

    private static ushort[] ReadColorTable(byte[] d, int pos, int count)
    {
        ushort[] table = new ushort[count];
        for (int i = 0; i < count; i++)
        {
            int p = pos + i * 3;
            table[i] = Bitmap.Rgb565(d[p], d[p + 1], d[p + 2]);
        }
        return table;
    }

    private static int SkipSubBlocks(byte[] d, int pos)
    {
        while (pos < d.Length && d[pos] != 0)
        {
            pos += d[pos] + 1;
        }
        return pos + 1;
    }

    /// <summary>Variable-width LZW decode (GIF flavor: clear + EOI codes).</summary>
    private static byte[] LzwDecode(List<byte> data, int minCodeSize, int expectedPixels)
    {
        int clearCode = 1 << minCodeSize;
        int eoiCode = clearCode + 1;
        int codeSize = minCodeSize + 1;

        List<byte[]> dict = new List<byte[]>();
        ResetDict(dict, clearCode);

        List<byte> outBytes = new List<byte>(expectedPixels);
        int bitPos = 0;
        byte[] prev = null;

        while (true)
        {
            int code = ReadCode(data, bitPos, codeSize);
            if (code < 0)
            {
                break;
            }
            bitPos += codeSize;

            if (code == clearCode)
            {
                ResetDict(dict, clearCode);
                codeSize = minCodeSize + 1;
                prev = null;
                continue;
            }
            if (code == eoiCode)
            {
                break;
            }

            byte[] entry;
            if (code < dict.Count)
            {
                entry = dict[code];
            }
            else if (prev != null)
            {
                entry = Append(prev, prev[0]);
            }
            else
            {
                break;
            }

            outBytes.AddRange(entry);

            if (prev != null)
            {
                dict.Add(Append(prev, entry[0]));
                // Widen the code as the dictionary fills (max 12 bits).
                if (dict.Count == (1 << codeSize) && codeSize < 12)
                {
                    codeSize++;
                }
            }
            prev = entry;

            if (outBytes.Count >= expectedPixels)
            {
                break;
            }
        }

        return outBytes.ToArray();
    }

    private static void ResetDict(List<byte[]> dict, int clearCode)
    {
        dict.Clear();
        for (int i = 0; i < clearCode; i++)
        {
            dict.Add(new byte[] { (byte)i });
        }
        dict.Add(new byte[0]); // clear code slot
        dict.Add(new byte[0]); // EOI slot
    }

    private static byte[] Append(byte[] a, byte b)
    {
        byte[] r = new byte[a.Length + 1];
        for (int i = 0; i < a.Length; i++)
        {
            r[i] = a[i];
        }
        r[a.Length] = b;
        return r;
    }

    /// <summary>Read a little-endian bit-packed code, or -1 past the end.</summary>
    private static int ReadCode(List<byte> data, int bitPos, int codeSize)
    {
        int value = 0;
        for (int i = 0; i < codeSize; i++)
        {
            int bit = bitPos + i;
            int byti = bit >> 3;
            if (byti >= data.Count)
            {
                return -1;
            }
            int b = (data[byti] >> (bit & 7)) & 1;
            value |= b << i;
        }
        return value;
    }
}
