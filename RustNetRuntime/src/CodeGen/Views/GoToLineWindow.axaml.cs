using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Markup.Xaml;

namespace CodeGen.Views;

public partial class GoToLineWindow : Window
{
    private readonly int _maxLine;

    public GoToLineWindow() : this(1) { }

    public GoToLineWindow(int maxLine)
    {
        _maxLine = Math.Max(1, maxLine);
        AvaloniaXamlLoader.Load(this);

        if (this.FindControl<TextBlock>("Range") is { } range)
        {
            range.Text = $"1 to {_maxLine}";
        }
        Opened += (_, _) => this.FindControl<TextBox>("LineNumber")?.Focus();
    }

    private void OnGo(object? sender, RoutedEventArgs e)
    {
        var input = this.FindControl<TextBox>("LineNumber")?.Text?.Trim() ?? "";
        var problem = this.FindControl<TextBlock>("Problem");

        if (!int.TryParse(input, out var line))
        {
            if (problem is not null)
            {
                problem.Text = "That is not a line number.";
                problem.IsVisible = true;
            }
            return;
        }

        // Clamp rather than reject: asking for line 9999 in a 200-line file
        // clearly means "the end".
        Close(Math.Clamp(line, 1, _maxLine));
    }

    private void OnCancel(object? sender, RoutedEventArgs e) => Close(null);
}
