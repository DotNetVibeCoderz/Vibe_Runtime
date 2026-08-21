using Avalonia.Controls;
using Avalonia.Markup.Xaml;
using CodeGen.ViewModels;

namespace CodeGen.Views;

/// <summary>
/// The Devices panel.
///
/// A window rather than a dock in the main layout: flashing is a deliberate,
/// occasional act, and it borrows the log panel while it runs. Giving it a
/// permanent column would spend screen on something used once a session.
/// </summary>
public partial class DevicesWindow : Window
{
    // Parameterless constructor for the XAML previewer.
    public DevicesWindow() => AvaloniaXamlLoader.Load(this);

    public DevicesWindow(DevicesViewModel model)
    {
        AvaloniaXamlLoader.Load(this);
        DataContext = model;

        // A scan on open, so the panel is never blank on arrival. It only
        // lists ports and probes — nothing is written to any board.
        Opened += async (_, _) =>
        {
            if (model.ScanCommand.CanExecute(null))
            {
                await model.ScanCommand.ExecuteAsync(null);
            }
        };
    }
}
