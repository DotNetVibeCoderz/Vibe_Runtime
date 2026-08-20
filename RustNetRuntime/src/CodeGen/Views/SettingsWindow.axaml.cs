using System.Globalization;
using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Layout;
using Avalonia.Markup.Xaml;
using Avalonia.Media;
using CodeGen.Models;

namespace CodeGen.Views;

/// <summary>
/// Every setting in <c>app.config</c>, editable.
///
/// The provider rows are built in code rather than XAML: there is one per
/// <see cref="LlmProvider"/>, and generating them keeps the dialog correct if a
/// provider is ever added.
/// </summary>
public partial class SettingsWindow : Window
{
    private readonly AppSettings _settings;
    private readonly Dictionary<LlmProvider, (TextBox Model, TextBox Key, TextBox Endpoint)> _providerFields = [];

    // Parameterless constructor for the XAML previewer.
    public SettingsWindow() : this(new AppSettings()) { }

    public SettingsWindow(AppSettings settings)
    {
        _settings = settings;
        AvaloniaXamlLoader.Load(this);

        // The previewer hands us an empty settings object; fill it so the
        // dialog has something to bind against.
        if (_settings.Providers.Count == 0)
        {
            foreach (var provider in Enum.GetValues<LlmProvider>())
            {
                _settings.Providers[provider] = new ProviderSettings { Provider = provider };
            }
        }

        BuildProviderRows();
        Load();
    }

    private void BuildProviderRows()
    {
        var host = this.FindControl<StackPanel>("ProviderRows");
        if (host is null) return;

        foreach (var provider in Enum.GetValues<LlmProvider>())
        {
            var values = _settings.Providers[provider];

            var model = new TextBox { Text = values.Model, Watermark = "model id" };
            var key = new TextBox { Text = values.ApiKey, PasswordChar = '•', Watermark = "API key" };
            var endpoint = new TextBox { Text = values.Endpoint, Watermark = "endpoint" };
            _providerFields[provider] = (model, key, endpoint);

            var grid = new Grid
            {
                ColumnDefinitions = new ColumnDefinitions("90,*"),
                RowDefinitions = new RowDefinitions("Auto,Auto,Auto"),
            };

            void AddRow(int row, string label, Control field)
            {
                // Spacing is expressed as margins: Grid.ColumnSpacing is a XAML
                // convenience that has no code-behind equivalent in Avalonia 11.
                var text = new TextBlock
                {
                    Text = label,
                    VerticalAlignment = VerticalAlignment.Center,
                    FontSize = 11,
                    Foreground = Brushes.Gray,
                    Margin = new Avalonia.Thickness(0, 3, 10, 3),
                };
                field.Margin = new Avalonia.Thickness(0, 3, 0, 3);
                Grid.SetRow(text, row);
                Grid.SetColumn(text, 0);
                Grid.SetRow(field, row);
                Grid.SetColumn(field, 1);
                grid.Children.Add(text);
                grid.Children.Add(field);
            }

            AddRow(0, "Model", model);
            AddRow(1, "API key", key);
            AddRow(2, "Endpoint", endpoint);

            var header = new TextBlock
            {
                Text = values.DisplayName.ToUpperInvariant(),
                FontWeight = FontWeight.SemiBold,
                FontSize = 11,
                Margin = new Avalonia.Thickness(0, 0, 0, 6),
            };
            header.Classes.Add("panel-label");

            var panel = new StackPanel { Spacing = 4 };
            panel.Children.Add(header);
            panel.Children.Add(grid);

            var border = new Border
            {
                Padding = new Avalonia.Thickness(12),
                CornerRadius = new Avalonia.CornerRadius(3),
                Child = panel,
            };
            border.Classes.Add("panel");

            host.Children.Add(border);
        }
    }

    private void Load()
    {
        // ConfigPath is a TextBlock; it is filled at the end of this method.
        var providerBox = this.FindControl<ComboBox>("Provider");
        if (providerBox is not null)
        {
            providerBox.ItemsSource = Enum.GetValues<LlmProvider>();
            providerBox.SelectedItem = _settings.ActiveProvider;
        }

        Set("Temperature", _settings.Temperature.ToString(CultureInfo.InvariantCulture));
        Set("MaxTokens", _settings.MaxTokens.ToString(CultureInfo.InvariantCulture));
        Set("SystemPrompt", _settings.SystemPrompt);
        Set("TavilyKey", _settings.TavilyApiKey);
        Set("MaxFileSize", _settings.MaxFileSizeKB.ToString(CultureInfo.InvariantCulture));
        Set("RustNetPath", _settings.RustNetPath);
        Set("DotnetPath", _settings.DotnetPath);
        Set("FontSize", _settings.EditorFontSize.ToString(CultureInfo.InvariantCulture));
        Set("TabSize", _settings.TabSize.ToString(CultureInfo.InvariantCulture));

        Check("AllowShell", _settings.AllowShellCommands);
        Check("LineNumbers", _settings.ShowLineNumbers);
        Check("WordWrap", _settings.WordWrap);

        var path = this.FindControl<TextBlock>("ConfigPath");
        if (path is not null)
        {
            path.Text = System.Configuration.ConfigurationManager
                .OpenExeConfiguration(System.Configuration.ConfigurationUserLevel.None).FilePath;
        }
    }

    private void OnSave(object? sender, RoutedEventArgs e)
    {
        if (this.FindControl<ComboBox>("Provider")?.SelectedItem is LlmProvider chosen)
        {
            _settings.ActiveProvider = chosen;
        }

        _settings.Temperature = ReadDouble("Temperature", _settings.Temperature);
        _settings.MaxTokens = ReadInt("MaxTokens", _settings.MaxTokens);
        _settings.SystemPrompt = Read("SystemPrompt");
        _settings.TavilyApiKey = Read("TavilyKey");
        _settings.MaxFileSizeKB = ReadInt("MaxFileSize", _settings.MaxFileSizeKB);
        _settings.RustNetPath = Read("RustNetPath");
        _settings.DotnetPath = Read("DotnetPath");
        _settings.EditorFontSize = ReadDouble("FontSize", _settings.EditorFontSize);
        _settings.TabSize = ReadInt("TabSize", _settings.TabSize);

        _settings.AllowShellCommands = IsChecked("AllowShell");
        _settings.ShowLineNumbers = IsChecked("LineNumbers");
        _settings.WordWrap = IsChecked("WordWrap");

        foreach (var (provider, fields) in _providerFields)
        {
            var values = _settings.Providers[provider];
            values.Model = fields.Model.Text?.Trim() ?? "";
            values.ApiKey = fields.Key.Text?.Trim() ?? "";
            values.Endpoint = fields.Endpoint.Text?.Trim() ?? "";
        }

        Close();
    }

    private void OnCancel(object? sender, RoutedEventArgs e) => Close();

    // -- small helpers -------------------------------------------------------

    private void Set(string name, string value)
    {
        if (this.FindControl<TextBox>(name) is { } box) box.Text = value;
    }

    private string Read(string name) => this.FindControl<TextBox>(name)?.Text?.Trim() ?? "";

    private double ReadDouble(string name, double fallback) =>
        double.TryParse(Read(name), NumberStyles.Float, CultureInfo.InvariantCulture, out var v) ? v : fallback;

    private int ReadInt(string name, int fallback) =>
        int.TryParse(Read(name), NumberStyles.Integer, CultureInfo.InvariantCulture, out var v) ? v : fallback;

    private void Check(string name, bool value)
    {
        if (this.FindControl<CheckBox>(name) is { } box) box.IsChecked = value;
    }

    private bool IsChecked(string name) => this.FindControl<CheckBox>(name)?.IsChecked ?? false;
}
