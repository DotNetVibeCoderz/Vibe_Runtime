using RustNet.UI;

namespace __NAME__;

/// <summary>
/// Display dashboard: the layout lives in an XML file on the device
/// filesystem (WPF/Glide-style), gets loaded at startup and refreshed with
/// live ADC readings. Capture the panel with `rustnet display capture` or
/// watch it in the VSCode simulator panel.
/// </summary>
public static class Program
{
    private const string Layout =
        "<window width=\"160\" height=\"128\" bg=\"0000\" pad=\"4\" gap=\"4\">" +
        "<label id=\"title\" text=\"__NAME__\" scale=\"2\" fg=\"07FF\"/>" +
        "<label id=\"reading\" text=\"--\" fg=\"FFFF\"/>" +
        "<progress id=\"bar\" value=\"0\" max=\"3300\" fg=\"07E0\"/>" +
        "<button text=\"OK\" bg=\"4208\" fg=\"FFFF\"/>" +
        "</window>";

    public static void Main()
    {
        Console.WriteLine("ui-dashboard starting");

        // First boot: seed the layout file so it can be tweaked on-device.
        if (!RustNet.IO.FileSystem.Exists("/data/ui.xml"))
        {
            RustNet.IO.FileSystem.WriteAllText("/data/ui.xml", Layout);
        }
        UiElement screen = Ui.LoadXml(RustNet.IO.FileSystem.ReadAllText("/data/ui.xml"));
        UiElement reading = screen.FindById("reading");
        UiElement bar = screen.FindById("bar");

        for (int i = 0; i < 10; i++)
        {
            int mv = RustNet.Hal.Adc.ReadMillivolts(0);
            reading.Text = $"ADC0: {mv} mV";
            bar.Value = mv;
            Ui.Render(screen);
            RustNet.Threading.Sleep.Ms(500);
        }

        Console.WriteLine("ui-dashboard finished");
    }
}
