using System.Runtime.CompilerServices;

namespace RustNet.Media;

/// <summary>
/// VNC (RFB) server that streams the device framebuffer over TCP so a desktop
/// VNC client can watch the display remotely (v0.8). Serves security type
/// <c>None</c>, a 32bpp true-colour pixel format and Raw full-frame updates.
/// The server runs in the background — start it once and keep drawing.
/// </summary>
public static class Vnc
{
    /// <summary>Start the RFB server on <paramref name="port"/> (e.g. 5900).
    /// Returns true if the server is (already) running.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Start(int port);

    /// <summary>Stop the server (existing clients drop on their next poll).</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Stop();

    /// <summary>Whether the server is currently running.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool IsRunning();
}
