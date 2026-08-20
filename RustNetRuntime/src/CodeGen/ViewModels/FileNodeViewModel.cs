using System.Collections.ObjectModel;
using CodeGen.Services;
using CommunityToolkit.Mvvm.ComponentModel;

namespace CodeGen.ViewModels;

/// <summary>A node in the explorer tree.</summary>
public sealed partial class FileNodeViewModel : ObservableObject
{
    [ObservableProperty]
    private bool _isExpanded;

    public FileNodeViewModel(ProjectNode node)
    {
        Name = node.Name;
        FullPath = node.FullPath;
        IsDirectory = node.IsDirectory;
        Glyph = node.Glyph;
        Children = [.. node.Children.Select(c => new FileNodeViewModel(c))];
    }

    public string Name { get; }
    public string FullPath { get; }
    public bool IsDirectory { get; }
    public string Glyph { get; }
    public ObservableCollection<FileNodeViewModel> Children { get; }

    /// <summary>Folders are dimmer than files: the files are what you click.</summary>
    public double Opacity => IsDirectory ? 0.72 : 1.0;
}
