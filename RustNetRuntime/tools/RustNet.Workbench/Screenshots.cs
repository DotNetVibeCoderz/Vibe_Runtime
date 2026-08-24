using Avalonia;
using Avalonia.Controls;
using Avalonia.Headless;
using Avalonia.Threading;
using Avalonia.VisualTree;

namespace RustNet.Workbench;

/// <summary>
/// Renders the Workbench to PNG files without a display.
///
/// The same reasoning as CodeGen's: a screenshot nobody can regenerate goes
/// stale the first time the layout changes, so `RustNet.Workbench --screenshot
/// docs/images` re-renders every image from the real window and the
/// documentation cannot drift from the product.
///
/// Avalonia's headless platform draws through Skia here — the default headless
/// drawing backend produces blank frames, which would be worse than no image.
/// </summary>
internal static class Screenshots
{
    private sealed record Shot(string FileName, string Tab);

    /// <summary>
    /// The board the pictures are taken against, and where it is plugged in.
    /// </summary>
    /// <remarks>
    /// <para>
    /// An M5Stack Tough on COM5 — a real board, and the one these were shot
    /// with. Written down rather than discovered by scanning: a screenshot
    /// that depends on what happened to be plugged into the machine that
    /// rendered it is not reproducible, and CodeGen's devices panel already
    /// refuses to scan for the same reason.
    /// </para>
    /// <para>
    /// **The board is named, not read.** Nothing here connects to it. No
    /// firmware implements the RNDP device side yet — it appears in the HAL
    /// crates only as a named integration point — so the Workbench has nothing
    /// to talk to, and every panel that would show live state shows "not
    /// connected". That is the honest picture of the tool as it stands, and
    /// filling those panels with invented readouts would make the
    /// documentation claim a feature that does not exist.
    /// </para>
    /// </remarks>
    private const string BoardId = "m5tough";
    private const string BoardPort = "COM5";

    /// <summary>
    /// The device to connect to before capturing, or null to stay offline.
    /// </summary>
    /// <remarks>
    /// Offline is still a legitimate set of screenshots — it is what somebody
    /// sees before they plug anything in — but a connected one shows the tool
    /// doing its job, which is what the documentation is for.
    /// </remarks>
    private static string? _deviceSpec;

    /// <summary>The window size every shot is rendered at.</summary>
    /// <remarks>
    /// Fixed rather than taken from the window's own defaults: the images sit
    /// side by side in the README, and a set that disagrees about width reads
    /// as carelessness.
    /// </remarks>
    private const int Width = 1280;
    private const int Height = 800;

    public static int Capture(string directory, string? deviceSpec = null)
    {
        _deviceSpec = deviceSpec;
        Directory.CreateDirectory(directory);

        try
        {
            AppBuilder.Configure<App>()
                .UseHeadless(new AvaloniaHeadlessPlatformOptions { UseHeadlessDrawing = false })
                .UseSkia()
                .SetupWithoutStarting();
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"Could not start the headless renderer: {ex.Message}");
            return 1;
        }

        // One window, several tabs. The tabs are the product — a single shot of
        // whichever one happened to be selected says almost nothing about a
        // tool whose whole shape is "ten views onto one board".
        var shots = new[]
        {
            new Shot("workbench-device.png", "DEVICE"),
            new Shot("workbench-apps.png", "APPS"),
            new Shot("workbench-firmware.png", "FIRMWARE"),
            new Shot("workbench-profiler.png", "PROFILER"),
            new Shot("workbench-about.png", "ABOUT"),
        };

        var written = 0;
        foreach (var shot in shots)
        {
            try
            {
                Render(shot, Path.Combine(directory, shot.FileName));
                Console.WriteLine($"wrote {Path.Combine(directory, shot.FileName)}");
                written++;
            }
            catch (Exception ex)
            {
                Console.Error.WriteLine($"{shot.FileName}: {ex.Message}");
            }
        }

        Console.WriteLine($"{written} of {shots.Length} screenshots rendered.");
        return written == shots.Length ? 0 : 1;
    }

    private static void Render(Shot shot, string path)
    {
        var window = new MainWindow { Width = Width, Height = Height };
        window.Show();
        ShowAttachedBoard(window);
        Populate(window);
        SelectTab(window, shot.Tab);

        // Let bindings settle and the first layout pass complete before the
        // frame is grabbed; otherwise the capture catches an empty window.
        for (var i = 0; i < 8; i++)
        {
            Dispatcher.UIThread.RunJobs();
        }

        var frame = window.CaptureRenderedFrame()
            ?? throw new InvalidOperationException("the renderer returned no frame");

        using (var stream = File.Create(path))
        {
            frame.Save(stream);
        }

        window.Close();
    }

    /// <summary>
    /// Connect to the live device, if one was named, and wait for the answers.
    /// </summary>
    /// <remarks>
    /// <para>
    /// Every panel fetches on a background thread and posts its result back to
    /// the UI thread, so this alternates between sleeping and pumping. Neither
    /// alone is enough: without the sleep the device has not answered yet, and
    /// without the pump the answer sits in the dispatcher queue unapplied.
    /// </para>
    /// <para>
    /// A fixed budget rather than waiting on a signal, because there is no
    /// signal to wait on — and a board that has gone away must produce
    /// screenshots of a disconnected tool rather than hanging the renderer.
    /// </para>
    /// </remarks>
    private static void Populate(MainWindow window)
    {
        if (_deviceSpec is null)
        {
            return;
        }

        window.ConnectAndPopulate(_deviceSpec);
        Settle(seconds: 12);

        // Sequentially, waiting for each: a serial port has one owner, so two
        // panels fetching at once means one of them failing.
        foreach (var panel in new[] { "apps", "logs", "perf", "display" })
        {
            window.RefreshPanel(panel);
            // The display frame is 150 KB, which at 115200 baud is about
            // thirteen seconds on the wire; the rest answer in well under one.
            Settle(seconds: panel == "display" ? 30 : 8);
        }
    }

    /// <summary>Pump the dispatcher for a while, so posted results are applied.</summary>
    /// <remarks>
    /// Sleeping and pumping alternately. Neither alone works: without the
    /// sleep the device has not answered yet, and without the pump its answer
    /// sits in the dispatcher queue unapplied.
    /// </remarks>
    private static void Settle(int seconds)
    {
        for (var i = 0; i < seconds * 20; i++)
        {
            Dispatcher.UIThread.RunJobs();
            Thread.Sleep(50);
        }
        Dispatcher.UIThread.RunJobs();
    }

    /// <summary>
    /// Point the window at the board these screenshots were taken with.
    /// </summary>
    /// <remarks>
    /// Sets the serial spec and selects the board in the firmware tab, so the
    /// pictures show a real part — its build recipe, its memory tier, the
    /// image actually built for it — rather than whichever entry happened to
    /// be first in the catalogue.
    /// </remarks>
    private static void ShowAttachedBoard(MainWindow window)
    {
        var device = window.FindControl<TextBox>("TxtDevice");
        if (device is not null)
        {
            device.Text = $"serial:{BoardPort}";
        }

        var port = window.FindControl<TextBox>("TxtBoardPort");
        if (port is not null)
        {
            port.Text = BoardPort;
        }

        var boards = window.FindControl<ComboBox>("CmbBoard");
        if (boards is null)
        {
            return;
        }

        // Matched on the id the catalogue renders into each row, so reordering
        // the catalogue moves the selection with it. A board that is no longer
        // in the catalogue leaves the default selected rather than throwing:
        // the shot is still worth taking, it just shows a different part.
        var items = boards.ItemsSource?.Cast<object>().Select(i => i?.ToString() ?? "").ToList();
        var index = items?.FindIndex(t => t.StartsWith(BoardId + " ", StringComparison.Ordinal)) ?? -1;
        if (index >= 0)
        {
            boards.SelectedIndex = index;
        }
        else
        {
            Console.Error.WriteLine($"no board `{BoardId}` in the catalogue; using the default");
        }
    }

    /// <summary>
    /// Bring the named tab to the front, or say which ones exist.
    /// </summary>
    /// <remarks>
    /// Matched by header text rather than index, so reordering the tabs moves
    /// the pictures with them instead of silently renaming them. A header that
    /// no longer exists throws with the list, which is what a person renaming
    /// a tab needs to read.
    /// </remarks>
    private static void SelectTab(MainWindow window, string header)
    {
        var tabs = window.GetVisualDescendants().OfType<TabControl>().FirstOrDefault()
            ?? throw new InvalidOperationException("the window has no TabControl");

        var items = tabs.Items.OfType<TabItem>().ToList();
        var match = items.FirstOrDefault(t => string.Equals(
            t.Header as string, header, StringComparison.OrdinalIgnoreCase));

        if (match is null)
        {
            var known = string.Join(", ", items.Select(t => t.Header as string));
            throw new InvalidOperationException($"no tab called `{header}`; the tabs are: {known}");
        }

        tabs.SelectedItem = match;
    }
}
