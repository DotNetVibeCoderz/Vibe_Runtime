using System.Diagnostics;

namespace RustNet.Deploy;

/// <summary>
/// Runs a <see cref="BoardCatalog"/> build or flash plan, reporting as it goes.
/// </summary>
/// <remarks>
/// Output is pushed through a callback rather than written to the console, so
/// the CLI can print it and the Workbench can stream it into a panel without
/// either owning the logic. Flashing a board takes tens of seconds and says
/// useful things while it works — a tool that shows nothing until it finishes
/// is indistinguishable from one that has hung.
/// </remarks>
public static class BoardFlasher
{
    /// <summary>Walk up from a starting directory to the repository root.</summary>
    public static string? FindRepoRoot(string from)
    {
        var dir = new DirectoryInfo(from);
        while (dir is not null)
        {
            if (File.Exists(Path.Combine(dir.FullName, "Cargo.toml"))
                && Directory.Exists(Path.Combine(dir.FullName, "runtime")))
            {
                return dir.FullName;
            }
            dir = dir.Parent;
        }
        return null;
    }

    /// <summary>
    /// Build a board's firmware. Returns true if cargo succeeded.
    /// </summary>
    public static bool Build(string repoRoot, BoardRecipe board, Action<string> log)
    {
        string ws = BoardCatalog.WorkspacePath(repoRoot, board);
        log($"$ cargo {board.BuildArgs}");
        log($"  in {ws}");
        bool ok = Run("cargo", board.BuildArgs, ws, log, board.Env);
        if (ok)
        {
            string elf = BoardCatalog.ArtifactPath(repoRoot, board);
            log(File.Exists(elf)
                ? $"BUILD OK  {elf}  ({new FileInfo(elf).Length / 1024} KiB)"
                // Not a failure to report as one: cargo said it worked, so the
                // recipe's idea of where the output lands is what is wrong.
                : $"BUILD OK, but no artifact at the expected path:\n  {elf}");
        }
        else
        {
            log("BUILD FAILED");
        }
        return ok;
    }

    /// <summary>
    /// Run a flash plan. Returns true if every step succeeded.
    /// </summary>
    public static bool Flash(IReadOnlyList<FlashStep> plan, Action<string> log)
    {
        if (plan.Count == 0)
        {
            log("nothing to flash for this board");
            return false;
        }

        foreach (FlashStep step in plan)
        {
            log($"-- {step.Description}");
            bool ok = step switch
            {
                RunStep r => RunTool(r, log),
                EnterBootloaderStep b => EnterBootloader(b, log),
                CopyToVolumeStep c => CopyToVolume(c, log),
                _ => false,
            };
            if (!ok)
            {
                log("FLASH FAILED");
                return false;
            }
        }
        log("FLASH OK");
        return true;
    }

    private static bool RunTool(RunStep step, Action<string> log)
    {
        log($"$ {step.Exe} {step.Args}");
        try
        {
            return Run(step.Exe, step.Args, step.WorkingDirectory, log);
        }
        catch (Exception ex)
        {
            // The usual failure by far: the vendor tool is not installed. Say
            // which one, because "the system cannot find the file specified"
            // does not.
            log($"could not run '{step.Exe}': {ex.Message}");
            log($"is it installed and on PATH?");
            return false;
        }
    }

    private static bool EnterBootloader(EnterBootloaderStep step, Action<string> log)
    {
        RndpClient? client = null;
        bool asked = false;
        try
        {
            client = new RndpClient(TransportFactory.Open(step.DeviceSpec));
            client.RebootToBootloader();
            asked = true;
            log("device asked into its bootloader");
        }
        catch (Exception ex)
        {
            // Not fatal. The board may be in its bootloader already, or be
            // running an image that answers no protocol — the fallback below
            // covers the second case.
            log($"could not reach a running device ({ex.Message})");
        }
        finally
        {
            try
            {
                // The port vanishes the instant the board resets, so closing
                // it usually throws. Reporting that would contradict the
                // success line directly above it.
                client?.Dispose();
            }
            catch (Exception)
            {
            }
        }

        // Only after the client is disposed: it holds the serial port open,
        // and the fallback needs to open the same port itself. Doing this
        // inside the catch above fails with "access denied" every time —
        // which reads as a board that refused, not as a tool holding the door.
        if (!asked && !TouchAt1200(step.DeviceSpec, log))
        {
            log("continuing — the board may already be in its bootloader");
        }

        // USB takes a moment to drop the running device and bring the
        // bootloader up in its place. Without this pause the flasher runs
        // while the old descriptors are still live and reports that no
        // bootloader is present — a race that looks exactly like a board
        // that ignored the request.
        Thread.Sleep(2500);
        return true;
    }

    /// <summary>
    /// Open a serial port at 1200 baud and close it again — the oldest
    /// bootloader trigger there is.
    /// </summary>
    /// <remarks>
    /// For images that answer no protocol. The Meadow's ESP32 bridge is a
    /// transparent USB-to-UART pass-through: it speaks no RNDP, so the
    /// request above cannot reach it, and without this the only way back to
    /// the normal firmware would be a hand on the boot pin. Returns false if
    /// the port is not serial or would not open, in which case the caller
    /// carries on and lets the flasher report what it finds.
    /// </remarks>
    private static bool TouchAt1200(string deviceSpec, Action<string> log)
    {
        if (!deviceSpec.StartsWith("serial:", StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }
        string port = deviceSpec["serial:".Length..].Split(':')[0];
        try
        {
            using (var sp = new System.IO.Ports.SerialPort(port, 1200))
            {
                sp.Open();
                sp.DtrEnable = true;
                Thread.Sleep(150);
            }
            log($"{port}: opened at 1200 baud to request the bootloader");
            return true;
        }
        catch (Exception ex)
        {
            log($"1200-baud touch on {port} did not work ({ex.Message})");
            return false;
        }
    }

    private static bool CopyToVolume(CopyToVolumeStep step, Action<string> log)
    {
        if (!File.Exists(step.SourcePath))
        {
            log($"no image to copy at {step.SourcePath}");
            return false;
        }

        // The board disappears from USB and comes back as a drive; how long
        // that takes is the host's business, not ours.
        DriveInfo? target = null;
        for (int i = 0; i < 40 && target is null; i++)
        {
            target = FindVolume(step.VolumeLabel);
            if (target is null)
            {
                if (i == 0)
                {
                    log($"waiting for the '{step.VolumeLabel}' drive...");
                }
                Thread.Sleep(500);
            }
        }
        if (target is null)
        {
            log($"'{step.VolumeLabel}' never appeared — hold BOOTSEL while plugging the board in");
            return false;
        }

        string dest = Path.Combine(target.RootDirectory.FullName, Path.GetFileName(step.SourcePath));
        log($"copying {new FileInfo(step.SourcePath).Length / 1024} KiB to {dest}");
        File.Copy(step.SourcePath, dest, overwrite: true);
        // The bootloader reboots into the new image the moment the last block
        // lands, so the copy "failing" at the end is what success looks like.
        log("copied; the board reboots into the new firmware on its own");
        return true;
    }

    private static DriveInfo? FindVolume(string label)
    {
        foreach (DriveInfo d in DriveInfo.GetDrives())
        {
            try
            {
                if (d.IsReady && string.Equals(d.VolumeLabel, label, StringComparison.OrdinalIgnoreCase))
                {
                    return d;
                }
            }
            catch (IOException)
            {
                // A drive that vanishes mid-enumeration is one we did not want.
            }
        }
        return null;
    }

    /// <summary>
    /// Start a process, stream both its streams, and wait.
    /// </summary>
    private static bool Run(
        string exe,
        string args,
        string cwd,
        Action<string> log,
        IReadOnlyDictionary<string, string>? env = null)
    {
        var psi = new ProcessStartInfo
        {
            FileName = exe,
            Arguments = args,
            WorkingDirectory = cwd,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
        };
        foreach (var (key, value) in env ?? new Dictionary<string, string>())
        {
            psi.Environment[key] = value;
            log($"  {key}={value}");
        }
        using var proc = Process.Start(psi)
            ?? throw new InvalidOperationException($"could not start {exe}");

        // Both streams, both drained concurrently: cargo writes progress to
        // stderr and results to stdout, and reading one to the end before
        // starting the other deadlocks when the unread pipe fills.
        proc.OutputDataReceived += (_, e) => { if (e.Data is not null) { log(e.Data); } };
        proc.ErrorDataReceived += (_, e) => { if (e.Data is not null) { log(e.Data); } };
        proc.BeginOutputReadLine();
        proc.BeginErrorReadLine();
        proc.WaitForExit();
        return proc.ExitCode == 0;
    }
}
