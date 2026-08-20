using System.ComponentModel;
using System.Text;
using CodeGen.Models;
using CodeGen.Services;
using Microsoft.SemanticKernel;

namespace CodeGen.Plugins;

/// <summary>
/// The assistant's hands: creating projects, and reading and writing source
/// files.
///
/// Every path is resolved inside the open project and rejected if it escapes —
/// the model decides *what* to change, but not *where*.
/// </summary>
public sealed class WorkspacePlugin(ProjectService projects, AppSettings settings, Action<string> log)
{
    [KernelFunction("list_files")]
    [Description("List the files and folders in the open project. Use this before reading or writing to see what exists.")]
    public string ListFiles(
        [Description("Folder relative to the project root. Empty means the root.")] string folder = "")
    {
        if (projects.CurrentProjectPath is null) return "No project is open.";

        if (!TryResolve(folder, out var target, out var error)) return error;
        if (!Directory.Exists(target)) return $"No folder at '{folder}'.";

        var builder = new StringBuilder();
        foreach (var entry in Directory.EnumerateFileSystemEntries(target).OrderBy(e => e))
        {
            var relative = Path.GetRelativePath(projects.CurrentProjectPath, entry);
            var name = Path.GetFileName(entry);
            if (name is "bin" or "obj" or ".git" or "target") continue;
            builder.AppendLine(Directory.Exists(entry) ? $"{relative}/" : relative);
        }
        return builder.Length == 0 ? "(empty)" : builder.ToString();
    }

    [KernelFunction("read_file")]
    [Description("Read a source file from the open project. Returns the file contents with line numbers.")]
    public string ReadFile(
        [Description("Path relative to the project root, for example 'Program.cs'.")] string path)
    {
        if (!TryResolve(path, out var target, out var error)) return error;
        if (!File.Exists(target)) return $"No file at '{path}'.";

        var info = new FileInfo(target);
        if (info.Length > settings.MaxFileSizeKB * 1024)
        {
            return $"'{path}' is {info.Length / 1024} KB, over the {settings.MaxFileSizeKB} KB limit.";
        }

        var lines = File.ReadAllLines(target);
        var builder = new StringBuilder();
        for (var i = 0; i < lines.Length; i++)
        {
            builder.Append(i + 1).Append('\t').AppendLine(lines[i]);
        }
        log($"Jack read {path}");
        return builder.ToString();
    }

    [KernelFunction("write_file")]
    [Description("Create a file or replace its entire contents. Use this for new files and for whole-file rewrites.")]
    public string WriteFile(
        [Description("Path relative to the project root.")] string path,
        [Description("The complete new contents of the file.")] string contents)
    {
        if (!TryResolve(path, out var target, out var error)) return error;

        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(target)!);
            var existed = File.Exists(target);
            File.WriteAllText(target, contents, new UTF8Encoding(false));
            log($"Jack {(existed ? "rewrote" : "created")} {path}");
            return $"{(existed ? "Rewrote" : "Created")} {path} ({contents.Length} characters).";
        }
        catch (Exception ex)
        {
            return $"Could not write '{path}': {ex.Message}";
        }
    }

    [KernelFunction("edit_file")]
    [Description("Replace one exact block of text in a file. Prefer this over rewriting a whole file for small changes.")]
    public string EditFile(
        [Description("Path relative to the project root.")] string path,
        [Description("The exact text to find. Must match once.")] string find,
        [Description("The text to put in its place.")] string replace)
    {
        if (!TryResolve(path, out var target, out var error)) return error;
        if (!File.Exists(target)) return $"No file at '{path}'.";

        var original = File.ReadAllText(target);
        var occurrences = CountOccurrences(original, find);
        if (occurrences == 0) return $"That text does not appear in '{path}'.";
        if (occurrences > 1)
        {
            return $"That text appears {occurrences} times in '{path}'. Include more surrounding lines to make it unique.";
        }

        File.WriteAllText(target, original.Replace(find, replace), new UTF8Encoding(false));
        log($"Jack edited {path}");
        return $"Edited {path}.";
    }

    [KernelFunction("delete_file")]
    [Description("Delete a file from the project.")]
    public string DeleteFile(
        [Description("Path relative to the project root.")] string path)
    {
        if (!TryResolve(path, out var target, out var error)) return error;
        if (!File.Exists(target)) return $"No file at '{path}'.";

        File.Delete(target);
        log($"Jack deleted {path}");
        return $"Deleted {path}.";
    }

    [KernelFunction("search_project")]
    [Description("Find which files contain a piece of text. Use this to locate code before changing it.")]
    public string SearchProject(
        [Description("The text to look for.")] string query)
    {
        if (projects.CurrentProjectPath is null) return "No project is open.";
        if (string.IsNullOrWhiteSpace(query)) return "Give me something to search for.";

        var hits = new StringBuilder();
        var matches = 0;
        foreach (var file in Directory.EnumerateFiles(projects.CurrentProjectPath, "*.*", SearchOption.AllDirectories))
        {
            if (file.Contains($"{Path.DirectorySeparatorChar}bin{Path.DirectorySeparatorChar}")
                || file.Contains($"{Path.DirectorySeparatorChar}obj{Path.DirectorySeparatorChar}")
                || file.Contains($"{Path.DirectorySeparatorChar}.git{Path.DirectorySeparatorChar}"))
            {
                continue;
            }

            string[] lines;
            try
            {
                if (new FileInfo(file).Length > settings.MaxFileSizeKB * 1024) continue;
                lines = File.ReadAllLines(file);
            }
            catch
            {
                continue; // binary or locked; skip rather than fail the search
            }

            for (var i = 0; i < lines.Length; i++)
            {
                if (!lines[i].Contains(query, StringComparison.OrdinalIgnoreCase)) continue;
                hits.AppendLine($"{Path.GetRelativePath(projects.CurrentProjectPath, file)}:{i + 1}: {lines[i].Trim()}");
                if (++matches >= 60)
                {
                    hits.AppendLine("(stopped at 60 matches)");
                    return hits.ToString();
                }
            }
        }
        return matches == 0 ? $"No matches for '{query}'." : hits.ToString();
    }

    [KernelFunction("create_project")]
    [Description("Create a new project from a template. Call list_templates first to see the options.")]
    public string CreateProject(
        [Description("Folder the project will be created inside.")] string parentDirectory,
        [Description("Name of the project. Becomes the folder and assembly name.")] string name,
        [Description("Template id, or 'blank' for an empty console project.")] string templateId = "blank")
    {
        var template = TemplateCatalog.Find(templateId);
        if (template is null)
        {
            var ids = string.Join(", ", TemplateCatalog.All.Select(t => t.Id));
            return $"No template called '{templateId}'. Available: {ids}";
        }

        try
        {
            var opened = projects.Create(parentDirectory, name, template);
            log($"Jack created project {name} from '{template.Name}'");
            return $"Created {name} from the {template.Name} template at {projects.CurrentProjectPath}. Opened {Path.GetFileName(opened)}.";
        }
        catch (Exception ex)
        {
            return $"Could not create the project: {ex.Message}";
        }
    }

    [KernelFunction("list_templates")]
    [Description("List the project templates available, with their ids and what they produce.")]
    public string ListTemplates()
    {
        var builder = new StringBuilder();
        foreach (var group in TemplateCatalog.All.GroupBy(t => t.Category))
        {
            builder.AppendLine($"## {group.Key}");
            foreach (var template in group)
            {
                builder.AppendLine($"- {template.Id} — {template.Name} ({template.DomainLabel}): {template.Summary}");
            }
        }
        return builder.ToString();
    }

    /// <summary>
    /// Resolves a project-relative path, refusing anything outside the project.
    /// </summary>
    private bool TryResolve(string relative, out string fullPath, out string error)
    {
        fullPath = "";
        error = "";

        if (projects.CurrentProjectPath is null)
        {
            error = "No project is open. Create or open one first.";
            return false;
        }

        var root = Path.GetFullPath(projects.CurrentProjectPath);
        var combined = Path.GetFullPath(Path.Combine(root, relative ?? ""));

        if (!combined.StartsWith(root, StringComparison.OrdinalIgnoreCase))
        {
            error = $"'{relative}' is outside the open project. Paths must stay inside {root}.";
            return false;
        }

        fullPath = combined;
        return true;
    }

    private static int CountOccurrences(string haystack, string needle)
    {
        if (string.IsNullOrEmpty(needle)) return 0;
        var count = 0;
        var index = 0;
        while ((index = haystack.IndexOf(needle, index, StringComparison.Ordinal)) >= 0)
        {
            count++;
            index += needle.Length;
        }
        return count;
    }
}
