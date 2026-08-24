using RustNet.Graphics;
using RustNet.Threading;

namespace __NAME__;

/// <summary>
/// Display test pattern: primitives, text scaling, color bars and a
/// bouncing-ball animation (double-buffered).
/// </summary>
public static class Program
{
    public static void Main()
    {
        Console.WriteLine("__NAME__ display test");
        Display.Init(160, 128);

        // Color bars
        int[] colors = new int[8];
        colors[0] = Color.White;
        colors[1] = Color.Yellow;
        colors[2] = Color.Cyan;
        colors[3] = Color.Green;
        colors[4] = Color.Magenta;
        colors[5] = Color.Red;
        colors[6] = Color.Blue;
        colors[7] = Color.Black;
        for (int i = 0; i < 8; i++)
        {
            Display.FillRect(i * 20, 0, 20, 40, colors[i]);
        }

        // Primitives
        Display.DrawRect(4, 46, 40, 30, Color.White);
        Display.FillRect(50, 46, 40, 30, Color.Green);
        Display.DrawCircle(115, 61, 15, Color.Cyan);
        Display.DrawLine(0, 127, 159, 80, Color.Red);

        // Text at two scales
        Display.DrawText(4, 82, "RustNet", Color.White, 2);
        Display.DrawText(4, 100, "graphics test OK", Color.Yellow, 1);
        Display.Present();
        Console.WriteLine("static pattern drawn");
        Sleep.Ms(500);

        // Bouncing ball animation
        int x = 20;
        int y = 60;
        int dx = 3;
        int dy = 2;
        for (int frame = 0; frame < 60; frame++)
        {
            Display.FillRect(0, 40, 160, 88, Color.Black);
            Display.FillRect(x - 4, y - 4, 8, 8, Color.Magenta);
            Display.DrawText(4, 116, string.Concat("frame ", frame.ToString()), Color.White, 1);
            Display.Present();
            x = x + dx;
            y = y + dy;
            if (x < 8 || x > 152)
            {
                dx = -dx;
            }
            if (y < 48 || y > 116)
            {
                dy = -dy;
            }
            Sleep.Ms(30);
        }
        Console.WriteLine("animation done");
    }
}
