using System.Runtime.CompilerServices;

namespace RustNet.Threading;

/// <summary>
/// Delays.
/// </summary>
/// <remarks>
/// Pure C# over <see cref="System.Threading.Thread.Sleep(int)"/> rather than
/// its own runtime call: RustCLR already has a scheduler and a real
/// <c>Thread.Sleep</c>, and a second path into it would be a second thing that
/// could disagree about what "sleep" means. This type exists because the
/// device templates say <c>Sleep.Ms</c>, not because the runtime needs it to.
/// <para>
/// For waits shorter than a scheduler tick use
/// <see cref="RustNet.Hal.Delay.Micros"/>, which busy-waits instead.
/// </para>
/// </remarks>
public static class Sleep
{
    public static void Ms(int milliseconds)
    {
        if (milliseconds > 0)
        {
            System.Threading.Thread.Sleep(milliseconds);
        }
    }

    public static void Seconds(int seconds) => Ms(seconds * 1000);
}
