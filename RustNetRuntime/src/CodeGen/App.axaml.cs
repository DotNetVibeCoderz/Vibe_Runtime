using Avalonia;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using CodeGen.ViewModels;
using CodeGen.Views;

namespace CodeGen;

public partial class App : Application
{
    public override void Initialize() => AvaloniaXamlLoader.Load(this);

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            var viewModel = new MainWindowViewModel();
            desktop.MainWindow = new MainWindow { DataContext = viewModel };

            // Persist layout and the open project on the way out.
            desktop.ShutdownRequested += (_, _) => viewModel.PersistSettings();
        }

        base.OnFrameworkInitializationCompleted();
    }
}
