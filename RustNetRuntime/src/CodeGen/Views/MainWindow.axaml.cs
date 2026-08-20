using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.Markup.Xaml;
using Avalonia.Platform.Storage;
using AvaloniaEdit;
using AvaloniaEdit.TextMate;
using CodeGen.Models;
using CodeGen.ViewModels;
using TextMateSharp.Grammars;

namespace CodeGen.Views;

public partial class MainWindow : Window, IWorkspaceDialogs
{
    private TextMate.Installation? _textMate;
    private RegistryOptions? _grammars;

    public MainWindow()
    {
        AvaloniaXamlLoader.Load(this);

        var editor = this.FindControl<TextEditor>("Editor");
        if (editor is not null)
        {
            // Syntax highlighting, driven by the open file's extension.
            _grammars = new RegistryOptions(ThemeName.DarkPlus);
            _textMate = editor.InstallTextMate(_grammars);
            editor.TextArea.Caret.PositionChanged += OnCaretMoved;
            editor.Options.ConvertTabsToSpaces = true;
        }

        var chatBox = this.FindControl<TextBox>("ChatBox");
        if (chatBox is not null)
        {
            chatBox.KeyDown += OnChatKeyDown;
        }

        DataContextChanged += OnDataContextChanged;
        Opened += (_, _) => ApplyGrammar();
    }

    private MainWindowViewModel? ViewModel => DataContext as MainWindowViewModel;

    private void OnDataContextChanged(object? sender, EventArgs e)
    {
        if (ViewModel is not { } viewModel) return;

        viewModel.Dialogs = this;
        viewModel.RequestGoToLine += GoToLine;
        viewModel.PropertyChanged += (_, args) =>
        {
            if (args.PropertyName == nameof(MainWindowViewModel.SelectedTab)) ApplyGrammar();
        };
    }

    /// <summary>Ctrl+Enter sends; a bare Enter keeps writing.</summary>
    private void OnChatKeyDown(object? sender, KeyEventArgs e)
    {
        if (e.Key != Key.Enter) return;
        if (!e.KeyModifiers.HasFlag(KeyModifiers.Control)) return;

        e.Handled = true;
        if (ViewModel?.SendChatCommand.CanExecute(null) == true)
        {
            ViewModel.SendChatCommand.Execute(null);
        }
    }

    private void OnCaretMoved(object? sender, EventArgs e)
    {
        var editor = this.FindControl<TextEditor>("Editor");
        if (editor is null || ViewModel is null) return;
        var caret = editor.TextArea.Caret;
        ViewModel.CursorPosition = $"Ln {caret.Line}, Col {caret.Column}";
    }

    /// <summary>Switches the TextMate grammar to match the selected file.</summary>
    private void ApplyGrammar()
    {
        if (_textMate is null || _grammars is null) return;
        var extension = Path.GetExtension(ViewModel?.SelectedTab?.FilePath ?? "");
        if (string.IsNullOrEmpty(extension)) return;

        // TextMate has no Rust or TOML grammar in the default registry for some
        // builds; falling back to no highlighting beats throwing.
        try
        {
            var language = _grammars.GetLanguageByExtension(extension);
            if (language is null) return;
            _textMate.SetGrammar(_grammars.GetScopeByLanguageId(language.Id));
        }
        catch (Exception)
        {
            _textMate.SetGrammar(null);
        }
    }

    private void GoToLine(int line)
    {
        var editor = this.FindControl<TextEditor>("Editor");
        if (editor is null) return;

        var target = Math.Clamp(line, 1, editor.Document.LineCount);
        editor.ScrollToLine(target);
        editor.TextArea.Caret.Line = target;
        editor.TextArea.Caret.Column = 1;
        editor.Focus();
    }

    private void OnTreeDoubleTapped(object? sender, TappedEventArgs e)
    {
        if (sender is not TreeView tree) return;
        if (tree.SelectedItem is not FileNodeViewModel node || node.IsDirectory) return;
        ViewModel?.OpenFile(node.FullPath);
    }

    private void OnCloseTab(object? sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: EditorTabViewModel tab })
        {
            ViewModel?.CloseTabCommand.Execute(tab);
        }
    }

    private void OnExit(object? sender, RoutedEventArgs e)
    {
        ViewModel?.PersistSettings();
        Close();
    }

    private async void OnAbout(object? sender, RoutedEventArgs e) =>
        await ShowMessageAsync(
            "CodeGen",
            "CodeGen — the IDE for RustNetRuntime.\n\n"
            + "C# stays the language; RustCLR is the runtime beneath it, rebuilt in Rust.\n"
            + "The assistant is Jack — The Code Bender.\n\n"
            + "Built by Gravicode Studios, led by Kang Fadhil.");

    // ══ IWorkspaceDialogs ═════════════════════════════════════════════════

    public async Task<string?> PickFolderAsync(string title)
    {
        var folders = await StorageProvider.OpenFolderPickerAsync(new FolderPickerOpenOptions
        {
            Title = title,
            AllowMultiple = false,
        });
        return folders.Count > 0 ? folders[0].TryGetLocalPath() : null;
    }

    public async Task<string?> PickFileAsync(string title)
    {
        var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = title,
            AllowMultiple = false,
            FileTypeFilter =
            [
                new FilePickerFileType("Source files")
                {
                    Patterns = ["*.cs", "*.rs", "*.csproj", "*.axaml", "*.xaml", "*.json", "*.toml", "*.md", "*.config"],
                },
                FilePickerFileTypes.All,
            ],
        });
        return files.Count > 0 ? files[0].TryGetLocalPath() : null;
    }

    public async Task<IReadOnlyList<string>> PickImagesAsync()
    {
        var files = await StorageProvider.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = "Attach images",
            AllowMultiple = true,
            FileTypeFilter = [FilePickerFileTypes.ImageAll],
        });
        return [.. files.Select(f => f.TryGetLocalPath()).Where(p => p is not null).Select(p => p!)];
    }

    public async Task<NewProjectRequest?> NewProjectAsync()
    {
        var dialog = new NewProjectWindow();
        return await dialog.ShowDialog<NewProjectRequest?>(this);
    }

    public async Task ShowSettingsAsync(AppSettings settings)
    {
        var dialog = new SettingsWindow(settings);
        await dialog.ShowDialog(this);
    }

    public async Task<int?> AskLineNumberAsync(int maxLine)
    {
        var dialog = new GoToLineWindow(maxLine);
        return await dialog.ShowDialog<int?>(this);
    }

    public async Task ShowMessageAsync(string title, string message)
    {
        var dialog = new MessageWindow(title, message);
        await dialog.ShowDialog(this);
    }
}
