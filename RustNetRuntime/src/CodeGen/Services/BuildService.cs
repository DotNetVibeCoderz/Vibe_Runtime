using CodeGen.Models;

namespace CodeGen.Services;

/// <summary>Where a program should run.</summary>
public enum RunTarget
{
    /// <summary>The reference .NET runtime.</summary>
    Dotnet,
    /// <summary>RustCLR, through the rustnet toolchain.</summary>
    RustClr,
}

/// <summary>
/// Compiling, running and deploying the open project.
///
/// Building always goes through the .NET SDK — RustCLR consumes IL, it does not
/// compile C#. Running can go either way, which is the point: the same
/// assembly can be checked against both runtimes from one keystroke.
/// </summary>
public sealed class BuildService(ProjectService projects, ProcessRunner runner)
{
    public string Configuration { get; set; } = "Release";

    public async Task<ProcessResult> BuildAsync(
        AppSettings settings,
        Action<string> log,
        CancellationToken cancellationToken = default)
    {
        var target = projects.FindBuildTarget();
        if (target is null)
        {
            log("No .csproj or .sln found in the open project.");
            return new ProcessResult(-1, "", "no build target", TimeSpan.Zero);
        }

        log($"$ dotnet build -c {Configuration} \"{Path.GetFileName(target)}\"");
        return await runner.RunAsync(
            settings.DotnetPath,
            ["build", "-c", Configuration, "--nologo", target],
            projects.CurrentProjectPath,
            log,
            log,
            cancellationToken).ConfigureAwait(false);
    }

    public async Task<ProcessResult> RunAsync(
        AppSettings settings,
        RunTarget target,
        Action<string> log,
        CancellationToken cancellationToken = default)
    {
        var build = await BuildAsync(settings, log, cancellationToken).ConfigureAwait(false);
        if (!build.Succeeded)
        {
            log("Build failed; not running.");
            return build;
        }

        var assembly = projects.FindOutputAssembly(Configuration);
        if (assembly is null)
        {
            log($"Build succeeded but no assembly was found under bin/{Configuration}.");
            return new ProcessResult(-1, "", "no output assembly", TimeSpan.Zero);
        }

        if (target == RunTarget.Dotnet)
        {
            log($"$ dotnet \"{Path.GetFileName(assembly)}\"");
            return await runner.RunAsync(
                settings.DotnetPath,
                [assembly],
                Path.GetDirectoryName(assembly),
                log,
                log,
                cancellationToken).ConfigureAwait(false);
        }

        var rustnet = ResolveRustNet(settings);
        log($"$ {Path.GetFileName(rustnet)} run \"{Path.GetFileName(assembly)}\" --stats");
        var result = await runner.RunAsync(
            rustnet,
            ["run", assembly, "--stats"],
            Path.GetDirectoryName(assembly),
            log,
            log,
            cancellationToken).ConfigureAwait(false);

        if (result.ExitCode == -1 && result.StandardError.Contains("Could not start"))
        {
            log("The rustnet toolchain was not found. Build it with `cargo build --release -p rustnet-cli`,");
            log("then set Toolchain.RustNetPath in Settings to the resulting binary.");
        }
        return result;
    }

    /// <summary>Publishes a self-contained build.</summary>
    public async Task<ProcessResult> DeployAsync(
        AppSettings settings,
        string runtimeIdentifier,
        Action<string> log,
        CancellationToken cancellationToken = default)
    {
        var target = projects.FindBuildTarget();
        if (target is null)
        {
            log("No .csproj or .sln found in the open project.");
            return new ProcessResult(-1, "", "no build target", TimeSpan.Zero);
        }

        var output = Path.Combine(projects.CurrentProjectPath!, "publish", runtimeIdentifier);
        log($"$ dotnet publish -c {Configuration} -r {runtimeIdentifier} -o \"{output}\"");

        var result = await runner.RunAsync(
            settings.DotnetPath,
            ["publish", target, "-c", Configuration, "-r", runtimeIdentifier, "--self-contained", "true", "-o", output, "--nologo"],
            projects.CurrentProjectPath,
            log,
            log,
            cancellationToken).ConfigureAwait(false);

        if (result.Succeeded)
        {
            log($"Published to {output}");
        }
        return result;
    }

    /// <summary>
    /// Runs the built assembly against the reduced binding set.
    ///
    /// The same 300 bindings a 192 KB board carries. A program that fails here
    /// will fail on that board, and finding out on a desktop costs seconds
    /// rather than a build-and-flash cycle.
    /// </summary>
    public async Task<ProcessResult> RunOnMinimalBclAsync(
        AppSettings settings,
        Action<string> log,
        CancellationToken cancellationToken = default)
    {
        var assembly = projects.FindOutputAssembly(Configuration);
        if (assembly is null)
        {
            log("Build the project first.");
            return new ProcessResult(-1, "", "no assembly", TimeSpan.Zero);
        }

        var rustnet = ResolveRustNet(settings);
        log($"$ {Path.GetFileName(rustnet)} run --bcl minimal \"{Path.GetFileName(assembly)}\"");
        return await runner.RunAsync(
            rustnet,
            ["run", "--bcl", "minimal", assembly],
            Path.GetDirectoryName(assembly),
            log,
            log,
            cancellationToken).ConfigureAwait(false);
    }

    /// <summary>Runs `rustnet verify`, which reports what RustCLR cannot resolve.</summary>
    public async Task<ProcessResult> VerifyOnRustClrAsync(
        AppSettings settings,
        Action<string> log,
        CancellationToken cancellationToken = default)
    {
        var assembly = projects.FindOutputAssembly(Configuration);
        if (assembly is null)
        {
            log("Build the project first — verify needs a compiled assembly.");
            return new ProcessResult(-1, "", "no assembly", TimeSpan.Zero);
        }

        var rustnet = ResolveRustNet(settings);
        log($"$ {Path.GetFileName(rustnet)} verify \"{Path.GetFileName(assembly)}\"");
        return await runner.RunAsync(
            rustnet,
            ["verify", assembly],
            Path.GetDirectoryName(assembly),
            log,
            log,
            cancellationToken).ConfigureAwait(false);
    }

    /// <summary>
    /// Finds the rustnet binary: the configured path, then the workspace build
    /// output, then PATH.
    /// </summary>
    public static string ResolveRustNet(AppSettings settings)
    {
        if (!string.IsNullOrWhiteSpace(settings.RustNetPath) && File.Exists(settings.RustNetPath))
        {
            return settings.RustNetPath;
        }

        var executable = OperatingSystem.IsWindows() ? "rustnet.exe" : "rustnet";
        var here = AppContext.BaseDirectory;
        for (var directory = new DirectoryInfo(here); directory is not null; directory = directory.Parent)
        {
            foreach (var profile in (string[])["release", "debug"])
            {
                var candidate = Path.Combine(directory.FullName, "target", profile, executable);
                if (File.Exists(candidate)) return candidate;
            }
        }
        return "rustnet";
    }
}
