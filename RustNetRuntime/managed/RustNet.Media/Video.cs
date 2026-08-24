using System.Runtime.CompilerServices;
using System.Collections.Generic;

namespace RustNet.Media;

/// <summary>
/// Motion-JPEG video (v0.8). A clip is a sequence of JPEG frames, each
/// length-prefixed (u32 little-endian) — the simplest streamable container.
/// Frames are JPEG-encoded by the runtime (<see cref="EncodeJpeg"/>); a
/// captured RGB565 camera frame goes straight in, and playback decodes each
/// frame back to a <c>RustNet.Drawing.Bitmap</c>.
/// </summary>
public static class Video
{
    /// <summary>JPEG-encode an RGB565 frame (little-endian) at the given
    /// quality (1–100). Returns the JPEG bytes.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] EncodeJpeg(byte[] rgb565, int width, int height, int quality);
}

/// <summary>Builds an MJPEG clip frame by frame.</summary>
public class MjpegWriter
{
    private readonly List<byte> _buf = new List<byte>();

    public int FrameCount { get; private set; }

    /// <summary>Encode an RGB565 frame to JPEG and append it.</summary>
    public void AddFrame(byte[] rgb565, int width, int height, int quality)
    {
        AddJpegFrame(Video.EncodeJpeg(rgb565, width, height, quality));
    }

    /// <summary>Append an already-encoded JPEG frame.</summary>
    public void AddJpegFrame(byte[] jpeg)
    {
        int len = jpeg.Length;
        _buf.Add((byte)(len & 0xFF));
        _buf.Add((byte)((len >> 8) & 0xFF));
        _buf.Add((byte)((len >> 16) & 0xFF));
        _buf.Add((byte)((len >> 24) & 0xFF));
        for (int i = 0; i < jpeg.Length; i++)
        {
            _buf.Add(jpeg[i]);
        }
        FrameCount = FrameCount + 1;
    }

    /// <summary>The whole clip as a byte buffer.</summary>
    public byte[] ToBytes()
    {
        byte[] outBytes = new byte[_buf.Count];
        for (int i = 0; i < _buf.Count; i++)
        {
            outBytes[i] = _buf[i];
        }
        return outBytes;
    }
}

/// <summary>Reads an MJPEG clip into its individual JPEG frames.</summary>
public class MjpegReader
{
    private readonly List<byte[]> _frames = new List<byte[]>();

    public MjpegReader(byte[] data)
    {
        int i = 0;
        while (i + 4 <= data.Length)
        {
            int len = data[i] | (data[i + 1] << 8) | (data[i + 2] << 16) | (data[i + 3] << 24);
            i = i + 4;
            if (len < 0 || i + len > data.Length)
            {
                break;
            }
            byte[] frame = new byte[len];
            for (int j = 0; j < len; j++)
            {
                frame[j] = data[i + j];
            }
            _frames.Add(frame);
            i = i + len;
        }
    }

    /// <summary>Number of frames in the clip.</summary>
    public int Count => _frames.Count;

    /// <summary>The JPEG bytes of frame <paramref name="index"/>
    /// (decode with <c>Bitmap.Decode</c>).</summary>
    public byte[] Frame(int index) => _frames[index];
}
