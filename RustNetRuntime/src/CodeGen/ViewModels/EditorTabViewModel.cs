using AvaloniaEdit.Document;
using CommunityToolkit.Mvvm.ComponentModel;

namespace CodeGen.ViewModels;

/// <summary>One open file in the editor.</summary>
public sealed partial class EditorTabViewModel : ObservableObject
{
    [ObservableProperty]
    private bool _isDirty;

    public EditorTabViewModel(string path)
    {
        FilePath = path;
        FileName = Path.GetFileName(path);
        Document = new TextDocument(File.Exists(path) ? File.ReadAllText(path) : "");
        Document.TextChanged += (_, _) => IsDirty = true;
    }

    public string FilePath { get; }
    public string FileName { get; }

    /// <summary>The buffer AvaloniaEdit edits in place.</summary>
    public TextDocument Document { get; }

    /// <summary>Tab caption, marked when there are unsaved changes.</summary>
    public string Caption => IsDirty ? $"{FileName} ●" : FileName;

    /// <summary>The TextMate grammar id for this file's extension.</summary>
    public string Language => Path.GetExtension(FilePath).ToLowerInvariant() switch
    {
        ".cs" => "csharp",
        ".rs" => "rust",
        ".json" => "json",
        ".xml" or ".csproj" or ".axaml" or ".config" or ".props" => "xml",
        ".md" => "markdown",
        ".toml" => "toml",
        ".yml" or ".yaml" => "yaml",
        _ => "text",
    };

    public void Save()
    {
        File.WriteAllText(FilePath, Document.Text);
        IsDirty = false;
    }

    /// <summary>Reloads from disk, discarding edits. Used after Jack writes a file.</summary>
    public void Reload()
    {
        if (!File.Exists(FilePath)) return;
        var text = File.ReadAllText(FilePath);
        if (text == Document.Text) return;
        Document.Text = text;
        IsDirty = false;
    }

    partial void OnIsDirtyChanged(bool value) => OnPropertyChanged(nameof(Caption));
}
