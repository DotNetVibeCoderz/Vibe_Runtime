using System.Runtime.CompilerServices;

namespace RustNet.IO;

/// <summary>
/// Files on a device: named blobs in the board's external memory.
/// </summary>
/// <remarks>
/// <para>
/// Deliberately not <c>FileStream</c>. NOR flash clears bits and cannot set
/// them, so the only way back is erasing a whole sector — and a stream with
/// <c>Seek</c> and partial writes over that would mean read-erase-write of the
/// surrounding sector on every write. That is slow, and it opens a power-loss
/// window over data that was previously fine. Here a file is read and written
/// whole, and the method names say so.
/// </para>
/// <para>
/// Directories are prefixes rather than objects. <see cref="CreateDirectory"/>
/// exists so an empty one can still be listed.
/// </para>
/// <para>
/// A program that needs random access inside a large file wants a database
/// rather than a file; that is what <c>RustNet.Data</c> is for.
/// </para>
/// </remarks>
public static class FileSystem
{
    /// <summary>
    /// Whether this board has storage at all. Everything else here throws
    /// <see cref="IOException"/> when it does not, so a program that can run
    /// either way should ask first.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool IsAvailable();

    /// <summary>
    /// Bytes the store has spoken for, superseded records included. Writing
    /// appends, so this only grows until a compaction; a program watching it
    /// approach the region size is watching for the compaction it will have to
    /// do.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int UsedBytes();

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern bool Exists(string path);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern byte[] ReadAllBytes(string path);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void WriteAllBytes(string path, byte[] data);

    /// <summary>
    /// Reads a file as UTF-8. A file that is not valid UTF-8 throws rather
    /// than coming back with replacement characters where bytes used to be —
    /// a corrupted file presented as a valid one is worse than an error.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string ReadAllText(string path);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void WriteAllText(string path, string text);

    /// <summary>
    /// Appends by reading, concatenating and writing a new record. Right for a
    /// log line, wrong for a megabyte.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void AppendText(string path, string text);

    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Delete(string path);

    /// <summary>
    /// Writes an empty marker so a directory with nothing in it can still be
    /// listed. Directories are prefixes; this does not reserve anything.
    /// </summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void CreateDirectory(string path);

    /// <summary>The immediate children of a prefix.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern string[] List(string path);
}
