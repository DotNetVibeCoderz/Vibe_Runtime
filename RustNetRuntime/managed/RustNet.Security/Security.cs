using System.Runtime.CompilerServices;

namespace RustNet.Security;

/// <summary>Digests.</summary>
public static class Hash
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] Sha256(byte[] data);

    /// <summary>The same digest as lower-case hex, for logging or comparison.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Sha256Hex(byte[] data);

    /// <summary>
    /// CRC-32 (IEEE 802.3), for noticing corruption.
    /// </summary>
    /// <remarks>
    /// Not a security check. A CRC is trivially forgeable — anything that has
    /// to survive an attacker rather than a bad cable uses <see cref="Hmac"/>.
    /// </remarks>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int Crc32(byte[] data);
}

/// <summary>Message authentication with HMAC-SHA256.</summary>
public static class Hmac
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] Compute(byte[] key, byte[] message);

    /// <summary>
    /// Checks a tag in constant time.
    /// </summary>
    /// <remarks>
    /// Use this rather than comparing <see cref="Compute"/>'s result yourself.
    /// A normal comparison stops at the first differing byte, and how long it
    /// took says where that byte was — which is enough to forge a tag one byte
    /// at a time. That is why this exists as a method instead of leaving the
    /// comparison to the caller.
    /// </remarks>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Verify(byte[] key, byte[] message, byte[] expectedTag);

    /// <summary>
    /// The tag over a UTF-8 string, base64-encoded — the shape cloud IoT SAS
    /// tokens and JWTs are built from.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string Sha256Base64(byte[] key, string data);
}

/// <summary>Symmetric encryption with AES in counter mode.</summary>
/// <remarks>
/// <para>
/// One method for both directions, because counter mode is its own inverse.
/// Two names would suggest a distinction that does not exist.
/// </para>
/// <para>
/// <b>A (key, counter) pair must never be used twice.</b> Reusing one XORs the
/// two plaintexts together and both can be recovered from the ciphertexts
/// alone. The runtime cannot check this — it does not know what you encrypted
/// before — so the responsibility is yours, and it is written here rather than
/// left to be discovered.
/// </para>
/// <para>
/// Keys are 16 or 32 bytes (AES-128 or AES-256). A key of any other length is
/// refused rather than padded: a five-byte password silently stretched into a
/// 128-bit key is a five-byte password.
/// </para>
/// </remarks>
public static class Cipher
{
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] Transform(byte[] key, byte[] counterBlock, byte[] data);
}
