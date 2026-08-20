using System.ComponentModel;
using CodeGen.Models;
using CodeGen.Services;
using Microsoft.SemanticKernel;

namespace CodeGen.Plugins;

/// <summary>
/// Compiling, running and diagnosing the project.
///
/// Output is truncated before it reaches the model: a failing build can emit
/// thousands of lines, and the first errors are the ones that matter.
/// </summary>
public sealed class ToolchainPlugin(
    ProjectService projects,
    BuildService builds,
    ProcessRunner runner,
    AppSettings settings,
    Action<string> log)
{
    private const int MaxReportedLines = 80;

    [KernelFunction("build")]
    [Description("Compile the open project with the .NET SDK. Returns the compiler output, including any errors.")]
    public async Task<string> BuildAsync()
    {
        if (projects.CurrentProjectPath is null) return "No project is open.";

        var result = await builds.BuildAsync(settings, log).ConfigureAwait(false);
        return Summarise(result, result.Succeeded ? "Build succeeded." : "Build failed.");
    }

    [KernelFunction("run")]
    [Description("Build and run the project. Set onRustClr to true to run it on the RustCLR runtime instead of .NET.")]
    public async Task<string> RunAsync(
        [Description("Run on RustCLR rather than the reference .NET runtime.")] bool onRustClr = false)
    {
        if (projects.CurrentProjectPath is null) return "No project is open.";

        var result = await builds
            .RunAsync(settings, onRustClr ? RunTarget.RustClr : RunTarget.Dotnet, log)
            .ConfigureAwait(false);

        var runtime = onRustClr ? "RustCLR" : ".NET";
        return Summarise(result, result.Succeeded
            ? $"Ran on {runtime} and exited with code {result.ExitCode}."
            : $"Run on {runtime} failed with exit code {result.ExitCode}.");
    }

    [KernelFunction("verify_on_rustclr")]
    [Description("Check which parts of the compiled project RustCLR cannot yet resolve. Use this to find unsupported framework calls.")]
    public async Task<string> VerifyAsync()
    {
        if (projects.CurrentProjectPath is null) return "No project is open.";

        var result = await builds.VerifyOnRustClrAsync(settings, log).ConfigureAwait(false);
        return Summarise(result, result.Succeeded
            ? "Everything the project references resolves on RustCLR."
            : "RustCLR reported unresolved members.");
    }

    [KernelFunction("deploy")]
    [Description("Publish a self-contained build for a runtime identifier such as win-x64, linux-arm64 or linux-riscv64.")]
    public async Task<string> DeployAsync(
        [Description("Runtime identifier, for example win-x64 or linux-arm64.")] string runtimeIdentifier = "win-x64")
    {
        if (projects.CurrentProjectPath is null) return "No project is open.";

        var result = await builds.DeployAsync(settings, runtimeIdentifier, log).ConfigureAwait(false);
        return Summarise(result, result.Succeeded
            ? $"Published for {runtimeIdentifier}."
            : $"Publish for {runtimeIdentifier} failed.");
    }

    [KernelFunction("disassemble")]
    [Description("Show the IL of a compiled method. Use this to explain what code actually compiles to.")]
    public async Task<string> DisassembleAsync(
        [Description("Filter on Type.Method, for example 'Program.Main'.")] string filter = "")
    {
        var assembly = projects.FindOutputAssembly(builds.Configuration);
        if (assembly is null) return "Build the project first — disassembly needs a compiled assembly.";

        var rustnet = BuildService.ResolveRustNet(settings);
        var arguments = string.IsNullOrWhiteSpace(filter)
            ? new[] { "disasm", assembly }
            : ["disasm", assembly, filter];

        var result = await runner.RunAsync(rustnet, arguments).ConfigureAwait(false);
        return Summarise(result, result.Succeeded ? "" : "Disassembly failed.");
    }

    [KernelFunction("run_command")]
    [Description("Run a shell command in the project folder. Only use this when no other tool fits.")]
    public async Task<string> RunCommandAsync(
        [Description("The executable to run, for example 'dotnet' or 'git'.")] string command,
        [Description("Arguments, separated by spaces.")] string arguments = "")
    {
        if (!settings.AllowShellCommands)
        {
            return "Shell commands are turned off in Settings.";
        }
        if (projects.CurrentProjectPath is null) return "No project is open.";

        var parts = arguments.Split(' ', StringSplitOptions.RemoveEmptyEntries);
        log($"$ {command} {arguments}");

        var result = await runner
            .RunAsync(command, parts, projects.CurrentProjectPath, log, log)
            .ConfigureAwait(false);
        return Summarise(result, $"Exit code {result.ExitCode}.");
    }

    /// <summary>Trims tool output to the part that carries the diagnosis.</summary>
    private static string Summarise(ProcessResult result, string headline)
    {
        var lines = result.Combined
            .Split('\n', StringSplitOptions.RemoveEmptyEntries)
            .Select(l => l.TrimEnd('\r'))
            .ToList();

        // Compiler errors are the point of a failed build; float them up.
        var errors = lines
            .Where(l => l.Contains(": error ", StringComparison.OrdinalIgnoreCase)
                        || l.Contains(": warning ", StringComparison.OrdinalIgnoreCase))
            .Take(MaxReportedLines)
            .ToList();

        var body = errors.Count > 0
            ? errors
            : lines.Count <= MaxReportedLines
                ? lines
                : [.. lines.Take(MaxReportedLines), $"… {lines.Count - MaxReportedLines} more lines"];

        return string.IsNullOrEmpty(headline)
            ? string.Join('\n', body)
            : $"{headline}\n{string.Join('\n', body)}";
    }
}
