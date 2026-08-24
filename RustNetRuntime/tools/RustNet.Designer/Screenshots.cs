using System;
using System.IO;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;

namespace RustNet.Designer;

/// <summary>
/// Renders the designer to PNG files, so the documentation shows the real
/// window rather than a drawing of it.
/// </summary>
/// <remarks>
/// <para>
/// The same job as CodeGen's and the Workbench's, done differently because
/// this is WPF rather than Avalonia: there is no headless platform to swap in,
/// so the window is shown at a large negative offset and captured with
/// <see cref="RenderTargetBitmap"/>. It is genuinely on screen for a moment,
/// several thousand pixels off the left edge of it.
/// </para>
/// <para>
/// Off-screen rather than never shown, because a WPF window that has not been
/// shown has no composition target: `Measure`/`Arrange` alone produce a bitmap
/// of the chrome with an empty client area, which looks like a broken theme
/// rather than a missing step.
/// </para>
/// </remarks>
internal static class Screenshots
{
    private const int Width = 1440;
    private const int Height = 820;

    /// <summary>Far enough left that no real monitor arrangement reaches it.</summary>
    private const double OffScreenLeft = -20_000;

    public static int Capture(string directory)
    {
        Directory.CreateDirectory(directory);

        try
        {
            var path = Path.Combine(directory, "designer-main.png");
            Render(path);
            Console.WriteLine($"wrote {path}");
            Console.WriteLine("1 of 1 screenshots rendered.");
            return 0;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"designer-main.png: {ex.Message}");
            return 1;
        }
    }

    private static void Render(string path)
    {
        var window = new MainWindow
        {
            Width = Width,
            Height = Height,
            WindowStartupLocation = WindowStartupLocation.Manual,
            Left = OffScreenLeft,
            Top = 0,
            ShowInTaskbar = false,
        };

        window.Show();

        // Let bindings settle and the first layout pass complete before the
        // frame is grabbed; otherwise the capture catches an empty window.
        // `Loaded` is not enough on its own — the visual tree exists there but
        // the first render pass has not run.
        for (var i = 0; i < 8; i++)
        {
            Drain();
        }
        window.UpdateLayout();
        Drain();

        // The *content*, not the window: `window` includes the OS chrome, whose
        // size is a property of whoever is looking at the screenshot rather
        // than of the designer.
        var content = (Visual)window.Content;
        var bounds = VisualTreeHelper.GetDescendantBounds(content);
        var width = (int)Math.Ceiling(bounds.Width);
        var height = (int)Math.Ceiling(bounds.Height);
        if (width <= 0 || height <= 0)
        {
            throw new InvalidOperationException(
                "the window laid out to nothing, so there is no frame to capture");
        }

        var bitmap = new RenderTargetBitmap(width, height, 96, 96, PixelFormats.Pbgra32);
        bitmap.Render(content);

        var encoder = new PngBitmapEncoder();
        encoder.Frames.Add(BitmapFrame.Create(bitmap));
        using (var stream = File.Create(path))
        {
            encoder.Save(stream);
        }

        window.Close();
        Drain();
    }

    /// <summary>Run the dispatcher until it has nothing left at render priority.</summary>
    private static void Drain()
    {
        var frame = new DispatcherFrame();
        Dispatcher.CurrentDispatcher.BeginInvoke(
            DispatcherPriority.ContextIdle,
            new Action(() => frame.Continue = false));
        Dispatcher.PushFrame(frame);
    }
}
