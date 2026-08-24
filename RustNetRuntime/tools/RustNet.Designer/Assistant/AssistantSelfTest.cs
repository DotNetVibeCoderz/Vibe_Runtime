using System;
using System.Collections.Generic;
using System.IO;
using System.Text;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;
using RustNet.Designer.Assistant.Plugins;
using RustNet.UI;

namespace RustNet.Designer.Assistant;

/// <summary>
/// Headless checks for everything in the assistant that does not need a model
/// behind it: settings, sessions, uploads, markdown and code rendering, the
/// expression evaluator, the design functions, and — the part most likely to
/// rot — that a kernel actually builds for each provider with the plugins
/// attached and the right execution-settings type.
///
/// Run by <c>rustnet-designer --selftest</c>, so a change that breaks the
/// Semantic Kernel wiring fails in CI rather than the first time someone opens
/// the panel.
/// </summary>
public static class AssistantSelfTest
{
    public static bool Run(TextWriter log)
    {
        var failures = new List<string>();
        string sandbox = Path.Combine(Path.GetTempPath(), "rustnet-designer-selftest-" + Guid.NewGuid().ToString("n")[..8]);

        try
        {
            Check(failures, "options", CheckOptions);
            Check(failures, "sessions", () => CheckSessions(sandbox));
            Check(failures, "attachments", () => CheckAttachments(sandbox));
            Check(failures, "markdown", CheckMarkdown);
            Check(failures, "highlighter", CheckHighlighter);
            Check(failures, "expressions", CheckExpressions);
            Check(failures, "html to text", CheckHtmlToText);
            Check(failures, "design functions", CheckDesignPlugin);
            Check(failures, "plugin registration", CheckPluginRegistration);
            Check(failures, "kernels", CheckKernels);
            Check(failures, "layout round-trip", CheckLayoutRoundTrip);
            Check(failures, "prompt library", CheckPromptLibrary);
            Check(failures, "formatter", CheckFormatter);
            Check(failures, "deployment", () => CheckDeployment(log));
        }
        finally
        {
            try
            {
                if (Directory.Exists(sandbox))
                {
                    Directory.Delete(sandbox, recursive: true);
                }
            }
            catch (IOException)
            {
                // A leftover temp directory is not a test failure.
            }
        }

        foreach (string failure in failures)
        {
            log.WriteLine("assistant selftest FAIL: " + failure);
        }
        if (failures.Count == 0)
        {
            log.WriteLine($"assistant selftest OK: 14 groups, {PromptLibrary.All.Count} prompts, "
                + $"{Enum.GetValues<AiProvider>().Length} providers wired");
        }
        return failures.Count == 0;
    }

    private static void Check(List<string> failures, string name, Action body)
    {
        try
        {
            body();
        }
        catch (Exception ex)
        {
            failures.Add($"{name}: {ex.Message}");
        }
    }

    private static void Expect(bool condition, string what)
    {
        if (!condition)
        {
            throw new InvalidOperationException(what);
        }
    }

    // ---- groups --------------------------------------------------------

    private static void CheckOptions()
    {
        AssistantOptions o = AssistantOptions.Load();
        Expect(o.Persona.Length > 200, "persona is missing or too short");
        Expect(o.Persona.Contains("Jack The Code Bender", StringComparison.Ordinal),
            "persona does not introduce Jack");
        Expect(!o.Persona.Contains("\\n", StringComparison.Ordinal), "persona still has literal \\n escapes");
        Expect(o.Temperature is >= 0 and <= 2, "temperature out of range");
        Expect(o.DataDirectory.Length > 0, "no data directory resolved");
        Expect(Directory.Exists(o.WorkspaceRoot), "workspace root does not exist: " + o.WorkspaceRoot);

        foreach (AiProvider p in Enum.GetValues<AiProvider>())
        {
            AssistantOptions.ProviderOptions po = o.For(p);
            Expect(po.Model.Length > 0, $"{p} has no default model");
            Expect(po.Models.Contains(po.Model), $"{p}'s model is missing from its model list");
            // A ${ENV_VAR} placeholder must have been resolved away, to a value
            // or to empty — never left as the literal placeholder.
            Expect(!po.ApiKey.StartsWith("${", StringComparison.Ordinal), $"{p} key placeholder unresolved");
        }
        Expect(o.IsProviderConfigured(AiProvider.Ollama), "Ollama should count as configured (endpoint only)");
    }

    private static void CheckSessions(string sandbox)
    {
        var store = new SessionStore(Path.Combine(sandbox, "sessions-test"));
        var session = new ChatSession();
        session.Messages.Add(new ChatMessage { Role = ChatRole.User, Text = "Design a boiler dashboard. Then apply it." });
        session.Messages.Add(new ChatMessage { Role = ChatRole.Assistant, Text = "Done." });
        session.RetitleFromFirstMessage();
        Expect(session.Title == "Design a boiler dashboard", "title not taken from the first sentence: " + session.Title);

        store.Save(session);
        List<ChatSession> loaded = store.LoadAll();
        Expect(loaded.Count == 1, "session did not round-trip");
        Expect(loaded[0].Messages.Count == 2, "messages did not round-trip");
        Expect(loaded[0].Messages[0].Role == ChatRole.User, "role did not round-trip");

        store.Reset(session);
        Expect(store.LoadAll()[0].Messages.Count == 0, "reset left messages behind");
        Expect(store.LoadAll().Count == 1, "reset removed the session");

        store.Delete(session);
        Expect(store.LoadAll().Count == 0, "delete left the session behind");
    }

    private static void CheckAttachments(string sandbox)
    {
        AssistantOptions options = AssistantOptions.Load();
        options.DataDirectory = Path.Combine(sandbox, "attach-test");
        var store = new SessionStore(options.DataDirectory);
        var attachments = new AttachmentStore(store, options);
        var session = new ChatSession();

        string doc = Path.Combine(sandbox, "notes.md");
        Directory.CreateDirectory(sandbox);
        File.WriteAllText(doc, "# Notes\nflow 62 C\n");
        ChatAttachment a = attachments.Add(session, doc);
        Expect(a.Kind == AttachmentKind.Document, "markdown classified as an image");
        Expect(a.MimeType == "text/markdown", "wrong mime type: " + a.MimeType);
        Expect(a.TextExcerpt.Contains("flow 62 C", StringComparison.Ordinal), "document text was not inlined");
        Expect(a.Url.StartsWith("file:///", StringComparison.Ordinal), "no file URL: " + a.Url);
        Expect(a.WebUrl.Contains(AttachmentStore.VirtualHost, StringComparison.Ordinal), "no virtual-host URL");
        Expect(File.Exists(a.StoredPath), "the upload was not copied into the session folder");

        // A second upload of the same name must not overwrite the first.
        ChatAttachment b = attachments.Add(session, doc);
        Expect(b.StoredPath != a.StoredPath, "a same-named upload overwrote the first");

        string png = Path.Combine(sandbox, "shot.png");
        File.WriteAllBytes(png, new byte[] { 0x89, 0x50, 0x4E, 0x47 });
        ChatAttachment img = attachments.Add(session, png);
        Expect(img.Kind == AttachmentKind.Image, "png not classified as an image");
        Expect(img.TextExcerpt.Length == 0, "an image should not carry a text excerpt");

        options.MaxImageBytes = 2;
        try
        {
            attachments.Add(session, png);
            throw new InvalidOperationException("an oversized image was accepted");
        }
        catch (IOException)
        {
            // Expected: the cap is reported at attach time, not at send time.
        }
    }

    private static void CheckMarkdown()
    {
        const string source = """
            # Boiler

            | pin | use |
            |---|---|
            | 4 | flow |

            ```xml
            <window width="320" height="240"/>
            ```

            ```csharp
            public static void Main() { }
            ```

            [docs](https://example.com) and `inline`.
            """;

        string html = MarkdownRenderer.RenderBody(source);
        Expect(html.Contains("<table>", StringComparison.Ordinal), "pipe table did not render");
        Expect(html.Contains("figure class=\"code", StringComparison.Ordinal), "code block was not rewritten into a card");
        Expect(html.Contains("data-act=\"apply-xml\"", StringComparison.Ordinal),
            "a <window> block did not offer the apply action");
        Expect(html.Contains("data-act=\"to-code\"", StringComparison.Ordinal), "no send-to-code action");
        Expect(html.Contains("t-kw", StringComparison.Ordinal), "C# was not highlighted");
        Expect(!html.Contains("<script", StringComparison.OrdinalIgnoreCase), "unexpected script in a rendered body");

        // The code payload must survive base64 in the data attribute.
        var attribute = System.Text.RegularExpressions.Regex.Match(html, "data-code=\"([^\"]+)\"");
        Expect(attribute.Success, "no data-code attribute");
        string decoded = Encoding.UTF8.GetString(Convert.FromBase64String(attribute.Groups[1].Value));
        Expect(decoded.Contains("<window", StringComparison.Ordinal), "the code payload did not survive encoding");

        var message = new ChatMessage
        {
            Role = ChatRole.Assistant,
            Text = "ok",
            Model = "Anthropic/claude-opus-5",
            ToolCalls = { "get_ui_reference", "apply_layout_xml" },
        };
        string turn = MarkdownRenderer.RenderMessage(message, streaming: true);
        Expect(turn.Contains("id=\"live\"", StringComparison.Ordinal), "a streaming turn has no live id");
        Expect(turn.Contains("carrier", StringComparison.Ordinal), "a streaming turn has no carrier mark");
        Expect(turn.Contains("apply_layout_xml", StringComparison.Ordinal), "tool pills missing");

        string page = MarkdownRenderer.Document(new[] { message }, "<h2>empty</h2>");
        Expect(page.StartsWith("<!doctype html>", StringComparison.OrdinalIgnoreCase), "document has no doctype");
        Expect(page.Contains("window.rn", StringComparison.Ordinal), "the host bridge script is missing");
        Expect(!page.Contains("{{BODY}}", StringComparison.Ordinal), "the body placeholder was not substituted");
    }

    private static void CheckHighlighter()
    {
        string cs = CodeHighlighter.Highlight("// note\nint x = 0x1F; string s = \"hi\";", "csharp");
        Expect(cs.Contains("t-cmt", StringComparison.Ordinal), "comment not classified");
        Expect(cs.Contains("t-num", StringComparison.Ordinal), "hex literal not classified");
        Expect(cs.Contains("t-str", StringComparison.Ordinal), "string not classified");

        string xml = CodeHighlighter.Highlight("<label id=\"a\" fg=\"F800\"/>", "xml");
        Expect(xml.Contains("t-tag", StringComparison.Ordinal), "tag not classified");
        Expect(xml.Contains("t-attr", StringComparison.Ordinal), "attribute not classified");

        // Unknown languages must still be safe to drop into the page.
        string plain = CodeHighlighter.Highlight("<b>not html</b>", "brainfuck");
        Expect(plain.Contains("&lt;b&gt;", StringComparison.Ordinal), "unknown language was not escaped");

        Expect(CodeHighlighter.Normalize("C#") == "csharp", "language alias not normalised");
        Expect(CodeHighlighter.Normalize("ps1") == "bash", "shell alias not normalised");
    }

    private static void CheckExpressions()
    {
        void Near(string expression, double expected)
        {
            double actual = Expression.Evaluate(expression);
            Expect(Math.Abs(actual - expected) < 1e-9, $"{expression} = {actual}, expected {expected}");
        }

        Near("(320-16)/8", 38);
        Near("2^3^2", 512);            // right-associative
        Near("-2^2", -4);              // the sign binds looser than the power
        Near("round(3.3/4096*1500, 3)", 1.208);   // 1.20849… truncates to 1.208 at 3 places
        Near("max(1, 7, 3) + min(4, 2)", 9);
        Near("hypot(3,4)", 5);
        Near("deg(pi)", 180);
        Near("17 % 5", 2);
        Near("1e3 + 1", 1001);

        foreach (string bad in new[] { "1 +", "sqrt()", "nope(2)", "1/0", "(1", "2 2" })
        {
            try
            {
                Expression.Evaluate(bad);
                throw new InvalidOperationException($"\"{bad}\" should not evaluate");
            }
            catch (Exception ex) when (ex is FormatException or DivideByZeroException)
            {
                // Expected.
            }
        }
    }

    private static void CheckHtmlToText()
    {
        string text = HtmlToText.Convert(
            "<html><head><title>Datasheet</title><style>p{color:red}</style></head>"
            + "<body><h2>Registers</h2><script>evil()</script><p>Address is 0x76 &amp; 0x77.</p>"
            + "<ul><li>one</li><li>two</li></ul></body></html>");
        Expect(text.Contains("Datasheet", StringComparison.Ordinal), "title lost");
        Expect(text.Contains("## Registers", StringComparison.Ordinal), "heading not converted");
        Expect(text.Contains("0x76 & 0x77", StringComparison.Ordinal), "entity not decoded");
        Expect(text.Contains("- one", StringComparison.Ordinal), "list item not converted");
        Expect(!text.Contains("evil", StringComparison.Ordinal), "script content leaked into the text");
        Expect(!text.Contains("color:red", StringComparison.Ordinal), "style content leaked into the text");
    }

    private static void CheckDesignPlugin()
    {
        var bridge = new FakeBridge();
        var design = new DesignPlugin(bridge);

        Expect(design.GetUiReference().Contains("scrollviewer", StringComparison.Ordinal),
            "the UI reference does not list every kind");
        Expect(design.GetGraphicsReference().Contains("FillCircle", StringComparison.Ordinal),
            "the graphics reference is missing primitives");
        Expect(design.GetLanguageLimits().Contains("untyped", StringComparison.Ordinal),
            "the language limits omit the untyped-catch trap");
        Expect(design.DescribePanel().Contains("40 chars", StringComparison.Ordinal),
            "panel metrics wrong for 320 px: " + design.DescribePanel());

        string good = "<window width=\"320\" height=\"240\"><label id=\"t\" text=\"Hi\"/></window>";
        Expect(design.ValidateLayoutXml(good).StartsWith("Valid.", StringComparison.Ordinal), "valid XML rejected");
        Expect(design.ValidateLayoutXml("<window").StartsWith("Invalid:", StringComparison.Ordinal),
            "malformed XML accepted");

        // A label wider than the panel is legal XML but wrong on the glass.
        string tooWide = "<window width=\"160\" height=\"128\"><label text=\""
            + new string('x', 40) + "\" scale=\"2\"/></window>";
        Expect(design.ValidateLayoutXml(tooWide).Contains("Warnings", StringComparison.Ordinal),
            "over-wide text was not warned about");

        Expect(design.ApplyLayoutXml(good).StartsWith("Applied", StringComparison.Ordinal), "apply failed");
        Expect(bridge.Applied == good, "the bridge did not receive the layout");
        Expect(design.ApplyLayoutXml("<nope").StartsWith("Not applied:", StringComparison.Ordinal),
            "bad XML was applied");

        design.SetGeneratedCode("Program.cs", "csharp", "class P { }");
        Expect(bridge.CodeFile == "Program.cs" && bridge.Code.Contains("class P", StringComparison.Ordinal),
            "generated code did not reach the code pane");

        Expect(design.Rgb565(255, 0, 0).StartsWith("F800", StringComparison.Ordinal),
            "red is not F800: " + design.Rgb565(255, 0, 0));
        Expect(design.Rgb565FromHex("#00FF00").StartsWith("07E0", StringComparison.Ordinal),
            "green is not 07E0: " + design.Rgb565FromHex("#00FF00"));
        Expect(design.Rgb565FromHex("nope").StartsWith("Not a hex", StringComparison.Ordinal),
            "a bad hex colour was accepted");
    }

    private static void CheckPluginRegistration()
    {
        // KernelPluginFactory validates every [KernelFunction] signature and
        // builds its JSON schema; a bad parameter type fails here, not at the
        // first tool call.
        var plugins = new List<KernelPlugin>
        {
            KernelPluginFactory.CreateFromObject(new DesignPlugin(new FakeBridge()), "design"),
            KernelPluginFactory.CreateFromObject(new TimePlugin(), "time"),
            KernelPluginFactory.CreateFromObject(new MathPlugin(), "math"),
            KernelPluginFactory.CreateFromObject(new WebPlugin(AssistantOptions.Load()), "web"),
        };

        var names = new HashSet<string>(StringComparer.Ordinal);
        foreach (KernelPlugin plugin in plugins)
        {
            foreach (KernelFunction f in plugin)
            {
                Expect(f.Description.Length > 20, $"{f.Name} has no useful description");
                Expect(names.Add(f.Name), $"duplicate function name {f.Name}");
            }
        }
        foreach (string required in new[]
                 {
                     "get_ui_reference", "apply_layout_xml", "validate_layout_xml", "set_generated_code",
                     "search_web", "fetch_page", "calculate", "get_current_datetime", "rgb565",
                 })
        {
            Expect(names.Contains(required), "missing function " + required);
        }
    }

    private static void CheckKernels()
    {
        AssistantOptions options = AssistantOptions.Load();
        var plugins = new List<KernelPlugin>
        {
            KernelPluginFactory.CreateFromObject(new DesignPlugin(new FakeBridge()), "design"),
        };

        foreach (AiProvider provider in Enum.GetValues<AiProvider>())
        {
            options.Provider = provider;
            // Placeholder credentials: building a kernel must not require a
            // reachable service, only a well-formed configuration.
            if (provider != AiProvider.Ollama)
            {
                options.Current.ApiKey = "selftest-not-a-real-key";
            }

            Kernel kernel = KernelFactory.Create(options, plugins, new ToolTap());
            Expect(kernel.GetRequiredService<IChatCompletionService>() != null,
                $"{provider} produced no chat service");
            Expect(kernel.Plugins.Count == 1, $"{provider} kernel lost its plugins");

            PromptExecutionSettings settings = KernelFactory.CreateSettings(options);
            Expect(settings.FunctionChoiceBehavior != null, $"{provider} settings have no tool behaviour");
        }

        // A missing key must be reported as something the person can fix.
        options.Provider = AiProvider.OpenAI;
        options.OpenAI.ApiKey = "";
        try
        {
            KernelFactory.Create(options, plugins, new ToolTap());
            throw new InvalidOperationException("a keyless provider built a kernel");
        }
        catch (InvalidOperationException ex) when (ex.Message.Contains("app.config", StringComparison.Ordinal))
        {
            // Expected: the message names where to put the key.
        }
    }

    private static void CheckLayoutRoundTrip()
    {
        // The designer saves through Ui.ToXml, and the assistant reads the
        // canvas the same way — anything ToXml drops is silently lost on save.
        const string source = """
            <window width="320" height="240" pad="8" gap="6">
              <stack orient="horizontal" pad="4" gap="10">
                <radio id="a" text="Heat" group="mode" checked="true"/>
                <border id="b" border="8410" bg="1082"/>
              </stack>
            </window>
            """;
        UiElement again = Ui.LoadXml(Ui.ToXml(Ui.LoadXml(source)));
        Expect(again.Padding == 8 && again.Gap == 6, "window padding/gap lost in the round-trip");
        UiElement stack = again.Children[0];
        Expect(stack.Horizontal, "orientation lost in the round-trip");
        Expect(stack.Padding == 4 && stack.Gap == 10, "stack padding/gap lost in the round-trip");
        Expect(again.FindById("a").Group == "mode", "radio group lost in the round-trip");
        Expect(again.FindById("a").Checked, "checked state lost in the round-trip");
        Expect(again.FindById("b").Border == 0x8410, "border colour lost in the round-trip");
    }

    private static void CheckPromptLibrary()
    {
        Expect(PromptLibrary.All.Count >= 40, $"only {PromptLibrary.All.Count} prompts");
        var titles = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (PromptTemplate p in PromptLibrary.All)
        {
            Expect(p.Category.Length > 0, "a prompt has no category");
            Expect(titles.Add(p.Title), "duplicate prompt title: " + p.Title);
            Expect(p.Text.Length > 60, "prompt too thin: " + p.Title);
        }
        var categories = new List<string>(PromptLibrary.Categories);
        Expect(categories.Count >= 6, $"only {categories.Count} prompt categories");
        Expect(PromptLibrary.DefaultPersona.Contains("Jack The Code Bender", StringComparison.Ordinal),
            "the fallback persona does not introduce Jack");
    }

    private static void CheckFormatter()
    {
        string xml = Editor.CodeFormatter.Format(
            "<window width=\"320\"><label text=\"Hi\"/><stack><rect/></stack></window>", "xml", out string? xmlError);
        Expect(xmlError == null, "XML formatter reported: " + xmlError);
        Expect(xml.Contains("\n  <label", StringComparison.Ordinal), "XML was not indented:\n" + xml);

        // Roslyn's Format fixes indentation and spacing while keeping the
        // author's line breaks — the same contract as Format Document in an IDE.
        string messy = "class P\n{\nstatic void Main()\n{\nint x=1;\nif(x>0)\n{\n"
            + "System.Console.WriteLine(\"{ not a brace }\");\n}\n}\n}\n";
        string code = Editor.CodeFormatter.Format(messy, "csharp", out string? codeError);
        Expect(codeError == null, "C# formatter reported: " + codeError);
        Expect(code.Contains("\n    static void Main()", StringComparison.Ordinal),
            "member indentation not fixed:\n" + code);
        Expect(code.Contains("int x = 1;", StringComparison.Ordinal), "spacing not fixed:\n" + code);
        Expect(code.Contains("if (x > 0)", StringComparison.Ordinal), "keyword spacing not fixed:\n" + code);
        // The brace inside the string literal must survive untouched — this is
        // exactly what a hand-rolled re-indenter gets wrong.
        Expect(code.Contains("\"{ not a brace }\"", StringComparison.Ordinal),
            "the formatter rewrote a string literal:\n" + code);

        // Unparseable input comes back unchanged, with a reason.
        const string broken = "class P { static void Main( ";
        string untouched = Editor.CodeFormatter.Format(broken, "csharp", out string? brokenError);
        Expect(untouched == broken && brokenError != null, "broken C# was not left alone");
    }

    /// <summary>
    /// The deploy pipeline as far as it can go without a board: scratch project,
    /// <c>dotnet build</c>, RNX compile, RNSB seal. If a device happens to be
    /// answering it also flashes and starts, which makes this the full path.
    /// </summary>
    private static void CheckDeployment(TextWriter log)
    {
        AssistantOptions options = AssistantOptions.Load();
        if (!Directory.Exists(Path.Combine(options.WorkspaceRoot, "dotnet", "src")))
        {
            log.WriteLine("deployment: skipped (no checkout at " + options.WorkspaceRoot + ")");
            return;
        }

        const string source = """
            using RustNet.Graphics;

            class Program
            {
                static void Main()
                {
                    Display.Init(160, 128);
                    Display.Clear(Color.Black);
                    Display.DrawText(8, 8, "SELFTEST", Color.Cyan, 1);
                    Display.Present();
                }
            }
            """;

        var lines = new List<string>();
        AppBuilderResult build = BuildForSelfTest(source, options.WorkspaceRoot, lines.Add);
        Expect(build.Ok, "the scratch project did not build:\n" + string.Join("\n", lines));

        byte[] assembly = System.IO.File.ReadAllBytes(build.AssemblyPath);
        Expect(assembly.Length > 64, $"RNX is only {assembly.Length} bytes");

        string keyPath = Path.Combine(options.WorkspaceRoot, "keys", "rustnet-signing.key");
        if (!File.Exists(keyPath))
        {
            log.WriteLine("deployment: built and compiled; signing skipped (no keys/rustnet-signing.key)");
            return;
        }
        byte[] image = RustNet.Deploy.Signing.Seal(
            RustNet.Deploy.ImageKind.App, RustNet.Deploy.ChipFamily.HostSim, assembly, File.ReadAllBytes(keyPath));
        Expect(image.Length > assembly.Length, "the signed container is not larger than its payload");

        // Opportunistic: flash it only if something is actually listening.
        Deployment.DeviceTarget? target = Deployment.DeviceDiscovery.Probe(
            Deployment.DeviceDiscovery.VirtualDeviceSpec, _ => { });
        if (target == null)
        {
            log.WriteLine($"deployment: built, compiled ({assembly.Length} B) and signed ({image.Length} B); "
                + "no device answered, so the flash hop was not exercised");
            return;
        }
        using var client = RustNet.Deploy.RndpClient.Connect(target.Spec);
        client.FlashApp("designerselftest", image);
        client.StartApp("designerselftest");
        client.StopApp();
        log.WriteLine($"deployment: full path OK — built, RNX {assembly.Length} B, signed {image.Length} B, "
            + $"flashed and started on {target.Board}");
    }

    private sealed record AppBuilderResult(bool Ok, string AssemblyPath);

    private static AppBuilderResult BuildForSelfTest(string source, string root, Action<string> log)
    {
        Deployment.AppBuilder.BuildResult result = Deployment.AppBuilder
            .BuildAsync(source, "DesignerSelfTest", root, log, CancellationToken.None)
            .GetAwaiter().GetResult();
        return new AppBuilderResult(result.Ok, result.AssemblyPath);
    }

    /// <summary>A designer that records what the assistant did to it.</summary>
    private sealed class FakeBridge : IDesignerBridge
    {
        public string Applied { get; private set; } = "";
        public string Code { get; private set; } = "";
        public string CodeFile { get; private set; } = "";

        public string GetLayoutXml() => "<window width=\"320\" height=\"240\"/>";

        public void ApplyLayoutXml(string xml)
        {
            Ui.LoadXml(xml);   // throw on bad input, exactly as the window does
            Applied = xml;
        }

        public (int Width, int Height) GetPanelSize() => (320, 240);

        public string DescribeSelection() => "window at 0,0 sized 320x240";

        public void SetGeneratedCode(string fileName, string language, string code)
        {
            CodeFile = fileName;
            Code = code;
        }

        public string GetGeneratedCode() => Code;
    }
}
