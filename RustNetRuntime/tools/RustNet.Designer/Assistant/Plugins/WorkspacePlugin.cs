using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Linq;
using System.Text;
using Microsoft.SemanticKernel;

namespace RustNet.Designer.Assistant.Plugins;

/// <summary>
/// The assistant's read access to the RustNet checkout — documentation,
/// templates and the managed API surface — plus the attachments the person
/// uploaded in this session, and a write path for generated files.
///
/// Reads are confined to the workspace root and writes to a single generated/
/// folder under the assistant's data directory. Both are checked by resolving
/// the full path and comparing prefixes, so <c>..</c> cannot walk out.
/// </summary>
public sealed class WorkspacePlugin
{
    private readonly AssistantOptions _options;
    private readonly AttachmentStore _attachments;
    private readonly Func<ChatSession?> _currentSession;

    public WorkspacePlugin(AssistantOptions options, AttachmentStore attachments, Func<ChatSession?> currentSession)
    {
        _options = options;
        _attachments = attachments;
        _currentSession = currentSession;
    }

    private string Root => _options.WorkspaceRoot;
    private string GeneratedDir => Path.Combine(_options.DataDirectory, "generated");

    // ---- documentation -------------------------------------------------

    [KernelFunction("list_rustnet_docs")]
    [Description("List the RustNet documentation pages available to read, with their first line as a summary.")]
    public string ListDocs()
    {
        string docs = Path.Combine(Root, "docs");
        if (!Directory.Exists(docs))
        {
            return $"No docs folder under {Root}. Set Assistant.WorkspaceRoot in app.config to the RustNet checkout.";
        }
        var sb = new StringBuilder();
        foreach (string file in Directory.EnumerateFiles(docs, "*.md").OrderBy(f => f))
        {
            string name = Path.GetFileNameWithoutExtension(file);
            // Skip the Indonesian translations; they duplicate the English page.
            if (name.EndsWith(".id", StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            sb.AppendLine($"- {name} — {FirstHeading(file)}");
        }
        return sb.ToString();
    }

    [KernelFunction("read_rustnet_doc")]
    [Description("Read a RustNet documentation page. This is the authoritative source for how the "
        + "runtime, protocol, HAL and managed libraries behave — prefer it over recalling.")]
    public string ReadDoc(
        [Description("Page name without the .md extension, e.g. \"ui\", \"networking\", \"dotnet-support\".")]
        string name)
    {
        string safe = Path.GetFileNameWithoutExtension(name.Trim());
        string path = Path.Combine(Root, "docs", safe + ".md");
        if (!File.Exists(path))
        {
            return $"No doc named \"{safe}\". Call list_rustnet_docs for the list.";
        }
        return Cap(File.ReadAllText(path));
    }

    // ---- templates -----------------------------------------------------

    [KernelFunction("list_templates")]
    [Description("List the project templates shipped with RustNet. Each is a working app; they are the "
        + "best reference for how a real RustNet program is structured.")]
    public string ListTemplates()
    {
        string dir = Path.Combine(Root, "templates");
        if (!Directory.Exists(dir))
        {
            return $"No templates folder under {Root}.";
        }
        var sb = new StringBuilder();
        foreach (string sub in Directory.EnumerateDirectories(dir).OrderBy(d => d))
        {
            string readme = Path.Combine(sub, "README.md");
            string summary = File.Exists(readme) ? FirstHeading(readme) : "";
            sb.AppendLine($"- {Path.GetFileName(sub)}{(summary.Length > 0 ? " — " + summary : "")}");
        }
        return sb.ToString();
    }

    [KernelFunction("read_template")]
    [Description("Read a template's source. Returns Program.cs by default, or the named file inside "
        + "the template. Use it to copy the structure of a working app.")]
    public string ReadTemplate(
        [Description("Template folder name, e.g. \"ui-dashboard\", \"mqtt-dashboard\", \"graphics-primitives\".")]
        string template,
        [Description("Optional file inside the template, e.g. \"README.md\". Blank = Program.cs.")]
        string file = "")
    {
        string dir = Path.Combine(Root, "templates", Path.GetFileName(template.Trim()));
        if (!Directory.Exists(dir))
        {
            return $"No template named \"{template}\". Call list_templates for the list.";
        }
        string wanted = file.Trim().Length == 0 ? "Program.cs" : file.Trim();
        string path = Path.Combine(dir, wanted);
        if (!IsInside(dir, path) || !File.Exists(path))
        {
            var names = Directory.EnumerateFiles(dir).Select(Path.GetFileName);
            return $"No file \"{wanted}\" in {template}. It contains: {string.Join(", ", names)}";
        }
        return Cap($"// templates/{Path.GetFileName(dir)}/{wanted}\n" + File.ReadAllText(path));
    }

    // ---- managed API ---------------------------------------------------

    [KernelFunction("find_managed_api")]
    [Description("Search the RustNet.* managed libraries for API declarations matching a term. Use it to "
        + "confirm a method exists and get its exact signature before calling it in generated code.")]
    public string FindManagedApi(
        [Description("Type, method or keyword, e.g. \"Mqtt\", \"ReadMillivolts\", \"FileSystem\".")] string query)
    {
        string src = Path.Combine(Root, "dotnet", "src");
        if (!Directory.Exists(src))
        {
            return $"No dotnet/src under {Root}.";
        }
        string needle = query.Trim();
        if (needle.Length < 2)
        {
            return "Give at least two characters to search for.";
        }

        var hits = new List<string>();
        foreach (string file in Directory.EnumerateFiles(src, "*.cs", SearchOption.AllDirectories))
        {
            if (file.Contains($"{Path.DirectorySeparatorChar}obj{Path.DirectorySeparatorChar}")
                || file.Contains($"{Path.DirectorySeparatorChar}bin{Path.DirectorySeparatorChar}"))
            {
                continue;
            }
            string relative = Path.GetRelativePath(Root, file);
            string[] lines = File.ReadAllLines(file);
            for (int i = 0; i < lines.Length; i++)
            {
                string line = lines[i].Trim();
                bool declaration = line.StartsWith("public ", StringComparison.Ordinal)
                    || line.StartsWith("public static ", StringComparison.Ordinal);
                if (declaration && line.Contains(needle, StringComparison.OrdinalIgnoreCase))
                {
                    hits.Add($"{relative}:{i + 1}  {line.TrimEnd('{', ' ')}");
                    if (hits.Count >= 80)
                    {
                        goto done;
                    }
                }
            }
        }
    done:
        return hits.Count == 0
            ? $"No public declaration matching \"{needle}\" in dotnet/src. It may not exist — do not call it."
            : string.Join("\n", hits);
    }

    // ---- attachments ---------------------------------------------------

    [KernelFunction("list_attachments")]
    [Description("List the files the person attached to this session, with sizes and types.")]
    public string ListAttachments()
    {
        ChatSession? session = _currentSession();
        if (session == null)
        {
            return "No session is open.";
        }
        var sb = new StringBuilder();
        foreach (ChatMessage m in session.Messages)
        {
            foreach (ChatAttachment a in m.Attachments)
            {
                sb.AppendLine($"- {a.FileName} ({a.Kind}, {a.MimeType}, {a.SizeBytes / 1024} KB)");
            }
        }
        return sb.Length == 0 ? "No attachments in this session." : sb.ToString();
    }

    [KernelFunction("read_attachment")]
    [Description("Read the full text of a document the person attached. Only an excerpt is inlined in "
        + "the message, so use this when you need the rest of it.")]
    public string ReadAttachment(
        [Description("The attachment's file name as listed by list_attachments.")] string fileName)
    {
        ChatSession? session = _currentSession();
        if (session == null)
        {
            return "No session is open.";
        }
        string wanted = Path.GetFileName(fileName.Trim());
        foreach (ChatMessage m in session.Messages)
        {
            foreach (ChatAttachment a in m.Attachments)
            {
                if (!a.FileName.Equals(wanted, StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }
                if (a.Kind == AttachmentKind.Image)
                {
                    return $"{a.FileName} is an image; it is already in the message as image content.";
                }
                try
                {
                    return _attachments.ReadFullText(a);
                }
                catch (Exception ex)
                {
                    return $"Cannot read {a.FileName}: {ex.Message}";
                }
            }
        }
        return $"No attachment named \"{wanted}\" in this session.";
    }

    // ---- generated output ----------------------------------------------

    [KernelFunction("save_generated_file")]
    [Description("Save a generated file so the person can open it. Files land in the assistant's "
        + "generated/ folder; the path returned is absolute. Use set_generated_code for the code pane instead "
        + "when the person is meant to review it first.")]
    public string SaveGeneratedFile(
        [Description("File name, optionally with one subfolder, e.g. \"Program.cs\" or \"thermostat/ui.xml\".")]
        string relativePath,
        [Description("The complete file contents.")] string content)
    {
        try
        {
            string full = Path.GetFullPath(Path.Combine(GeneratedDir, relativePath));
            if (!IsInside(GeneratedDir, full))
            {
                return "Refused: the path escapes the generated folder.";
            }
            Directory.CreateDirectory(Path.GetDirectoryName(full)!);
            File.WriteAllText(full, content);
            return $"Saved {content.Length} characters to {full}";
        }
        catch (Exception ex)
        {
            return "Save failed: " + ex.Message;
        }
    }

    // ---- helpers -------------------------------------------------------

    private static bool IsInside(string root, string candidate)
    {
        string a = Path.GetFullPath(root).TrimEnd(Path.DirectorySeparatorChar) + Path.DirectorySeparatorChar;
        string b = Path.GetFullPath(candidate);
        return b.StartsWith(a, StringComparison.OrdinalIgnoreCase);
    }

    private static string FirstHeading(string path)
    {
        foreach (string line in File.ReadLines(path))
        {
            string t = line.Trim();
            if (t.StartsWith("# ", StringComparison.Ordinal))
            {
                return t.Substring(2).Trim();
            }
        }
        return "";
    }

    private string Cap(string s)
        => s.Length <= _options.HttpMaxChars
            ? s
            : s.Substring(0, _options.HttpMaxChars) + $"\n\n[truncated at {_options.HttpMaxChars} characters]";
}
