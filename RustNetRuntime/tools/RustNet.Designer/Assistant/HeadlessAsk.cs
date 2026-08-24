using System;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using RustNet.UI;

namespace RustNet.Designer.Assistant;

/// <summary>
/// One assistant turn without a window: <c>rustnet-designer --ask "…"</c>.
///
/// This is how the model path is exercised without clicking — streaming, tool
/// calls and whatever the assistant did to the layout all land on stdout. The
/// designer it talks to is a console stand-in, so <c>apply_layout_xml</c> prints
/// the layout instead of drawing it.
/// </summary>
public static class HeadlessAsk
{
    /// <summary>
    /// Usage: <c>--ask "&lt;prompt&gt;" [--provider OpenAI] [--model gpt-5]
    /// [--layout ui.xml] [--no-tools]</c>. Returns a process exit code.
    /// </summary>
    public static int Run(string[] args, TextWriter log)
    {
        string prompt = Arg(args, "--ask") ?? "";
        if (prompt.Length == 0)
        {
            log.WriteLine("usage: rustnet-designer --ask \"<prompt>\" [--provider <p>] [--model <m>] [--layout <ui.xml>] [--no-tools]");
            return 2;
        }

        AssistantOptions options = AssistantOptions.Load();
        if (Arg(args, "--provider") is { Length: > 0 } provider)
        {
            options.Provider = AssistantOptions.ParseProvider(provider, options.Provider);
        }
        if (Arg(args, "--model") is { Length: > 0 } model)
        {
            options.Current.Model = model;
        }
        if (Has(args, "--no-tools"))
        {
            options.ToolsEnabled = false;
        }
        // Keep the real session store out of it: an --ask run should not leave a
        // conversation in the panel's list.
        options.DataDirectory = Path.Combine(Path.GetTempPath(), "rustnet-designer-ask");

        if (!options.IsProviderConfigured(options.Provider))
        {
            log.WriteLine($"{options.Provider} is not configured — set Assistant.{options.Current.Name}.ApiKey "
                + "in app.config or export the environment variable it names.");
            return 1;
        }

        var bridge = new ConsoleBridge(Arg(args, "--layout"));
        var store = new SessionStore(options.DataDirectory);
        var attachments = new AttachmentStore(store, options);
        using var service = new AssistantService(options, store, attachments, bridge);

        var session = new ChatSession { Provider = options.Provider.ToString(), Model = options.Current.Model };
        var userMessage = new ChatMessage { Role = ChatRole.User, Text = prompt };
        session.Messages.Add(userMessage);

        log.WriteLine($"--- {options.Provider}/{options.Current.Model}, tools {(options.ToolsEnabled ? "on" : "off")} ---");
        log.WriteLine($"> {prompt}");
        log.WriteLine();

        var calls = new System.Collections.Generic.List<string>();
        try
        {
            // Task.Run, not a bare GetResult: this runs on the WPF dispatcher
            // thread, whose synchronisation context would deadlock any
            // continuation that posts back to it.
            ChatMessage reply = Task.Run(() => service.SendAsync(
                session, userMessage,
                onDelta: piece => log.Write(piece),
                onTool: name =>
                {
                    if (calls.Count == 0 || calls[^1] != name)
                    {
                        calls.Add(name);
                        log.WriteLine($"\n[calls {name}]");
                    }
                },
                CancellationToken.None)).GetAwaiter().GetResult();

            log.WriteLine();
            log.WriteLine();
            log.WriteLine($"--- {reply.Text.Length} chars, {calls.Count} tool call(s): {string.Join(", ", calls)} ---");
            log.WriteLine(bridge.Report());
            return 0;
        }
        catch (Exception ex)
        {
            log.WriteLine();
            log.WriteLine("FAILED: " + ex.GetType().Name + ": " + ex.Message);
            if (ex.InnerException != null)
            {
                log.WriteLine("  inner: " + ex.InnerException.Message);
            }
            return 1;
        }
    }

    private static string? Arg(string[] args, string name)
    {
        for (int i = 0; i < args.Length - 1; i++)
        {
            if (args[i] == name)
            {
                return args[i + 1];
            }
        }
        return null;
    }

    private static bool Has(string[] args, string name) => Array.IndexOf(args, name) >= 0;

    /// <summary>A designer made of stdout.</summary>
    private sealed class ConsoleBridge : IDesignerBridge
    {
        private string _layout;
        private string _code = "";
        private string _codeFile = "";
        private int _applied;

        public ConsoleBridge(string? layoutFile)
        {
            _layout = layoutFile != null && File.Exists(layoutFile)
                ? File.ReadAllText(layoutFile)
                : App.SampleXml;
        }

        public string GetLayoutXml() => _layout;

        public void ApplyLayoutXml(string xml)
        {
            Ui.LoadXml(xml);   // reject bad input exactly as the window does
            _layout = xml;
            _applied++;
        }

        public (int Width, int Height) GetPanelSize()
        {
            UiElement root = Ui.LoadXml(_layout);
            return (root.Width > 0 ? root.Width : 160, root.Height > 0 ? root.Height : 128);
        }

        public string DescribeSelection() => "window (headless: nothing is selected)";

        public void SetGeneratedCode(string fileName, string language, string code)
        {
            _codeFile = fileName;
            _code = code;
        }

        public string GetGeneratedCode() => _code;

        public string Report()
        {
            var sb = new StringBuilder();
            if (_applied > 0)
            {
                sb.AppendLine($"applied {_applied} layout(s); the canvas now holds:");
                sb.AppendLine(_layout);
            }
            if (_code.Length > 0)
            {
                sb.AppendLine($"code pane: {_codeFile} ({_code.Split('\n').Length} lines)");
                sb.AppendLine(_code);
            }
            if (sb.Length == 0)
            {
                sb.AppendLine("(the assistant did not touch the canvas or the code pane)");
            }
            return sb.ToString();
        }
    }
}
