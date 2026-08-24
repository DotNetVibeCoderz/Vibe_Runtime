using System.Runtime.CompilerServices;

namespace RustNet.Sys;

/// <summary>Monotonic time since device boot.</summary>
public static class Uptime
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern long Ms();
}

/// <summary>Identity and health of the device the app is running on.</summary>
public static class DeviceInfo
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Chip();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Board();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Version();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern long UptimeMs();

    /// <summary>Full device summary as JSON (chip, board, version, uptime).</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Json();
}

/// <summary>
/// Power management: sleep modes, wake sources (GPIO edge / RTC alarm),
/// shutdown and reset. Arm wake sources before sleeping or shutting down.
/// </summary>
public static class Power
{
    public const int Light = 0;
    public const int Deep = 1;
    public const int Hibernate = 2;

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int BatteryMillivolts();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Sleep(int mode, int durationMs);

    /// <summary>Wake on a GPIO edge (true = rising) during the next sleep/shutdown.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void ArmWakeGpio(int pin, bool rising);

    /// <summary>Wake via RTC alarm after the given number of seconds.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void ArmWakeRtc(int seconds);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void ClearWakeSources();

    /// <summary>Why the chip is running: power-on, rtc-alarm, gpio, watchdog-reset, ...</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string WakeReason();

    /// <summary>Reboot the device. On the virtual device this halts the app.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Reset();

    /// <summary>Power off entirely; only armed wake sources bring it back.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Shutdown();
}

/// <summary>Battery-backed real-time clock (epoch = seconds since 1970 UTC).</summary>
public static class Rtc
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern long Epoch();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Set(long epoch);

    /// <summary>Current time as "YYYY-MM-DD HH:MM:SS".</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string NowString();

    /// <summary>Arm the RTC alarm at an absolute epoch second (deep-sleep wake source).</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void SetAlarm(long epoch);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void ClearAlarm();
}

/// <summary>Hardware watchdog: once started, feed it within the timeout or the chip resets.</summary>
public static class Watchdog
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Start(int timeoutMs);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Feed();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Stop();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool IsRunning();
}

/// <summary>
/// External memories hanging off the MCU. Index 0 is QSPI flash (erase
/// before rewrite), index 1 is SDRAM on boards that have it.
/// </summary>
public static class ExtMemory
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int Size(int index);

    /// <summary>"qspi-flash" or "sdram".</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Kind(int index);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] Read(int index, int address, int length);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Write(int index, int address, byte[] data);

    /// <summary>Erase to 0xFF (QSPI flash only); rounds to sector boundaries.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Erase(int index, int address, int length);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int SectorSize(int index);
}
