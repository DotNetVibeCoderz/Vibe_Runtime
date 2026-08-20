using Avalonia.Controls;
using Avalonia.Interactivity;
using Avalonia.Markup.Xaml;

namespace CodeGen.Views;

public partial class MessageWindow : Window
{
    public MessageWindow() : this("CodeGen", "") { }

    public MessageWindow(string title, string message)
    {
        AvaloniaXamlLoader.Load(this);
        Title = title;
        if (this.FindControl<TextBlock>("Heading") is { } heading) heading.Text = title.ToUpperInvariant();
        if (this.FindControl<TextBlock>("Body") is { } body) body.Text = message;
    }

    private void OnClose(object? sender, RoutedEventArgs e) => Close();
}
