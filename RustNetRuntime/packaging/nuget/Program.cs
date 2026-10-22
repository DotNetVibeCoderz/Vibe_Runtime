// The `rustnet` command as a .NET tool: finds the native toolchain for this machine,
// runs it with the same arguments and console, and returns its exit code.
// Built by Gravicode Studios, led by Kang Fadhil.
using System.Diagnostics;
using System.Runtime.InteropServices;

var rid = RuntimeId();
var nativeRoot = Path.Combine(AppContext.BaseDirectory, "native");
var binary = Path.Combine(nativeRoot, rid, OperatingSystem.IsWindows() ? "rustnet.exe" : "rustnet");

if (!File.Exists(binary))
{
    var shipped = Directory.Exists(nativeRoot)
        ? string.Join(", ", Directory.GetDirectories(nativeRoot).Select(Path.GetFileName).Order())
        : "none";
    Console.Error.WriteLine($"rustnet: this package has no toolchain for {rid} (it has: {shipped}).");
    Console.Error.WriteLine("Download a build for your platform from https://github.com/DotNetVibeCoderz/Vibe_Runtime/releases");
    return 127;
}

// NuGet does not keep the executable bit, so set it on first use.
if (!OperatingSystem.IsWindows())
{
    var mode = File.GetUnixFileMode(binary);
    const UnixFileMode exec = UnixFileMode.UserExecute | UnixFileMode.GroupExecute | UnixFileMode.OtherExecute;
    if ((mode & UnixFileMode.UserExecute) == 0)
    {
        try { File.SetUnixFileMode(binary, mode | exec); }
        catch (UnauthorizedAccessException)
        {
            Console.Error.WriteLine($"rustnet: {binary} is not executable and cannot be made so; run: chmod +x \"{binary}\"");
            return 126;
        }
    }
}

var start = new ProcessStartInfo(binary) { UseShellExecute = false };
foreach (var arg in args) start.ArgumentList.Add(arg);

// Ctrl+C reaches the toolchain directly (same console); the launcher just waits for it.
Console.CancelKeyPress += (_, e) => e.Cancel = true;

using var process = Process.Start(start)!;
process.WaitForExit();
return process.ExitCode;

static string RuntimeId()
{
    var os = OperatingSystem.IsWindows() ? "win" : OperatingSystem.IsMacOS() ? "osx" : "linux";
    var arch = RuntimeInformation.OSArchitecture switch
    {
        Architecture.X64 => "x64",
        Architecture.Arm64 => "arm64",
        Architecture.Arm => "arm",
        var other => other.ToString().ToLowerInvariant(),
    };
    return $"{os}-{arch}";
}
