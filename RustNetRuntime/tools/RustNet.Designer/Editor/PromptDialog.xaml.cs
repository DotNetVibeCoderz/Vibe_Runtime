using System.Windows;
using System.Windows.Input;

namespace RustNet.Designer.Editor;

/// <summary>
/// A one-line question in the tool's own colours — WPF's built-in input box is a
/// VB6 relic and a white MessageBox would break the theme on the one dialog the
/// Designer needs.
/// </summary>
public partial class PromptDialog : Window
{
    private PromptDialog(string title, string label, string initial)
    {
        InitializeComponent();
        Title = title;
        Label.Text = label;
        Input.Text = initial;
        Loaded += (_, _) =>
        {
            DarkTitleBar.Apply(this);
            Input.Focus();
            Input.SelectAll();
        };
    }

    /// <summary>The typed answer, or null if it was cancelled.</summary>
    public static string? Ask(Window? owner, string title, string label, string initial = "")
    {
        var dialog = new PromptDialog(title, label, initial);
        if (owner != null)
        {
            dialog.Owner = owner;
        }
        return dialog.ShowDialog() == true ? dialog.Input.Text : null;
    }

    private void OnOk(object sender, RoutedEventArgs e) => DialogResult = true;

    private void OnCancel(object sender, RoutedEventArgs e) => DialogResult = false;

    private void OnKeyDown(object sender, KeyEventArgs e)
    {
        if (e.Key == Key.Escape)
        {
            e.Handled = true;
            DialogResult = false;
        }
    }
}
