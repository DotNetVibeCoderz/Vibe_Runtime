using System.Runtime.CompilerServices;

namespace RustNet.Usb;

/// <summary>
/// USB host role: enumerate an attached USB device and exchange bulk data
/// with it (v0.8, chip-gated). The host matches a class driver (CDC/HID/MSC)
/// against the device descriptor. On the virtual device it talks to the
/// peripheral configured via <see cref="UsbClient"/> over the USB simulator.
/// </summary>
public static class UsbHost
{
    /// <summary>Enumerate the attached device. Returns
    /// <c>"vid:pid:class:product"</c> (hex ids; class one of
    /// cdc/hid/msc/vendor), or empty if nothing is attached/matched.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Enumerate();

    /// <summary>Send a bulk-OUT transfer to the attached device.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void BulkOut(byte[] data);

    /// <summary>Read a bulk-IN transfer from the attached device.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] BulkIn();
}
