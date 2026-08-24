using System.Runtime.CompilerServices;

namespace RustNet.Resources;

/// <summary>
/// Assets bundled into the app and available at runtime. Add files as
/// <c>&lt;EmbeddedResource&gt;</c> items in the csproj; the MetadataProcessor
/// copies each manifest resource into the RNX module, and these calls read
/// it back on the device by name.
///
/// The resource name is the .NET manifest name — typically
/// <c>&lt;RootNamespace&gt;.&lt;Folder&gt;.&lt;File&gt;</c> (e.g.
/// <c>MyApp.assets.logo.gif</c>). Use <see cref="Exists"/> to check.
/// </summary>
public static class Resource
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Exists(string name);

    /// <summary>Raw bytes of an embedded resource (e.g. a BMP/GIF to decode).</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] GetBytes(string name);

    /// <summary>An embedded resource decoded as a UTF-8 string (e.g. a UI XML layout).</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string GetString(string name);
}
