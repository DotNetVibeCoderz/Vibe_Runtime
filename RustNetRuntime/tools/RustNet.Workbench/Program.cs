using Avalonia;

namespace RustNet.Workbench;

internal static class Program
{
    [STAThread]
    public static int Main(string[] args)
    {
        // Before the desktop lifetime, deliberately: the headless renderer
        // sets up its own Avalonia platform, and starting a windowing one
        // first would leave two of them and no display to draw on.
        var index = Array.FindIndex(args, a => a is "--screenshot" or "--screenshots");
        if (index >= 0)
        {
            var directory = index + 1 < args.Length && !args[index + 1].StartsWith("--")
                ? args[index + 1]
                : Path.Combine(AppContext.BaseDirectory, "screenshots");
            // `--device serial:COM5` renders the tool connected to a real
            // board. Without it the screenshots show the disconnected state,
            // which is honest but far less useful.
            var at = Array.IndexOf(args, "--device");
            var spec = at >= 0 && at + 1 < args.Length ? args[at + 1] : null;
            return Screenshots.Capture(directory, spec);
        }

        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
        return 0;
    }

    public static AppBuilder BuildAvaloniaApp()
        => AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .LogToTrace();
}
