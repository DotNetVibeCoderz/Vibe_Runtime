using System;
using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Interop;

namespace RustNet.Designer;

/// <summary>
/// Asks Windows for the dark title bar. Without it a graphite tool sits under a
/// bright white caption, which is the one seam the theme cannot paint over.
/// Best-effort: unsupported builds simply keep the light caption.
/// </summary>
internal static class DarkTitleBar
{
    private const int DwmwaUseImmersiveDarkMode = 20;

    [DllImport("dwmapi.dll", SetLastError = true)]
    private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attribute, ref int value, int size);

    /// <summary>Call once the window has an HWND (SourceInitialized or later).</summary>
    public static void Apply(Window window)
    {
        try
        {
            IntPtr hwnd = new WindowInteropHelper(window).Handle;
            if (hwnd == IntPtr.Zero)
            {
                return;
            }
            int on = 1;
            DwmSetWindowAttribute(hwnd, DwmwaUseImmersiveDarkMode, ref on, sizeof(int));
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine("dark title bar unavailable: " + ex.Message);
        }
    }
}
