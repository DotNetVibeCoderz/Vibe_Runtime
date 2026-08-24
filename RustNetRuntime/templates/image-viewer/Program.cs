using RustNet.Drawing;
using RustNet.Graphics;
using RustNet.Resources;

namespace __NAME__;

/// <summary>Loads an embedded GIF and displays it — no filesystem, no
/// base64: the asset ships inside the RNX via RustNet.Resources.</summary>
public static class Program
{
    public static void Main()
    {
        Console.WriteLine("image-viewer starting");
        byte[] bytes = Resource.GetBytes("logo.gif");
        Bitmap logo = Bitmap.Decode(bytes);
        Console.WriteLine(string.Concat("logo ", logo.Width.ToString(), "x", logo.Height.ToString()));

        Display.Init(160, 128);
        Display.Clear(Color.Black);
        Display.DrawImage((160 - logo.Width) / 2, (128 - logo.Height) / 2,
            logo.Width, logo.Height, logo.ToRgb565Bytes());
        Display.Present();
        Console.WriteLine("image-viewer done");
    }
}
