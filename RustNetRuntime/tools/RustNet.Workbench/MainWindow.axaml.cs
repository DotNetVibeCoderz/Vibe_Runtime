using System.Diagnostics;
using System.Text.Json;
using Avalonia.Media;
using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Media.Imaging;
using Avalonia.Platform;
using Avalonia.Threading;
using RustNet.Deploy;

namespace RustNet.Workbench;

public partial class MainWindow : Window
{
    private string _deviceSpec = TransportFactory.DefaultSpec;
    private readonly DispatcherTimer _timer;

    public MainWindow()
    {
        InitializeComponent();
        FillAbout();
        FillBoards();
        _timer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(1) };
        _timer.Tick += (_, _) =>
        {
            if (ChkFollow.IsChecked == true)
            {
                OnRefreshLogs(this, new RoutedEventArgs());
            }
            if (ChkPerfWatch.IsChecked == true)
            {
                OnRefreshPerf(this, new RoutedEventArgs());
            }
        };
        _timer.Start();
    }

    private RndpClient Client() => RndpClient.Connect(_deviceSpec);

    /// <summary>Run device I/O off the UI thread; marshal result/error back.</summary>
    private void Device(Func<RndpClient, string> work, Action<string>? onDone = null)
    {
        string spec = _deviceSpec;
        Task.Run(() =>
        {
            string result;
            bool ok = true;
            try
            {
                using var client = RndpClient.Connect(spec);
                result = work(client);
            }
            catch (Exception ex)
            {
                result = ex.Message;
                ok = false;
            }
            Dispatcher.UIThread.Post(() =>
            {
                LblStatus.Text = ok ? "ok" : result;
                LblStatus.Foreground = Brush(ok ? "LampOn" : "LampFault");
                LampState.Fill = Brush(ok ? "LampOn" : "LampFault");
                if (ok)
                {
                    onDone?.Invoke(result);
                }
            });
        });
    }

    /// <summary>A palette brush by resource key, so colour lives in one file.</summary>
    private IBrush Brush(string key)
        => this.FindResource(key) as IBrush ?? Avalonia.Media.Brushes.Gray;

    /// <summary>
    /// Put the device on the readout strip.
    /// </summary>
    /// <remarks>
    /// The strip answers the question a person actually has in front of a
    /// board — is it alive, and what is it doing — so it is filled from the
    /// same JSON every panel already fetches rather than from a separate poll.
    /// Fields the device does not report are left out instead of shown empty:
    /// a readout with gaps in it reads as a fault.
    /// </remarks>
    private void UpdateReadout(string infoJson)
    {
        try
        {
            using var doc = JsonDocument.Parse(infoJson);
            var root = doc.RootElement;
            string Text(string name) =>
                root.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String
                    ? v.GetString() ?? "" : "";
            long Number(string name) =>
                root.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.Number
                    ? v.GetInt64() : 0;
            bool Flag(string name) =>
                root.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.True;

            var parts = new List<string>();
            string chip = Text("chip");
            string board = Text("board");
            if (chip.Length > 0) parts.Add(chip);
            if (board.Length > 0 && board != chip) parts.Add(board);

            long up = Number("uptime_ms");
            if (up > 0) parts.Add($"up {Uptime(up)}");

            string app = Text("active_app");
            bool running = Flag("running");
            parts.Add(app.Length > 0
                ? $"{app} {(running ? "running" : "stopped")}"
                : "no app running");

            long heap = Number("heap_used");
            if (heap > 0) parts.Add($"heap {heap / 1024} KB");

            TxtReadout.Text = string.Join("   ·   ", parts);
            TxtReadout.Foreground = Brush("Amber");
        }
        catch (JsonException)
        {
            // Not JSON: show it anyway. A device that answers something
            // unexpected is more useful on screen than hidden behind a parser.
            TxtReadout.Text = infoJson.Replace("\n", " ").Trim();
            TxtReadout.Foreground = Brush("LegendDim");
        }
    }

    /// <summary>Milliseconds as something readable at a glance.</summary>
    private static string Uptime(long ms)
    {
        var t = TimeSpan.FromMilliseconds(ms);
        if (t.TotalDays >= 1) return $"{(int)t.TotalDays}d {t.Hours}h";
        if (t.TotalHours >= 1) return $"{(int)t.TotalHours}h {t.Minutes}m";
        if (t.TotalMinutes >= 1) return $"{(int)t.TotalMinutes}m {t.Seconds}s";
        return $"{t.TotalSeconds:0.0}s";
    }

    /// <summary>
    /// Fill the About panel.
    /// </summary>
    /// <remarks>
    /// The board table is written out here rather than in the markup because
    /// its second column is the honest part: every board speaks RNDP, but the
    /// firmware for each comes from a different place, and a person deciding
    /// whether this tool covers their hardware needs that distinction more
    /// than they need a list of names.
    /// </remarks>
    private void FillAbout()
    {
        var version = typeof(MainWindow).Assembly.GetName().Version;
        LblAboutVersion.Text = version is null ? "development build" : $"v{version.ToString(3)}";

        TxtAboutBoards.Text = string.Join("\n", new[]
        {
            "BOARD                      CHIP      FIRMWARE COMES FROM",
            "-------------------------  --------  ----------------------------------",
            "Virtual device (host)      host-sim  cargo build -p rustnet-firmware",
            "ESP32 DevKit / WROOM       esp32     runtime/firmware-esp32  (esp toolchain)",
            "M5Stack Tough              esp32     runtime/firmware-esp32 --features board-m5tough",
            "M5Stack Core2              esp32     runtime/firmware-esp32 --features board-m5core2",
            "Seeed XIAO ESP32C3         esp32c3   runtime/firmware-esp32  (riscv, RNDP over native USB)",
            "Sipeed Maix Go             k210      runtime/firmware-k210   (riscv64gc, kflash)",
            "Raspberry Pi Pico          rp2040    runtime/firmware-rp2040 (thumbv6m, UF2 over its own USB)",
            "STM32F401 / Nucleo-F401RE  stm32     runtime/firmware-stm32  (thumbv7em, SWD)",
            "Netduino 3 WiFi            stm32     runtime/firmware-stm32  (thumbv7em, DFU)",
            "Meadow F7 Micro            stm32     runtime/firmware-meadow-f7 (Cortex-M7, DFU)",
            "",
            "The chip picker when flashing an app must match the board: an image",
            "signed for one chip is refused by another. The FIRMWARE tab builds and",
            "flashes any of these, given that target's toolchain.",
        });
    }

    // ---- connection / device tab ----

    private void OnConnect(object? sender, RoutedEventArgs e)
    {
        _deviceSpec = string.IsNullOrWhiteSpace(TxtDevice.Text) ? TransportFactory.DefaultSpec : TxtDevice.Text!;
        Device(c => c.Info(), info =>
        {
            TxtInfo.Text = info;
            UpdateReadout(info);
            LblStatus.Text = "connected";
            OnRefreshApps(sender, e);
        });
    }

    /// <summary>
    /// Connect to a device and fill every tab, for the screenshot renderer.
    /// </summary>
    /// <remarks>
    /// <para>
    /// The same handlers the buttons call, in the order somebody would click
    /// them. Driving the real path rather than assigning to the controls means
    /// a screenshot cannot show a layout that the live application never
    /// produces -- which is the entire reason these images are rendered from
    /// the app instead of drawn by hand.
    /// </para>
    /// <para>
    /// Every one of them does its device I/O on a background thread and posts
    /// the result back, so the caller has to pump the dispatcher and wait. See
    /// <c>Screenshots.Populate</c>.
    /// </para>
    /// </remarks>
    public void ConnectAndPopulate(string spec)
    {
        TxtDevice.Text = spec;
        OnConnect(this, new RoutedEventArgs());
    }

    /// <summary>
    /// Fill one panel, for the screenshot renderer.
    /// </summary>
    /// <remarks>
    /// One at a time, and the caller waits between them. Each of these opens
    /// the device transport for itself, and on a serial link only one process
    /// can hold the port — firing them together left three of the four failing
    /// while the fourth reported `ok`, which produced screenshots of empty
    /// panels above a status bar claiming success.
    /// </remarks>
    public void RefreshPanel(string which)
    {
        var e = new RoutedEventArgs();
        switch (which)
        {
            case "apps": OnRefreshApps(this, e); break;
            case "logs": OnRefreshLogs(this, e); break;
            case "perf": OnRefreshPerf(this, e); break;
            case "display": OnCaptureDisplay(this, e); break;
            default: throw new ArgumentException($"no panel called `{which}`");
        }
    }

    private void OnRefreshInfo(object? sender, RoutedEventArgs e)
        => Device(c => c.Info(), info =>
        {
            TxtInfo.Text = info;
            UpdateReadout(info);
        });

    private void OnReboot(object? sender, RoutedEventArgs e)
        => Device(c =>
        {
            c.Reboot();
            return "rebooted";
        });

    private void OnGenerateKeys(object? sender, RoutedEventArgs e)
    {
        try
        {
            string dir = string.IsNullOrWhiteSpace(TxtKeyDir.Text) ? "." : TxtKeyDir.Text!;
            Directory.CreateDirectory(dir);
            var (priv, pub) = Signing.GenerateKeypair();
            File.WriteAllBytes(Path.Combine(dir, "rustnet-signing.key"), priv);
            File.WriteAllBytes(Path.Combine(dir, "rustnet-signing.pub"), pub);
            LblKeysInfo.Text = $"keypair written to {Path.GetFullPath(dir)} (keep the .key file secret)";
        }
        catch (Exception ex)
        {
            LblKeysInfo.Text = $"error: {ex.Message}";
        }
    }

    private void OnProvision(object? sender, RoutedEventArgs e)
    {
        string dir = string.IsNullOrWhiteSpace(TxtKeyDir.Text) ? "." : TxtKeyDir.Text!;
        string pubPath = Path.Combine(dir, "rustnet-signing.pub");
        if (!File.Exists(pubPath))
        {
            LblKeysInfo.Text = $"{pubPath} not found — generate keys first";
            return;
        }
        byte[] pub = File.ReadAllBytes(pubPath);
        Device(c =>
        {
            c.ProvisionKey(pub);
            return "provisioned";
        }, _ => LblKeysInfo.Text = "device provisioned with the public key");
    }

    // ---- apps tab ----

    private void OnRefreshApps(object? sender, RoutedEventArgs e)
        => Device(c => c.ListApps(), json =>
        {
            LstApps.Items.Clear();
            foreach (string entry in json.Trim('[', ']').Split("},", StringSplitOptions.RemoveEmptyEntries))
            {
                string cleaned = entry.Trim().TrimEnd('}') + "}";
                if (cleaned.Length > 2)
                {
                    LstApps.Items.Add(cleaned);
                }
            }
        });

    private string? SelectedAppName()
    {
        string? row = LstApps.SelectedItem as string;
        if (row is null)
        {
            return null;
        }
        int idx = row.IndexOf("\"name\":\"", StringComparison.Ordinal);
        if (idx < 0)
        {
            return null;
        }
        int start = idx + 8;
        int end = row.IndexOf('"', start);
        return end > start ? row[start..end] : null;
    }

    private void OnFlashApp(object? sender, RoutedEventArgs e)
    {
        string? path = TxtAppPath.Text;
        if (string.IsNullOrWhiteSpace(path) || !File.Exists(path))
        {
            LblStatus.Text = "app file not found";
            return;
        }
        string name = string.IsNullOrWhiteSpace(TxtAppName.Text)
            ? Path.GetFileNameWithoutExtension(path).ToLowerInvariant()
            : TxtAppName.Text!;
        string chipName = (CmbChip.SelectedItem as ComboBoxItem)?.Content?.ToString() ?? "host-sim";
        string keyDir = string.IsNullOrWhiteSpace(TxtKeyDir.Text) ? "." : TxtKeyDir.Text!;
        string keyPath = Path.Combine(keyDir, "rustnet-signing.key");
        if (!File.Exists(keyPath))
        {
            LblStatus.Text = $"signing key not found at {keyPath} (Device tab -> Generate)";
            return;
        }
        Task.Run(() =>
        {
            try
            {
                // The assembly goes across as the SDK built it. RustCLR reads
                // real PE/ECMA-335, so there is no intermediate format to
                // compile to and no second metadata writer to keep in step
                // with the first — a whole tool's worth of code that this
                // runtime does not need.
                byte[] assembly = File.ReadAllBytes(path);
                byte[] sealedApp = Signing.Seal(ImageKind.App, Signing.ParseChip(chipName), assembly,
                    File.ReadAllBytes(keyPath));
                using var client = Client();
                client.FlashApp(name, sealedApp);
                Dispatcher.UIThread.Post(() =>
                {
                    LblStatus.Text = $"flashed '{name}' ({sealedApp.Length} bytes)";
                    OnRefreshApps(sender, e);
                });
            }
            catch (Exception ex)
            {
                Dispatcher.UIThread.Post(() => LblStatus.Text = $"error: {ex.Message}");
            }
        });
    }

    private void OnStartApp(object? sender, RoutedEventArgs e)
    {
        string? name = SelectedAppName();
        if (name is null)
        {
            LblStatus.Text = "select an app first";
            return;
        }
        Device(c =>
        {
            c.StartApp(name);
            return $"'{name}' started";
        }, _ => OnRefreshApps(sender, e));
    }

    private void OnStopApp(object? sender, RoutedEventArgs e)
        => Device(c =>
        {
            c.StopApp();
            return "stopped";
        }, _ => OnRefreshApps(sender, e));

    private void OnEraseApp(object? sender, RoutedEventArgs e)
    {
        string? name = SelectedAppName();
        if (name is null)
        {
            LblStatus.Text = "select an app first";
            return;
        }
        Device(c =>
        {
            c.EraseApp(name);
            return $"'{name}' erased";
        }, _ => OnRefreshApps(sender, e));
    }

    // ---- data tab ----

    private void OnDataPush(object? sender, RoutedEventArgs e)
    {
        string? local = TxtDataLocal.Text;
        string? remote = TxtDataRemote.Text;
        if (string.IsNullOrWhiteSpace(local) || !File.Exists(local) || string.IsNullOrWhiteSpace(remote))
        {
            LblStatus.Text = "need an existing local file and a remote path";
            return;
        }
        byte[] data = File.ReadAllBytes(local);
        Device(c =>
        {
            c.FlashData(remote!, data);
            return $"pushed {data.Length} bytes to {remote}";
        });
    }

    private void OnDataPull(object? sender, RoutedEventArgs e)
    {
        string? remote = TxtDataRemote.Text;
        if (string.IsNullOrWhiteSpace(remote))
        {
            LblStatus.Text = "remote path required";
            return;
        }
        Device(c => System.Text.Encoding.UTF8.GetString(c.ReadData(remote!)),
            content => TxtDataOut.Text = content);
    }

    // ---- config / wifi tab ----

    private void OnConfigSet(object? sender, RoutedEventArgs e)
    {
        string? key = TxtCfgKey.Text;
        string? value = TxtCfgValue.Text;
        if (string.IsNullOrWhiteSpace(key))
        {
            return;
        }
        Device(c =>
        {
            c.SetConfig(key!, value ?? "");
            return "stored (encrypted)";
        }, msg => LblCfgOut.Text = msg);
    }

    private void OnConfigGet(object? sender, RoutedEventArgs e)
    {
        string? key = TxtCfgKey.Text;
        if (string.IsNullOrWhiteSpace(key))
        {
            return;
        }
        Device(c => c.GetConfig(key!), v => LblCfgOut.Text = $"{key} = {v}");
    }

    private void OnWifiSave(object? sender, RoutedEventArgs e)
    {
        string? ssid = TxtSsid.Text;
        if (string.IsNullOrWhiteSpace(ssid))
        {
            return;
        }
        string psk = TxtPsk.Text ?? "";
        Device(c =>
        {
            c.ConfigureWifi(ssid!, psk);
            return $"wifi '{ssid}' saved";
        }, msg => LblCfgOut.Text = msg);
    }

    // ---- boot image tab ----

    private void OnBootSet(object? sender, RoutedEventArgs e)
    {
        string? file = TxtBootFile.Text;
        if (string.IsNullOrWhiteSpace(file) || !File.Exists(file)
            || !ushort.TryParse(TxtBootW.Text, out ushort w) || !ushort.TryParse(TxtBootH.Text, out ushort h))
        {
            LblBootOut.Text = "need an existing file plus width and height";
            return;
        }
        byte[] data = File.ReadAllBytes(file);
        Device(c =>
        {
            c.SetBootImage(w, h, data);
            return $"boot image set ({w}x{h})";
        }, msg => LblBootOut.Text = msg);
    }

    private void OnBootGet(object? sender, RoutedEventArgs e)
        => Device(c =>
        {
            byte[] img = c.GetBootImage();
            string path = Path.GetFullPath("bootimg.bin");
            File.WriteAllBytes(path, img);
            return $"saved to {path} ({img.Length} bytes)";
        }, msg => LblBootOut.Text = msg);

    // ---- display tab ----

    private void OnCaptureDisplay(object? sender, RoutedEventArgs e)
    {
        string spec = _deviceSpec;
        Task.Run(() =>
        {
            try
            {
                using var client = RndpClient.Connect(spec);
                var (w, h, pixels) = client.GetDisplay();
                Dispatcher.UIThread.Post(() =>
                {
                    var bmp = new WriteableBitmap(new Avalonia.PixelSize(w, h), new Avalonia.Vector(96, 96),
                        PixelFormat.Bgra8888, AlphaFormat.Opaque);
                    using (var fb = bmp.Lock())
                    {
                        unsafe
                        {
                            byte* dst = (byte*)fb.Address;
                            for (int i = 0; i < w * h; i++)
                            {
                                ushort px = BitConverter.ToUInt16(pixels, i * 2);
                                dst[i * 4 + 0] = (byte)((px & 0x1F) << 3);          // B
                                dst[i * 4 + 1] = (byte)(((px >> 5) & 0x3F) << 2);   // G
                                dst[i * 4 + 2] = (byte)(((px >> 11) & 0x1F) << 3);  // R
                                dst[i * 4 + 3] = 255;
                            }
                        }
                    }
                    ImgDisplay.Source = bmp;
                    LblDisplayInfo.Text = $"{w}x{h} captured at {DateTime.Now:HH:mm:ss}";
                    LblStatus.Text = "ok";
                });
            }
            catch (Exception ex)
            {
                Dispatcher.UIThread.Post(() => LblDisplayInfo.Text = ex.Message);
            }
        });
    }

    // ---- logs / profiler tabs ----

    private void OnRefreshLogs(object? sender, RoutedEventArgs e)
        => Device(c => c.GetLogs(500), logs =>
        {
            TxtLogs.Text = logs;
            TxtLogs.CaretIndex = logs.Length;
        });

    private void OnRefreshPerf(object? sender, RoutedEventArgs e)
        => Device(c => c.GetPerf(), perf => TxtPerf.Text = perf.Replace(",", ",\n"));

    // ---- OTA tab ----

    private void OnOtaPush(object? sender, RoutedEventArgs e)
    {
        string? file = TxtOtaFile.Text;
        string? key = TxtOtaKey.Text;
        if (string.IsNullOrWhiteSpace(file) || !File.Exists(file)
            || string.IsNullOrWhiteSpace(key) || !File.Exists(key))
        {
            LblOtaOut.Text = "need firmware payload and private key files";
            return;
        }
        byte[] payload = File.ReadAllBytes(file);
        byte[] keyBytes = File.ReadAllBytes(key);
        string spec = _deviceSpec;
        Task.Run(() =>
        {
            try
            {
                byte[] sealedFw = Signing.Seal(ImageKind.Firmware, ChipFamily.HostSim, payload, keyBytes);
                using var client = RndpClient.Connect(spec);
                client.OtaUpdate(sealedFw, (done, total) => Dispatcher.UIThread.Post(() =>
                    PrgOta.Value = done * 100.0 / total));
                Dispatcher.UIThread.Post(() =>
                    LblOtaOut.Text = "update verified + staged; confirm after checking device health");
            }
            catch (Exception ex)
            {
                Dispatcher.UIThread.Post(() => LblOtaOut.Text = $"error: {ex.Message}");
            }
        });
    }

    private void OnOtaConfirm(object? sender, RoutedEventArgs e)
        => Device(c => c.OtaConfirm(), slot => LblOtaOut.Text = $"active slot confirmed: {slot}");

    private void OnOtaRollback(object? sender, RoutedEventArgs e)
        => Device(c => c.OtaRollback(), slot => LblOtaOut.Text = $"rolled back to slot: {slot}");

    // ---- firmware tab ----

    /// <summary>The repository this binary sits inside, if it does.</summary>
    /// <remarks>
    /// Delegated to <see cref="BoardFlasher.FindRepoRoot"/> rather than done
    /// again here. This had its own copy, and the copy looked for a
    /// <c>runtime/</c> directory — the *reference* project's layout. RustNet
    /// keeps its firmware in <c>embedded/</c>, so the search walked to the
    /// filesystem root every time and the firmware tab reported "repo not
    /// found above this binary" while sitting four levels inside the repo.
    /// One copy cannot disagree with itself.
    /// </remarks>
    private static string? FindRepoRoot() => BoardFlasher.FindRepoRoot(AppContext.BaseDirectory);

    private void OnFwBuild(object? sender, RoutedEventArgs e)
    {
        string chip = (CmbFwChip.SelectedItem as ComboBoxItem)?.Content?.ToString() ?? "host";
        string? root = FindRepoRoot();
        if (root is null)
        {
            TxtFwOut.Text = "RustNet repo (Cargo.toml) not found above the Workbench binary";
            return;
        }
        TxtFwOut.Text = $"building firmware variant '{chip}'...\n";
        Task.Run(() =>
        {
            var psi = new ProcessStartInfo
            {
                FileName = "cargo",
                Arguments = $"build -p rustnet-firmware --no-default-features --features chip-{chip}",
                WorkingDirectory = root,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
            };
            try
            {
                using var proc = Process.Start(psi)!;
                string output = proc.StandardError.ReadToEnd();
                proc.WaitForExit();
                Dispatcher.UIThread.Post(() =>
                    TxtFwOut.Text += output + (proc.ExitCode == 0 ? "\nBUILD OK" : "\nBUILD FAILED"));
            }
            catch (Exception ex)
            {
                Dispatcher.UIThread.Post(() => TxtFwOut.Text += $"error: {ex.Message}");
            }
        });
    }

    // ---- board firmware ----

    /// <summary>
    /// Fill the board picker from the shared catalogue.
    /// </summary>
    /// <remarks>
    /// From <see cref="BoardCatalog"/> rather than from XAML, so this list and
    /// the CLI's cannot disagree — the one you are not looking at is the one
    /// that would be wrong.
    /// </remarks>
    private void FillBoards()
    {
        CmbBoard.ItemsSource = BoardCatalog.All.Select(b => $"{b.Id}  —  {b.Name}").ToList();
        CmbBoard.SelectedIndex = BoardCatalog.All.ToList().FindIndex(b => b.Flash != FlashKind.None);
    }

    private BoardRecipe? SelectedBoard() =>
        CmbBoard.SelectedIndex >= 0 && CmbBoard.SelectedIndex < BoardCatalog.All.Count
            ? BoardCatalog.All[CmbBoard.SelectedIndex]
            : null;

    private void OnBoardChanged(object? sender, SelectionChangedEventArgs e)
    {
        if (SelectedBoard() is not { } board)
        {
            return;
        }
        LblBoardNote.Text = $"{board.Note}  Needs: {board.Requires}.";
        TxtBoardPort.IsEnabled = board.NeedsPort;

        string? root = FindRepoRoot();
        if (root is null)
        {
            LblBoardArtifact.Text = "repo not found above this binary";
            return;
        }
        string elf = BoardCatalog.ArtifactPath(root, board);
        LblBoardArtifact.Text = File.Exists(elf)
            ? $"built  {new FileInfo(elf).Length / 1024} KiB  {new FileInfo(elf).LastWriteTime:yyyy-MM-dd HH:mm}"
            : "not built yet";
    }

    private void OnBoardBuild(object? sender, RoutedEventArgs e) => RunBoardJob(build: true, flash: false);

    private void OnBoardFlash(object? sender, RoutedEventArgs e) => RunBoardJob(build: false, flash: true);

    private void OnBoardBuildFlash(object? sender, RoutedEventArgs e) => RunBoardJob(build: true, flash: true);

    /// <summary>
    /// Run a build and/or flash on a worker, streaming output into the panel.
    /// </summary>
    /// <remarks>
    /// Lines are appended from the worker through the dispatcher because a
    /// board build takes minutes, and a UI that freezes for a minute is a UI
    /// people force-quit. Buttons are disabled for the duration: two cargo
    /// builds in the same target directory block on each other's lock and look
    /// like a hang.
    /// </remarks>
    private void RunBoardJob(bool build, bool flash)
    {
        if (SelectedBoard() is not { } board)
        {
            return;
        }
        string? root = FindRepoRoot();
        if (root is null)
        {
            TxtFwOut.Text = "RustNet repo (Cargo.toml) not found above the Workbench binary";
            return;
        }
        string port = TxtBoardPort.Text?.Trim() ?? "";
        if (flash && board.NeedsPort && port.Length == 0)
        {
            TxtFwOut.Text = $"{board.Name} is flashed over a serial port — enter one (e.g. COM7).";
            return;
        }
        if (flash && board.Flash == FlashKind.None)
        {
            TxtFwOut.Text = "The virtual device is not flashed. Build it above and run it.";
            return;
        }

        TxtFwOut.Text = $"{board.Name} ({board.Chip})" + Environment.NewLine;
        SetBoardButtons(false);

        // The Pico can be asked into its bootloader over RNDP, so if the
        // Workbench is connected to one, hand that along and nobody has to
        // touch BOOTSEL.
        string? deviceSpec = board.Flash == FlashKind.Uf2 ? _deviceSpec : null;

        Task.Run(() =>
        {
            void Log(string line) => Dispatcher.UIThread.Post(() => TxtFwOut.Text += line + Environment.NewLine);
            try
            {
                if (build && !BoardFlasher.Build(root, board, Log))
                {
                    return;
                }
                if (flash)
                {
                    BoardFlasher.Flash(BoardCatalog.FlashPlan(root, board, port, deviceSpec), Log);
                }
            }
            catch (Exception ex)
            {
                Log($"error: {ex.Message}");
            }
            finally
            {
                Dispatcher.UIThread.Post(() =>
                {
                    SetBoardButtons(true);
                    OnBoardChanged(null, null!);
                });
            }
        });
    }

    private void SetBoardButtons(bool enabled)
    {
        CmbBoard.IsEnabled = enabled;
        BtnBoardBuild.IsEnabled = enabled;
        BtnBoardFlash.IsEnabled = enabled;
        BtnBoardBoth.IsEnabled = enabled;
    }

    private void OnFwList(object? sender, RoutedEventArgs e)
    {
        string? root = FindRepoRoot();
        if (root is null)
        {
            TxtFwOut.Text = "repo not found";
            return;
        }
        string dir = Path.Combine(root, "artifacts", "firmware");
        string target = Path.Combine(root, "target");
        var lines = new List<string>();
        if (Directory.Exists(dir))
        {
            lines.AddRange(Directory.GetFiles(dir).Select(f =>
                $"{Path.GetFileName(f),-44} {new FileInfo(f).Length / 1024,6} KiB"));
        }
        foreach (string profile in new[] { "debug", "release" })
        {
            string exe = Path.Combine(target, profile, "rustnet-firmware.exe");
            if (File.Exists(exe))
            {
                lines.Add($"target/{profile}/rustnet-firmware.exe {new FileInfo(exe).Length / 1024,6} KiB");
            }
        }
        TxtFwOut.Text = lines.Count > 0 ? string.Join('\n', lines) : "no firmware images built yet";
    }
}
