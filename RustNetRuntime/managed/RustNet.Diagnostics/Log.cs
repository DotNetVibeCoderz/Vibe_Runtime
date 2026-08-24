using System.Runtime.CompilerServices;

namespace RustNet.Diagnostics;

/// <summary>
/// Structured log channel. Records land in the device ring buffer and every
/// attached sink (serial console, RNDP log stream, remote MQTT/HTTP).
/// </summary>
public static class Log
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Info(string message);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Warn(string message);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Error(string message);
}
