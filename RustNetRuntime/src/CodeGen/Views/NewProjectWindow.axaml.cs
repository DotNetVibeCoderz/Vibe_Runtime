using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Markup.Xaml;
using Avalonia.Platform.Storage;
using CodeGen.Models;
using CodeGen.Services;
using CodeGen.ViewModels;

namespace CodeGen.Views;

public partial class NewProjectWindow : Window
{
    public NewProjectWindow()
    {
        AvaloniaXamlLoader.Load(this);

        var list = this.FindControl<ListBox>("Templates");
        if (list is not null)
        {
            list.ItemsSource = TemplateCatalog.All;
            list.SelectedIndex = 0;
            list.SelectionChanged += (_, _) => UpdatePreview();
        }

        var location = this.FindControl<TextBox>("Location");
        if (location is not null)
        {
            location.Text = DefaultLocation();
        }

        var name = this.FindControl<TextBox>("ProjectName");
        if (name is not null)
        {
            name.TextChanged += (_, _) => UpdatePreview();
        }

        UpdatePreview();
    }

    /// <summary>Somewhere writable that the user will recognise.</summary>
    private static string DefaultLocation()
    {
        var documents = Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments);
        var projects = Path.Combine(documents, "CodeGen Projects");
        return Directory.Exists(documents) ? projects : Environment.CurrentDirectory;
    }

    private void UpdatePreview()
    {
        var preview = this.FindControl<TextBlock>("Preview");
        var list = this.FindControl<ListBox>("Templates");
        var name = this.FindControl<TextBox>("ProjectName");
        if (preview is null || list is null) return;

        if (list.SelectedItem is not ProjectTemplate template)
        {
            preview.Text = "Pick a template.";
            return;
        }

        var projectName = ProjectService.SanitiseName(name?.Text ?? "");
        if (projectName.Length == 0) projectName = "MyApp";

        var files = template.Files
            .Select(f => f.RelativePath.Replace("{NAME}", projectName))
            .Take(6);

        preview.Text = string.Join('\n', files)
            + (template.Files.Count > 6 ? $"\n… and {template.Files.Count - 6} more" : "")
            + $"\n\nRun with:\n{template.RunHint.Replace("{NAME}", projectName)}";
    }

    private async void OnBrowse(object? sender, RoutedEventArgs e)
    {
        var folders = await StorageProvider.OpenFolderPickerAsync(new FolderPickerOpenOptions
        {
            Title = "Where should the project go?",
            AllowMultiple = false,
        });
        if (folders.Count == 0) return;

        var location = this.FindControl<TextBox>("Location");
        if (location is not null) location.Text = folders[0].TryGetLocalPath();
    }

    private void OnCreate(object? sender, RoutedEventArgs e)
    {
        var list = this.FindControl<ListBox>("Templates");
        var name = this.FindControl<TextBox>("ProjectName");
        var location = this.FindControl<TextBox>("Location");
        var problem = this.FindControl<TextBlock>("Problem");

        void Complain(string message)
        {
            if (problem is null) return;
            problem.Text = message;
            problem.IsVisible = true;
        }

        if (list?.SelectedItem is not ProjectTemplate template)
        {
            Complain("Pick a template first.");
            return;
        }

        var projectName = ProjectService.SanitiseName(name?.Text ?? "");
        if (projectName.Length == 0)
        {
            Complain("A project needs a name with at least one letter or digit.");
            return;
        }

        var parent = location?.Text?.Trim() ?? "";
        if (parent.Length == 0)
        {
            Complain("Choose where the project should go.");
            return;
        }

        try
        {
            Directory.CreateDirectory(parent);
        }
        catch (Exception ex)
        {
            Complain($"Cannot use that folder: {ex.Message}");
            return;
        }

        Close(new NewProjectRequest(parent, projectName, template));
    }

    private void OnCancel(object? sender, RoutedEventArgs e) => Close(null);
}
