using RustNet.Graphics;
using RustNet.Hal;
using RustNet.IO;
using RustNet.Media;
using RustNet.Net;
using RustNet.Sys;
using RustNet.Threading;
using RustNet.UI;

namespace Kiosk;

/// <summary>
/// Which pads the Maix Go's three buttons sit on.
///
/// The reference kept these behind firmware internal calls so one compiled
/// module could run on any pinout. That trade is not worth it here: RustCLR
/// dispatches a native only for a method with no IL body, so an internal call
/// has to be registered by the firmware — which means a program using one can
/// no longer run on <c>--board host</c>, and cannot be tried at all without
/// flashing. Constants and the ordinary <c>Gpio</c> bindings run in the
/// simulator and on the board from the same source, and a different pinout is
/// three numbers rather than a firmware build.
///
/// The values are the Maix Go's, taken from the reference firmware's
/// <c>board</c> module rather than measured here.
/// </summary>
internal static class Buttons
{
    public const int Up = 17;
    public const int Middle = 15;
    public const int Down = 16;
}

/// <summary>
/// A two-screen kiosk for the Maix Go: a camera screen that photographs to
/// the board's filesystem, and a dashboard that publishes to an MQTT broker
/// and shows what comes back. Up and down switch screens; the middle button
/// acts on the screen you are looking at.
///
/// Everything here runs interpreted on the K210 — the C# is the application,
/// not a description of one.
/// </summary>
internal static class Program
{
    private const int Width = 320;
    private const int Height = 240;

    /// <summary>Which screen is showing. Not an enum: the interpreter's
    /// generics are erased and an int compares in one instruction.</summary>
    private const int ScreenCamera = 0;
    private const int ScreenDashboard = 1;

    /// <summary>
    /// The topic tree this device owns. A single prefix keeps a broker shared
    /// with other devices legible, and makes one subscription enough to hear
    /// everything addressed to us.
    /// </summary>
    private const string TopicPrefix = "rustnet/maixgo/";

    private static int _screen = ScreenCamera;
    private static int _photos;
    private static string _status = "starting";
    private static string _lastMessage = "";
    private static int _published;
    private static bool _pollFailed;

    private static void Main()
    {
        Display.Init(Width, Height);
        Gpio.SetMode(Buttons.Up, PinMode.InputPullUp);
        Gpio.SetMode(Buttons.Down, PinMode.InputPullUp);
        Gpio.SetMode(Buttons.Middle, PinMode.InputPullUp);

        Splash("bringing up the camera");
        Camera.Configure(Width, Height);

        Splash("joining the network");
        // Empty credentials mean "use what `rustnet wifi` provisioned". The
        // app carries none, and never should.
        bool joined = Wifi.Connect("", "");
        Status(joined ? "wifi " + Wifi.GetIp() : "no wifi");

        if (joined)
        {
            Splash("connecting to the broker");
            ConnectBroker();
        }

        // The three buttons are pulled up and short to ground, so a pin reads
        // *false* while its button is held. Edges rather than levels: a held
        // button would otherwise switch screens every frame.
        bool upWas = false;
        bool downWas = false;
        bool middleWas = false;

        while (true)
        {
            bool up = !Gpio.Read(Buttons.Up);
            bool down = !Gpio.Read(Buttons.Down);
            bool middle = !Gpio.Read(Buttons.Middle);

            if (up && !upWas) _screen = ScreenCamera;
            if (down && !downWas) _screen = ScreenDashboard;
            if (middle && !middleWas) Act();

            upWas = up;
            downWas = down;
            middleWas = middle;

            // Poll every frame, on whichever screen. A broker's messages do
            // not wait for the user to look at the dashboard, and one polled
            // only while that screen is showing arrives in a burst whenever
            // it is — which reads as a device that ignores its broker.
            PollBroker();

            if (_screen == ScreenCamera) DrawCamera();
            else DrawDashboard();

            Display.Present();

            // Yield, briefly and deliberately.
            //
            // The firmware answers `rustnet` between interpreter fuel slices
            // and from inside `Sleep`, so a render loop that never sleeps is a
            // device the tools cannot reach: `rustnet flash` times out against
            // a board that is running perfectly and drawing every frame. Ten
            // milliseconds is invisible next to a frame here, and it is the
            // difference between a demo you can update over the wire and one
            // you have to reflash in ISP mode.
            Sleep.Ms(10);
        }
    }

    /// <summary>What the middle button does depends on what is showing.</summary>
    private static void Act()
    {
        if (_screen == ScreenCamera) TakePhoto();
        else PublishReading();
    }

    // ---------------------------------------------------------------- camera

    /// <summary>
    /// Capture a frame and write it to the board's flash filesystem.
    ///
    /// Saved as raw RGB565 rather than as an image format: there is no JPEG
    /// encoder on this device, and inventing a header the tools cannot read
    /// would be worse than a file whose shape is written on the label.
    /// </summary>
    private static void TakePhoto()
    {
        _status = "capturing";
        DrawCamera();
        Display.Present();

        byte[] frame = Camera.Capture();
        string name = "photo-" + _photos + "-" + Width + "x" + Height + ".rgb565";
        FileSystem.WriteAllBytes(name, frame);
        _photos++;
        Status("saved " + name);

        // A photograph is worth telling the broker about, if there is one.
        Publish(TopicPrefix + "photo", name);
    }

    private static void DrawCamera()
    {
        byte[] frame = Camera.Capture();
        Display.DrawImage(0, 0, Width, Height, frame);

        // The overlay is drawn directly rather than through a UI tree. A host
        // call costs around 220 microseconds on this chip, and a layout pass
        // over a tree spends hundreds of them before a single pixel moves —
        // affordable once a second on a dashboard, not on every camera frame.
        Display.FillRect(0, Height - 28, Width, 28, UiColors.Black);
        Display.DrawText(6, Height - 20, "UP camera  DOWN dash  MID photo",
            UiColors.White, 1);
        Display.DrawText(6, 6, _photos + " saved", UiColors.Green, 1);
    }

    // ------------------------------------------------------------- dashboard

    /// <summary>
    /// The dashboard is a RustNet.UI tree: a stack of labels and a progress
    /// bar, laid out and rendered by the toolkit rather than positioned by
    /// hand. It is rebuilt each frame because the screen is small and the
    /// values all change; a cached tree would save a few allocations and cost
    /// the clarity of seeing the whole screen in one function.
    /// </summary>
    private static void DrawDashboard()
    {
        Display.Clear(UiColors.Black);

        UiElement root = UiElement.Make("stack");
        root.Width = Width;
        root.Height = Height;
        root.Padding = 8;
        root.Gap = 6;
        root.Background = UiColors.Black;

        UiElement title = UiElement.Label("RustNet on RISC-V");
        title.Scale = 2;
        title.Foreground = UiColors.Cyan;
        root.Children.Add(title);

        root.Children.Add(Line("net:  " + _status, UiColors.White));
        root.Children.Add(Line("sent: " + _published + " to " + TopicPrefix, UiColors.Gray));
        root.Children.Add(Line("recv: " + (_lastMessage.Length == 0 ? "(nothing yet)" : _lastMessage),
            UiColors.Yellow));
        root.Children.Add(Line("photos: " + _photos, UiColors.Green));

        UiElement bar = UiElement.Make("progress");
        bar.Width = Width - 16;
        bar.Height = 12;
        bar.Min = 0;
        bar.Max = 100;
        // Something that visibly moves, so a frozen screen is distinguishable
        // from an idle one at a glance.
        bar.Value = (int)(Uptime.Ms() / 100 % 101);
        bar.Foreground = UiColors.Cyan;
        root.Children.Add(bar);

        root.Children.Add(Line("UP camera  DOWN dash  MID publish", UiColors.Gray));

        Ui.Render(root);
    }

    private static UiElement Line(string text, int colour)
    {
        UiElement e = UiElement.Label(text);
        e.Foreground = colour;
        e.Scale = 1;
        return e;
    }

    // ------------------------------------------------------------------ mqtt

    /// <summary>
    /// The broker address, and why it is not a constant here.
    ///
    /// A hostname baked into an application is a hostname that is wrong on
    /// somebody else's network. This reads it from the filesystem, where
    /// `rustnet` can write it, and falls back to the address a broker most
    /// often has on a development network.
    /// </summary>
    private static string BrokerAddress()
    {
        if (FileSystem.Exists("broker.txt"))
        {
            string configured = FileSystem.ReadAllText("broker.txt").Trim();
            if (configured.Length > 0) return configured;
        }
        return "test.mosquitto.org:1883";
    }

    private static void ConnectBroker()
    {
        string address = BrokerAddress();
        try
        {
            if (Mqtt.Connect(address, "maixgo-kiosk"))
            {
                Mqtt.Subscribe(TopicPrefix + "cmd");
                Status("broker " + address);
            }
        }
        catch (Exception e)
        {
            // A missing broker is an ordinary condition on a development
            // desk, not a reason to stop showing the camera.
            Status("broker: " + e.Message);
        }
    }

    /// <summary>
    /// Record a status line, and say it out loud.
    ///
    /// The screen is the interface, but the console is how this device is
    /// checked when nobody is looking at it — and "did the broker connect?"
    /// is not a question a photograph of a screen answers quickly.
    /// </summary>
    private static void Status(string what)
    {
        _status = what;
        Console.WriteLine("kiosk: " + what);
    }

    private static void PublishReading()
    {
        Publish(TopicPrefix + "uptime", Uptime.Ms().ToString());
    }

    private static void Publish(string topic, string payload)
    {
        try
        {
            Mqtt.Publish(topic, payload, 0);
            _published++;
            Console.WriteLine("kiosk: published " + topic + " = " + payload);
        }
        catch (Exception e)
        {
            Status("publish: " + e.Message);
        }
    }

    private static void PollBroker()
    {
        try
        {
            // "topic\npayload", or empty when nothing has arrived. On this
            // board the call does not block — a UI loop cannot afford a poll
            // that waits on a quiet broker.
            string message = Mqtt.Poll();
            if (message.Length == 0) return;

            int split = message.IndexOf('\n');
            _lastMessage = split < 0 ? message : message.Substring(split + 1);
        }
        catch (Exception e)
        {
            // No broker: the dashboard still draws. Reported once rather than
            // every frame — a poll that fails fails at frame rate, and a
            // screenful of the same line hides everything else. Reported at
            // all because a silently swallowed poll is indistinguishable from
            // a broker with nothing to say, and this demo spent a debugging
            // round on exactly that.
            if (!_pollFailed)
            {
                _pollFailed = true;
                Status("poll: " + e.Message);
            }
        }
    }

    // ----------------------------------------------------------------- chrome

    /// <summary>A progress line during the slow parts of start-up — the camera
    /// settles over frames and a join can take seconds, and a black screen for
    /// that long reads as a crash.</summary>
    private static void Splash(string what)
    {
        Display.Clear(UiColors.Black);
        Display.DrawText(20, Height / 2 - 12, "RustNet", UiColors.Cyan, 3);
        Display.DrawText(20, Height / 2 + 20, what, UiColors.White, 1);
        Display.Present();
    }
}
