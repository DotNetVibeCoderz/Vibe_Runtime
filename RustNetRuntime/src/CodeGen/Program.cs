using Avalonia;

namespace CodeGen;

internal static class Program
{
    [STAThread]
    public static int Main(string[] args)
    {
        // Headless modes come first: they must not start a windowing platform.
        var index = Array.FindIndex(args, a => a is "--screenshot" or "--screenshots");
        if (index >= 0)
        {
            var directory = index + 1 < args.Length && !args[index + 1].StartsWith("--")
                ? args[index + 1]
                : Path.Combine(AppContext.BaseDirectory, "screenshots");
            return Screenshots.Capture(directory);
        }

        index = Array.IndexOf(args, "--verify-templates");
        if (index >= 0)
        {
            var filter = index + 1 < args.Length && !args[index + 1].StartsWith("--")
                ? args[index + 1]
                : null;
            return TemplateVerifier.Run(filter, args.Contains("--keep"));
        }

        index = Array.IndexOf(args, "--set");
        if (index >= 0)
        {
            return Headless.Set(args[(index + 1)..]);
        }

        index = Array.IndexOf(args, "--chat");
        if (index >= 0)
        {
            if (index + 1 >= args.Length)
            {
                Console.Error.WriteLine("--chat needs a prompt.");
                return 2;
            }
            var project = Value(args, "--project");
            return Headless.Chat(args[index + 1], project);
        }

        if (args.Contains("--help") || args.Contains("-h"))
        {
            PrintUsage();
            return 0;
        }

        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
        return 0;
    }

    private static string? Value(string[] args, string flag)
    {
        var at = Array.IndexOf(args, flag);
        return at >= 0 && at + 1 < args.Length ? args[at + 1] : null;
    }

    private static void PrintUsage() => Console.WriteLine("""
        CodeGen — the IDE for RustNetRuntime

        USAGE
            CodeGen                                  Launch the IDE
            CodeGen --set Key=Value [Key=Value…]     Write settings into app.config
            CodeGen --chat "<prompt>" [--project D]  Run one assistant turn headlessly
            CodeGen --screenshot <directory>         Render the UI to PNG files
            CodeGen --verify-templates [id] [--keep]  Build every template and run it on
                                                     both runtimes, comparing the output

        SETTING KEYS
            Llm.Provider            OpenAI | Claude | Gemini | Ollama
            Llm.Temperature         Llm.MaxTokens        Llm.SystemPrompt
            <Provider>.Model        <Provider>.ApiKey    <Provider>.Endpoint
            Tavily.ApiKey           Toolchain.RustNetPath
            Toolchain.DotnetPath    Workspace.LastProject

        Built by Gravicode Studios, led by Kang Fadhil.
        """);

    public static AppBuilder BuildAvaloniaApp() =>
        AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .WithInterFont()
            .LogToTrace();
}
