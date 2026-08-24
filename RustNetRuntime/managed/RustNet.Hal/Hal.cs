using System.Runtime.CompilerServices;

namespace RustNet.Hal;

/// <summary>
/// How a pin is driven. The values are part of the ABI — the runtime's
/// <c>RustNet.Hal.Gpio::SetMode</c> binding reads these numbers, and a mode it
/// does not recognise is refused rather than treated as <see cref="Input"/>.
/// </summary>
public static class PinMode
{
    public const int Input = 0;
    public const int InputPullUp = 1;
    public const int InputPullDown = 2;
    public const int Output = 3;
    public const int OutputOpenDrain = 4;
}

/// <summary>
/// Digital pins.
/// </summary>
/// <remarks>
/// Every method here reaches the board the firmware installed. On a machine
/// with no board — a desktop running <c>rustnet run</c> without
/// <c>--board</c> — they throw <see cref="InvalidOperationException"/> saying
/// so, rather than pretending the write happened. Check
/// <see cref="Board.IsPresent"/> if the program has to run either way.
/// </remarks>
public static class Gpio
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void SetMode(int pin, int mode);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Write(int pin, bool high);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Read(int pin);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Toggle(int pin);
}

/// <summary>Analogue inputs.</summary>
public static class Adc
{
    /// <summary>The reading at the channel's native resolution.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int ReadRaw(int channel);

    /// <summary>
    /// The same reading in millivolts. Prefer this: raw counts mean nothing
    /// without the resolution and reference voltage of the chip you happen to
    /// be on, and those differ between every board this runs on.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int ReadMillivolts(int channel);
}

/// <summary>Pulse-width modulation: LEDs, motors, servos, buzzers.</summary>
public static class Pwm
{
    /// <summary>
    /// Configures and starts a channel. <paramref name="dutyPermyriad"/> is in
    /// hundredths of a percent, so 5000 is half. Out of 0..10000 is refused —
    /// a duty silently clamped is a motor running at a speed nobody chose.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Configure(int channel, int frequencyHz, int dutyPermyriad);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Stop(int channel);
}

/// <summary>The two-wire bus most sensors speak.</summary>
public static class I2c
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Write(int bus, int address, byte[] data);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] Read(int bus, int address, int length);

    /// <summary>
    /// Writes then reads without letting go of the bus, which is what reading
    /// a sensor register actually is. Doing it as two calls lets another
    /// thread's transfer land in the middle and gives you the wrong register.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] WriteRead(int bus, int address, byte[] command, int length);
}

/// <summary>The four-wire bus displays and flash chips speak.</summary>
public static class Spi
{
    /// <summary>Full duplex: returns as many bytes as were sent.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] Transfer(int bus, byte[] data);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Write(int bus, byte[] data);
}

/// <summary>Serial ports other than the console.</summary>
public static class Uart
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Configure(int port, int baud);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Write(int port, byte[] data);

    /// <summary>
    /// Reads whatever has arrived, up to <paramref name="maxBytes"/>. Does not
    /// block: an empty array means nothing was waiting, which is a normal
    /// answer and not an error.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] Read(int port, int maxBytes);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int Available(int port);
}

/// <summary>Busy-waits shorter than a scheduler tick.</summary>
public static class Delay
{
    /// <summary>
    /// Spins for roughly this many microseconds. For anything above a
    /// millisecond use <c>Thread.Sleep</c>, which lets other work run.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Micros(int micros);
}

/// <summary>
/// The hardware watchdog. Once started, the chip resets unless
/// <see cref="Feed"/> is called within the timeout.
/// </summary>
/// <remarks>
/// On most chips a watchdog cannot be turned off once armed, and
/// <see cref="Stop"/> throws <see cref="NotSupportedException"/> there. That is
/// the chip's rule, not this runtime's.
/// </remarks>
public static class Watchdog
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Start(int timeoutMs);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Feed();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Stop();
}

/// <summary>What is underneath.</summary>
public static class Board
{
    /// <summary>
    /// The board's name, or <c>"none"</c> where no board is installed. Asking
    /// is always allowed — it is the one hardware question a program running
    /// anywhere is entitled to ask.
    /// </summary>
    public static extern string Name
    {
        [MethodImpl(MethodImplOptions.InternalCall)]
        get;
    }

    public static extern bool IsPresent
    {
        [MethodImpl(MethodImplOptions.InternalCall)]
        get;
    }
}

/// <summary>
/// Timed edge trains and pulse measurement on a single pin — the shape an
/// HC-SR04 rangefinder, a DHT22 and an IR remote all need.
/// </summary>
/// <remarks>
/// This is here rather than left to the program because the timings are in
/// microseconds and a managed loop cannot hold them. A `Gpio.Write` followed
/// by `Delay.Micros` is at the mercy of whatever the interpreter or the
/// collector does in between; these run in the HAL, where the board can meet
/// the deadline.
/// </remarks>
public static class Signal
{
    /// <summary>Raw u32 little-endian durations; prefer <see cref="Generate"/>.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void GenerateRaw(int pin, bool initialHigh, byte[] timingsUs);

    /// <summary>Raw u32 little-endian widths; prefer <see cref="Capture"/>.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] CaptureRaw(int pin, int maxEdges, int timeoutUs);

    /// <summary>
    /// Sends a trigger pulse and measures the echo width in microseconds.
    /// Returns 0 on timeout — which is a real answer, meaning nothing came
    /// back, and not an error.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int PulseFeedback(int pin, bool pulseHigh, int pulseUs, int timeoutUs);

    /// <summary>
    /// Drives a timed edge train: the pin starts at <paramref name="initialHigh"/>
    /// and toggles after each duration.
    /// </summary>
    public static void Generate(int pin, bool initialHigh, int[] timingsUs)
    {
        byte[] raw = new byte[timingsUs.Length * 4];
        for (int i = 0; i < timingsUs.Length; i++)
        {
            raw[i * 4] = (byte)timingsUs[i];
            raw[i * 4 + 1] = (byte)(timingsUs[i] >> 8);
            raw[i * 4 + 2] = (byte)(timingsUs[i] >> 16);
            raw[i * 4 + 3] = (byte)(timingsUs[i] >> 24);
        }
        GenerateRaw(pin, initialHigh, raw);
    }

    /// <summary>Measures up to <paramref name="maxEdges"/> pulse widths, in microseconds.</summary>
    public static int[] Capture(int pin, int maxEdges, int timeoutUs)
    {
        byte[] raw = CaptureRaw(pin, maxEdges, timeoutUs);
        int[] widths = new int[raw.Length / 4];
        for (int i = 0; i < widths.Length; i++)
        {
            widths[i] = raw[i * 4]
                | (raw[i * 4 + 1] << 8)
                | (raw[i * 4 + 2] << 16)
                | (raw[i * 4 + 3] << 24);
        }
        return widths;
    }
}
