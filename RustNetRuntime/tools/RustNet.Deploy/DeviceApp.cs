namespace RustNet.Deploy;

/// <summary>
/// The application a firmware image carries, and the libraries it references.
/// </summary>
/// <remarks>
/// <para>
/// A board has no filesystem to resolve an <c>AssemblyRef</c> against, so
/// everything a program needs is compiled into the firmware. The build reads
/// the paths from the environment — <c>RUSTCLR_APP</c> and
/// <c>RUSTCLR_LIB1</c>..<c>LIB3</c> — and <c>include_bytes!</c> puts them in
/// <c>.rodata</c>.
/// </para>
/// <para>
/// Without this, a build carries whatever <c>HelloWorld.dll</c> is checked in
/// beside the firmware crate. That is a sensible default for proving a board
/// runs at all and a surprising one for somebody who pressed Deploy on their
/// own project: the flash succeeds, the board boots, and it runs a different
/// program. It has already happened once.
/// </para>
/// </remarks>
public sealed record DeviceApp
{
    /// <summary>The assembly holding the entry point.</summary>
    public required string AssemblyPath { get; init; }

    /// <summary>
    /// The assemblies it references, in dependency order — a library has to be
    /// loaded before the program whose `TypeRef`s bind to it.
    /// </summary>
    /// <remarks>
    /// Eight at most. <c>include_bytes!</c> needs one literal per file and the
    /// list is written out in each firmware's <c>main.rs</c>, so a ninth is a
    /// change to the firmware rather than a bigger number here. A program that
    /// needs more is refused at build time rather than failing to resolve a
    /// token on the board.
    /// </remarks>
    public IReadOnlyList<string> LibraryPaths { get; init; } = [];

    /// <summary>How many libraries a firmware image can carry.</summary>
    public const int MaxLibraries = 8;

    /// <summary>
    /// The environment the firmware build reads these from, or an error saying
    /// why this application cannot be carried.
    /// </summary>
    public bool TryToEnvironment(
        out IReadOnlyDictionary<string, string> environment,
        out string? error)
    {
        environment = new Dictionary<string, string>();
        error = null;

        if (!File.Exists(AssemblyPath))
        {
            error = $"the application does not exist: {AssemblyPath}";
            return false;
        }
        if (LibraryPaths.Count > MaxLibraries)
        {
            error = $"{LibraryPaths.Count} libraries, and a firmware image carries "
                  + $"{MaxLibraries}. Reference fewer, or widen the list in the firmware's main.rs.";
            return false;
        }

        var env = new Dictionary<string, string>
        {
            ["RUSTCLR_APP"] = Path.GetFullPath(AssemblyPath),
        };
        for (int i = 0; i < LibraryPaths.Count; i++)
        {
            string library = LibraryPaths[i];
            if (!File.Exists(library))
            {
                error = $"a referenced library does not exist: {library}";
                return false;
            }
            env[$"RUSTCLR_LIB{i + 1}"] = Path.GetFullPath(library);
        }

        environment = env;
        return true;
    }

    /// <summary>
    /// The application a project built, with every <c>RustNet.*</c> library
    /// beside it.
    /// </summary>
    /// <remarks>
    /// The device libraries are separate assemblies so a program pays only for
    /// what it references, and the SDK copies exactly those next to the output.
    /// So "what is beside the program" is already the right list — no project
    /// file has to be parsed to work it out.
    /// </remarks>
    public static DeviceApp FromBuildOutput(string assemblyPath)
    {
        string? directory = Path.GetDirectoryName(Path.GetFullPath(assemblyPath));
        string[] libraries = directory is null
            ? []
            : Directory
                .EnumerateFiles(directory, "RustNet.*.dll")
                .OrderBy(path => path, StringComparer.OrdinalIgnoreCase)
                .ToArray();
        return new DeviceApp { AssemblyPath = assemblyPath, LibraryPaths = libraries };
    }
}
