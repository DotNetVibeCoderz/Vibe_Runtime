using System.Diagnostics;
using System.Text;

namespace CodeGen.Services;

/// <summary>What a command produced.</summary>
public sealed record ProcessResult(int ExitCode, string StandardOutput, string StandardError, TimeSpan Duration)
{
    public bool Succeeded => ExitCode == 0;

    /// <summary>Both streams, in the order a terminal would have shown them.</summary>
    public string Combined =>
        string.IsNullOrEmpty(StandardError) ? StandardOutput : $"{StandardOutput}{StandardError}";
}

/// <summary>
/// Runs external tools — <c>dotnet</c>, <c>rustnet</c>, <c>cargo</c> — and
/// streams their output back as it arrives, so the log panel fills in during a
/// long build instead of after it.
/// </summary>
public sealed class ProcessRunner
{
    /// <summary>Hard ceiling so a hung tool cannot wedge the IDE.</summary>
    public TimeSpan Timeout { get; init; } = TimeSpan.FromMinutes(10);

    /// <param name="environment">
    /// Extra environment variables for the child. Used to hand a firmware build
    /// its <c>RUSTCLR_APP</c> — the assembly to embed — without writing into
    /// the crate.
    /// </param>
    public async Task<ProcessResult> RunAsync(
        string fileName,
        IEnumerable<string> arguments,
        string? workingDirectory = null,
        Action<string>? onOutput = null,
        Action<string>? onError = null,
        CancellationToken cancellationToken = default,
        IReadOnlyDictionary<string, string>? environment = null)
    {
        var info = new ProcessStartInfo
        {
            FileName = fileName,
            WorkingDirectory = workingDirectory ?? Environment.CurrentDirectory,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            // Redirected and then closed immediately, so a child that reads
            // stdin sees end-of-input instead of blocking forever on a console
            // nobody is typing at. Without this, one `Console.ReadLine()` in a
            // built program wedges the IDE until the ten-minute timeout.
            RedirectStandardInput = true,
            UseShellExecute = false,
            CreateNoWindow = true,
            StandardOutputEncoding = Encoding.UTF8,
            StandardErrorEncoding = Encoding.UTF8,
        };
        foreach (var argument in arguments)
        {
            info.ArgumentList.Add(argument);
        }
        if (environment is not null)
        {
            foreach (var (key, value) in environment)
            {
                info.Environment[key] = value;
            }
        }

        using var process = new Process { StartInfo = info, EnableRaisingEvents = true };
        var output = new StringBuilder();
        var error = new StringBuilder();
        var outputDone = new TaskCompletionSource();
        var errorDone = new TaskCompletionSource();

        process.OutputDataReceived += (_, e) =>
        {
            if (e.Data is null) { outputDone.TrySetResult(); return; }
            output.AppendLine(e.Data);
            onOutput?.Invoke(e.Data);
        };
        process.ErrorDataReceived += (_, e) =>
        {
            if (e.Data is null) { errorDone.TrySetResult(); return; }
            error.AppendLine(e.Data);
            onError?.Invoke(e.Data);
        };

        var started = Stopwatch.StartNew();
        try
        {
            process.Start();
        }
        catch (Exception ex)
        {
            // A missing tool is the common case; report it as output rather
            // than throwing, so the log panel explains what to install.
            return new ProcessResult(
                -1,
                "",
                $"Could not start '{fileName}': {ex.Message}",
                started.Elapsed);
        }

        process.BeginOutputReadLine();
        process.BeginErrorReadLine();
        // Signal end-of-input at once. Nothing here ever feeds a child, and a
        // child waiting on input it will never get is indistinguishable from a
        // hang.
        try
        {
            process.StandardInput.Close();
        }
        catch (IOException)
        {
            // The child exited before the handle was closed. Not a problem.
        }

        using var timeoutSource = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeoutSource.CancelAfter(Timeout);

        try
        {
            await process.WaitForExitAsync(timeoutSource.Token).ConfigureAwait(false);
            // Wait for the readers to drain, so no trailing lines are lost.
            await Task.WhenAny(
                Task.WhenAll(outputDone.Task, errorDone.Task),
                Task.Delay(500, CancellationToken.None)).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            TryKill(process);
            var reason = cancellationToken.IsCancellationRequested ? "cancelled" : $"timed out after {Timeout.TotalMinutes:0} min";
            error.AppendLine($"[{fileName} {reason}]");
            return new ProcessResult(-1, output.ToString(), error.ToString(), started.Elapsed);
        }

        return new ProcessResult(process.ExitCode, output.ToString(), error.ToString(), started.Elapsed);
    }

    private static void TryKill(Process process)
    {
        try
        {
            if (!process.HasExited) process.Kill(entireProcessTree: true);
        }
        catch
        {
            // The process may have exited between the check and the kill.
        }
    }

    /// <summary>Whether an executable can be found on PATH.</summary>
    public static bool IsAvailable(string fileName)
    {
        try
        {
            using var probe = Process.Start(new ProcessStartInfo
            {
                FileName = fileName,
                Arguments = "--version",
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true,
            });
            if (probe is null) return false;
            probe.WaitForExit(5000);
            return true;
        }
        catch
        {
            return false;
        }
    }
}
