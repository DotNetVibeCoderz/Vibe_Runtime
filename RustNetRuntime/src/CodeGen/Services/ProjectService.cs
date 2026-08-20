using System.Text;
using System.Text.RegularExpressions;
using CodeGen.Models;

namespace CodeGen.Services;

/// <summary>A file or folder in the explorer tree.</summary>
public sealed class ProjectNode
{
    public required string Name { get; init; }
    public required string FullPath { get; init; }
    public required bool IsDirectory { get; init; }
    public List<ProjectNode> Children { get; } = [];

    /// <summary>Icon glyph chosen by extension, so the tree reads at a glance.</summary>
    public string Glyph => IsDirectory
        ? "▸"
        : Path.GetExtension(Name).ToLowerInvariant() switch
        {
            ".cs" => "#",
            ".csproj" or ".sln" => "◇",
            ".rs" => "®",
            ".toml" or ".json" or ".config" or ".xml" => "≡",
            ".md" => "¶",
            _ => "·",
        };
}

/// <summary>Creating, opening and walking projects on disk.</summary>
public sealed partial class ProjectService
{
    /// <summary>Folders that would flood the tree and never need editing.</summary>
    private static readonly HashSet<string> IgnoredDirectories = new(StringComparer.OrdinalIgnoreCase)
    {
        "bin", "obj", ".git", ".vs", "node_modules", "target", ".idea",
    };

    public string? CurrentProjectPath { get; private set; }

    public string? CurrentProjectName =>
        CurrentProjectPath is null ? null : new DirectoryInfo(CurrentProjectPath).Name;

    public void Open(string path)
    {
        if (!Directory.Exists(path))
        {
            throw new DirectoryNotFoundException($"No folder at {path}");
        }
        CurrentProjectPath = Path.GetFullPath(path);
    }

    public void Close() => CurrentProjectPath = null;

    /// <summary>
    /// Creates a project from a template.
    /// </summary>
    /// <returns>The path of the file the editor should open first.</returns>
    public string Create(string parentDirectory, string projectName, ProjectTemplate template)
    {
        var safeName = SanitiseName(projectName);
        if (safeName.Length == 0)
        {
            throw new ArgumentException("A project name needs at least one letter or digit.", nameof(projectName));
        }

        var root = Path.Combine(parentDirectory, safeName);
        if (Directory.Exists(root) && Directory.EnumerateFileSystemEntries(root).Any())
        {
            throw new IOException($"{root} already exists and is not empty.");
        }
        Directory.CreateDirectory(root);

        var namespaceName = ToNamespace(safeName);
        string? firstSource = null;

        foreach (var file in template.Files)
        {
            var relative = Substitute(file.RelativePath, safeName, namespaceName);
            var target = Path.Combine(root, relative);
            Directory.CreateDirectory(Path.GetDirectoryName(target)!);
            File.WriteAllText(target, Substitute(file.Contents, safeName, namespaceName), new UTF8Encoding(false));

            if (firstSource is null && relative.EndsWith(".cs", StringComparison.OrdinalIgnoreCase))
            {
                firstSource = target;
            }
        }

        CurrentProjectPath = root;
        return firstSource ?? Path.Combine(root, template.Files[0].RelativePath);
    }

    /// <summary>Builds the explorer tree for the open project.</summary>
    public ProjectNode? BuildTree()
    {
        if (CurrentProjectPath is null) return null;
        return BuildNode(new DirectoryInfo(CurrentProjectPath), depth: 0);
    }

    private static ProjectNode? BuildNode(DirectoryInfo directory, int depth)
    {
        // Guard against symlink loops and pathological trees.
        if (depth > 24) return null;

        var node = new ProjectNode
        {
            Name = directory.Name,
            FullPath = directory.FullName,
            IsDirectory = true,
        };

        IEnumerable<FileSystemInfo> entries;
        try
        {
            entries = directory.EnumerateFileSystemInfos();
        }
        catch (UnauthorizedAccessException)
        {
            // An unreadable folder is shown as empty rather than failing the tree.
            return node;
        }

        foreach (var entry in entries.OrderBy(e => e is FileInfo).ThenBy(e => e.Name, StringComparer.OrdinalIgnoreCase))
        {
            if (entry is DirectoryInfo sub)
            {
                if (IgnoredDirectories.Contains(sub.Name) || sub.Name.StartsWith('.')) continue;
                var child = BuildNode(sub, depth + 1);
                if (child is not null) node.Children.Add(child);
            }
            else
            {
                node.Children.Add(new ProjectNode
                {
                    Name = entry.Name,
                    FullPath = entry.FullName,
                    IsDirectory = false,
                });
            }
        }

        return node;
    }

    /// <summary>Finds the project file to build, preferring a solution.</summary>
    public string? FindBuildTarget()
    {
        if (CurrentProjectPath is null) return null;

        var solution = Directory.EnumerateFiles(CurrentProjectPath, "*.sln", SearchOption.TopDirectoryOnly).FirstOrDefault();
        if (solution is not null) return solution;

        var project = Directory.EnumerateFiles(CurrentProjectPath, "*.csproj", SearchOption.AllDirectories)
            .FirstOrDefault(p => !p.Contains($"{Path.DirectorySeparatorChar}obj{Path.DirectorySeparatorChar}"));
        return project;
    }

    /// <summary>Locates the built assembly for a configuration.</summary>
    public string? FindOutputAssembly(string configuration)
    {
        if (CurrentProjectPath is null) return null;

        var candidates = Directory
            .EnumerateFiles(CurrentProjectPath, "*.dll", SearchOption.AllDirectories)
            .Where(p => p.Contains($"{Path.DirectorySeparatorChar}bin{Path.DirectorySeparatorChar}{configuration}{Path.DirectorySeparatorChar}"))
            .Where(p => !Path.GetFileName(p).StartsWith("Microsoft.", StringComparison.OrdinalIgnoreCase))
            .OrderByDescending(File.GetLastWriteTimeUtc)
            .ToList();

        // The project's own assembly shares the folder name with the project.
        var projectName = CurrentProjectName;
        return candidates.FirstOrDefault(p =>
                   Path.GetFileNameWithoutExtension(p).Equals(projectName, StringComparison.OrdinalIgnoreCase))
               ?? candidates.FirstOrDefault();
    }

    /// <summary>Replaces the template placeholders.</summary>
    private static string Substitute(string text, string name, string namespaceName) =>
        text.Replace("{NAMESPACE}", namespaceName).Replace("{NAME}", name);

    /// <summary>Strips characters that are not valid in a folder or assembly name.</summary>
    public static string SanitiseName(string raw)
    {
        var cleaned = InvalidNameCharacters().Replace(raw.Trim(), "");
        return cleaned.TrimStart('.', '-', '_');
    }

    /// <summary>Turns a project name into a valid C# namespace.</summary>
    public static string ToNamespace(string name)
    {
        var parts = name.Split(['.', '-', ' ', '_'], StringSplitOptions.RemoveEmptyEntries);
        var builder = new StringBuilder();
        foreach (var part in parts)
        {
            if (builder.Length > 0) builder.Append('.');
            // A namespace segment may not start with a digit.
            if (char.IsDigit(part[0])) builder.Append('_');
            builder.Append(char.ToUpperInvariant(part[0]));
            builder.Append(part.AsSpan(1));
        }
        return builder.Length == 0 ? "Application" : builder.ToString();
    }

    [GeneratedRegex(@"[^\w\.\- ]")]
    private static partial Regex InvalidNameCharacters();
}
