using RustNet.Graphics;
using RustNet.Net;
using RustNet.Sys;
using RustNet.Threading;

namespace __NAME__;

/// <summary>
/// A live MQTT dashboard drawn on the device's own panel: connects to a
/// broker, publishes telemetry on a timer, subscribes to a command topic and
/// renders every inbound message as it arrives — so the whole session is
/// visible on the glass without a serial console attached.
///
/// Built for the M5Stack Tough (320x240 ILI9342C), but the layout is computed
/// from whatever size Display reports, so smaller panels degrade gracefully.
/// Every state change is also written to the console, so `rustnet logs` shows
/// the same story without eyes on the panel.
///
/// Two runtime properties shape the loop, and both are deliberate:
///
///  * `Mqtt.Poll()` BLOCKS until a message arrives or the client's 10 s read
///    timeout expires. So the panel is repainted immediately *before* each
///    poll, and the header shows a "LISTENING" pill for exactly as long as the
///    app is parked in that call. At idle the dashboard therefore ticks about
///    once every 10 s — that is the client's timeout, not a hang.
///  * A dropped session is detected on PUBLISH (a write), not on poll: a poll
///    failure is indistinguishable from an idle timeout across platforms
///    (WSAETIMEDOUT on Windows, EAGAIN on lwIP), whereas a write to a dead
///    socket fails reliably.
/// </summary>
public static class Program
{
    // ---- Configuration --------------------------------------------------
    // An embedded app has no config file, so these are compiled in. Change
    // them here and re-flash. NOTE for ESP32 targets: the radio is joined by
    // the firmware at boot from credentials stored with `rustnet wifi --ssid
    // <s> --psk <p>`, and Ssid/Psk below only feed the on-screen status.
    // See README.md.
    private const string Ssid = "your-wifi-ssid";
    private const string Psk = "your-wifi-password";
    private const string Broker = "broker.hivemq.com:1883";
    private const string ClientId = "__NAME__";
    private const string TopicTelemetry = "rustnet/__NAME__/telemetry";
    private const string TopicCommand = "rustnet/__NAME__/cmd";

    // ---- Panel geometry -------------------------------------------------
    // BASIC_LEGACY glyphs are 8x8 and DrawText advances 8*scale per character,
    // so every column below is a multiple of 8.
    private const int Glyph = 8;
    private const int RowCount = 7;

    private static int W;
    private static int H;
    private static int _headerH;
    private static int _bannerY;
    private static int _noteY;
    private static int _rowsY;
    private static int _pitch;
    private static int _inboxY;
    private static int _inboxLineY;
    private static int _inboxLines;
    private static int _valueX;
    private static int _valueChars;
    private static int _inboxChars;

    // ---- Palette --------------------------------------------------------
    private static int _bg;
    private static int _chrome;
    private static int _rule;
    private static int _label;
    private static int _value;
    private static int _dim;
    private static int _ok;
    private static int _warn;
    private static int _bad;

    // ---- State ----------------------------------------------------------
    private static string _state = "booting";
    private static int _stateColor;
    private static bool _listening;
    private static bool _linked;
    private static int _tx;
    private static int _rx;
    private static int _errors;
    private static int _idle;
    private static int _seq;
    private static long _bootMs;
    private static string _note = "";
    private static string _chip = "?";
    private static string _board = "?";
    private static string _ssid = "";
    private static string _ip = "";
    private static List<string> _inbox = new List<string>();

    public static void Main()
    {
        Display.Init(320, 240);
        W = Display.Width();
        H = Display.Height();
        Layout();
        Palette();
        _bootMs = Uptime.Ms();
        _stateColor = _warn;

        Console.WriteLine("__NAME__ mqtt dashboard");
        Console.WriteLine(string.Concat("panel ", W.ToString(), "x", H.ToString()));
        Identify();
        ReadNetwork();
        Splash();

        JoinWifi();
        ReadNetwork();
        while (!Link())
        {
            Sleep.Ms(3000);
        }

        while (true)
        {
            // Re-read every cycle: DHCP can renew and a reconnect can land on
            // a different AP, so a value cached at boot goes stale.
            ReadNetwork();
            if (!_linked && !Link())
            {
                Sleep.Ms(3000);
                continue;
            }
            Publish();
            Listen();
        }
    }

    // ---- Session --------------------------------------------------------

    private static void Identify()
    {
        // DeviceInfo is served by the firmware host; tolerate a host that does
        // not implement it rather than dying on the first frame.
        try
        {
            _chip = DeviceInfo.Chip();
            _board = DeviceInfo.Board();
        }
        catch (Exception ex)
        {
            Note(ex.Message);
        }
    }

    /// <summary>
    /// Ask the interface what it is actually associated with, rather than
    /// echoing the constants above — on ESP32 the firmware performed the join,
    /// so these are the only truthful values available to the app.
    /// </summary>
    private static void ReadNetwork()
    {
        try
        {
            string reported = Wifi.GetSsid();
            _ssid = reported.Length > 0 ? reported : Ssid;
        }
        catch (Exception)
        {
            _ssid = Ssid;
        }
        try
        {
            string ip = Wifi.GetIp();
            _ip = ip.Length > 0 ? ip : "(no address)";
        }
        catch (Exception)
        {
            // Board exposes no WiFi interface: say so instead of showing blank.
            _ip = "(no wifi interface)";
        }
    }

    private static void JoinWifi()
    {
        SetState("wifi", _warn);
        Render();
        try
        {
            if (Wifi.Connect(Ssid, Psk))
            {
                Console.WriteLine(string.Concat("wifi: ", Ssid));
                return;
            }
            Note("wifi connect returned false");
        }
        catch (Exception ex)
        {
            Note(ex.Message);
        }
        // Not fatal: on ESP32 the join already happened at boot, and the
        // broker connect below is the real test of connectivity.
        Console.WriteLine("wifi: unconfirmed, trying the broker anyway");
    }

    private static bool Link()
    {
        SetState("connecting", _warn);
        Render();
        Console.WriteLine(string.Concat("connecting to ", Broker));
        try
        {
            if (!Mqtt.Connect(Broker, ClientId))
            {
                _errors++;
                Note("broker refused the connection");
                SetState("no broker", _bad);
                Render();
                return false;
            }
            Mqtt.Subscribe(TopicCommand);
            _linked = true;
            _note = "";
            SetState("online", _ok);
            Render();
            Console.WriteLine(string.Concat("subscribed to ", TopicCommand));
            Log(string.Concat("-- linked as ", ClientId));
            return true;
        }
        catch (Exception ex)
        {
            _errors++;
            Note(ex.Message);
            SetState("no broker", _bad);
            Render();
            return false;
        }
    }

    private static void Publish()
    {
        _seq++;
        string payload = string.Concat(
            "{\"seq\":", _seq.ToString(),
            ",\"uptime_ms\":", Elapsed().ToString(),
            ",\"rx\":", _rx.ToString(),
            ",\"chip\":\"", _chip, "\"}");
        try
        {
            Mqtt.Publish(TopicTelemetry, payload, 1);
            _tx++;
            SetState("online", _ok);
            Console.WriteLine(string.Concat("tx ", payload));
        }
        catch (Exception ex)
        {
            // A write to a dead socket is the reliable disconnect signal.
            _errors++;
            _linked = false;
            Note(ex.Message);
            SetState("dropped", _bad);
            Console.WriteLine(string.Concat("publish failed: ", ex.Message));
        }
    }

    private static void Listen()
    {
        _listening = true;
        Render();
        try
        {
            // Blocks until a message lands or the client's 10 s read timeout
            // expires; the "LISTENING" pill is lit for exactly that window.
            string message = Mqtt.Poll();
            _listening = false;
            _rx++;
            Handle(message);
        }
        catch (Exception)
        {
            // Idle timeout — nothing arrived. Not an error, and NOT treated as
            // a dropped session: Publish() is what detects that.
            _listening = false;
            _idle++;
        }
        Render();
    }

    /// <summary>Poll() returns "topic\npayload" — split on the first newline.</summary>
    private static void Handle(string message)
    {
        string topic = message;
        string payload = "";
        int nl = message.IndexOf("\n");
        if (nl >= 0)
        {
            topic = message.Substring(0, nl);
            payload = message.Substring(nl + 1);
        }
        Console.WriteLine(string.Concat("rx ", topic, " -> ", payload));

        string command = payload.Trim().ToLower();
        if (command == "clear")
        {
            _inbox.Clear();
            Log("-- inbox cleared by command");
            return;
        }
        if (command == "ping")
        {
            Log("<- ping, replying pong");
            try
            {
                Mqtt.Publish(TopicTelemetry, "{\"pong\":true}", 1);
                _tx++;
            }
            catch (Exception ex)
            {
                _errors++;
                _linked = false;
                Note(ex.Message);
            }
            return;
        }
        Log(string.Concat("< ", Leaf(topic), " ", payload));
    }

    // ---- Rendering ------------------------------------------------------

    private static void Render()
    {
        Display.Clear(_bg);
        Header();
        Banner();
        Rows();
        Inbox();
        Display.Present();
    }

    private static void Header()
    {
        Display.FillRect(0, 0, W, _headerH, _chrome);
        Display.DrawLine(0, _headerH, W - 1, _headerH, _rule);
        Display.DrawText(8, (_headerH - 16) / 2, "MQTT DASH", _value, 2);

        // Status lamp, plus a LISTENING pill while parked in Mqtt.Poll().
        int lampX = W - 14;
        int lampY = _headerH / 2;
        // Bezel as a larger filled disc, not DrawCircle: DrawCircle is managed
        // Bresenham (~40 interpreted SetPixel host calls), FillCircle is one
        // native scanline fill.
        Display.FillCircle(lampX, lampY, 7, _rule);
        Display.FillCircle(lampX, lampY, 5, _stateColor);
        if (_listening)
        {
            int pillW = 9 * Glyph + 8;
            int pillX = lampX - 12 - pillW;
            Display.FillRect(pillX, lampY - 8, pillW, 16, _bg);
            Display.DrawRect(pillX, lampY - 8, pillW, 16, _ok);
            Display.DrawText(pillX + 4, lampY - 4, "LISTENING", _ok, 1);
        }
    }

    private static void Banner()
    {
        Display.DrawText(8, _bannerY, Fit(_state.ToUpper(), (W - 16) / (Glyph * 2)), _stateColor, 2);
        if (_note.Length > 0)
        {
            Display.DrawText(8, _noteY, Fit(_note, (W - 16) / Glyph), _bad, 1);
        }
    }

    private static void Rows()
    {
        Row(0, "BOARD", string.Concat(_chip, " / ", _board), _value);
        Row(1, "WIFI", _ssid, _value);
        Row(2, "IP", _ip, _value);
        Row(3, "BROKER", Broker, _value);
        Row(4, "PUB", Leaf(TopicTelemetry), _value);
        Row(5, "SUB", Leaf(TopicCommand), _value);
        Row(6, "UP", string.Concat(Clock(Elapsed()), "  tx ", _tx.ToString(),
            "  rx ", _rx.ToString(), "  err ", _errors.ToString()), _dim);
    }

    private static void Row(int index, string label, string value, int color)
    {
        int y = _rowsY + index * _pitch;
        Display.DrawText(8, y, label, _label, 1);
        Display.DrawText(_valueX, y, Fit(value, _valueChars), color, 1);
    }

    private static void Inbox()
    {
        Display.DrawLine(0, _inboxY, W - 1, _inboxY, _rule);
        Display.DrawText(8, _inboxY + 5, "INBOX", _label, 1);
        Display.DrawText(8 + 6 * Glyph, _inboxY + 5,
            string.Concat("idle ", _idle.ToString()), _dim, 1);

        if (_inbox.Count == 0)
        {
            Display.DrawText(8, _inboxLineY, "(waiting for a message)", _dim, 1);
            return;
        }
        // Newest last, oldest trimmed off the top.
        int first = _inbox.Count - _inboxLines;
        if (first < 0) first = 0;
        for (int i = first; i < _inbox.Count; i++)
        {
            int y = _inboxLineY + (i - first) * _pitch;
            int color = i == _inbox.Count - 1 ? _ok : _value;
            Display.DrawText(8, y, Fit(_inbox[i], _inboxChars), color, 1);
        }
    }

    private static void Splash()
    {
        Display.Clear(_bg);
        Display.FillGradient(0, 0, W, H, Color.FromRgb(0, 12, 32), Color.FromRgb(0, 0, 8), true);
        Center(H / 2 - 30, "RustNet", Color.White, 4);
        Center(H / 2 + 6, "MQTT Dashboard", Color.Cyan, 2);
        Center(H / 2 + 34, Broker, _dim, 1);
        Display.Present();
        Sleep.Ms(1200);
    }

    // ---- Helpers --------------------------------------------------------

    private static void Layout()
    {
        _headerH = 30;
        _pitch = 13;
        _bannerY = _headerH + 6;          // scale-2 banner: 16 px tall
        _noteY = _bannerY + 18;           // scale-1 note:    8 px tall
        // Rows clear the note's line unconditionally — reserving the space only
        // when a note exists would make the layout jump, and reserving none at
        // all collides the note with the first row the moment an error appears.
        _rowsY = _noteY + 12;
        _inboxY = _rowsY + RowCount * _pitch + 5;
        _inboxLineY = _inboxY + 18;
        _inboxLines = (H - 6 - _inboxLineY) / _pitch;
        if (_inboxLines < 1) _inboxLines = 1;
        if (_inboxLines > 8) _inboxLines = 8;
        _valueX = 8 + 7 * Glyph;
        _valueChars = (W - _valueX - 8) / Glyph;
        _inboxChars = (W - 16) / Glyph;
    }

    private static void Palette()
    {
        _bg = Color.FromRgb(6, 8, 14);
        _chrome = Color.FromRgb(20, 24, 48);
        _rule = Color.FromRgb(70, 78, 110);
        _label = Color.FromRgb(120, 130, 160);
        _value = Color.White;
        _dim = Color.FromRgb(130, 140, 160);
        _ok = Color.FromRgb(60, 230, 130);
        _warn = Color.FromRgb(240, 200, 60);
        _bad = Color.FromRgb(240, 90, 80);
    }

    private static void SetState(string state, int color)
    {
        _state = state;
        _stateColor = color;
    }

    private static void Note(string text)
    {
        _note = text;
    }

    private static void Log(string line)
    {
        _inbox.Add(line);
        // Keep a little history beyond what fits, but never grow without bound.
        while (_inbox.Count > _inboxLines + 4)
        {
            _inbox.RemoveAt(0);
        }
    }

    private static long Elapsed()
    {
        return Uptime.Ms() - _bootMs;
    }

    /// <summary>Last segment of a topic, so "a/b/telemetry" shows as "telemetry".</summary>
    private static string Leaf(string topic)
    {
        int cut = -1;
        for (int i = 0; i < topic.Length; i++)
        {
            if (topic[i] == '/') cut = i;
        }
        return cut >= 0 ? topic.Substring(cut + 1) : topic;
    }

    private static string Fit(string text, int max)
    {
        if (max < 1) return "";
        if (text.Length <= max) return text;
        if (max <= 3) return text.Substring(0, max);
        return string.Concat(text.Substring(0, max - 3), "...");
    }

    private static string Clock(long ms)
    {
        int total = (int)(ms / 1000);
        int h = total / 3600;
        int m = (total % 3600) / 60;
        int s = total % 60;
        return string.Concat(h.ToString(), ":", Two(m), ":", Two(s));
    }

    private static string Two(int v)
    {
        return v < 10 ? string.Concat("0", v.ToString()) : v.ToString();
    }

    private static void Center(int y, string text, int color, int scale)
    {
        int x = (W - text.Length * Glyph * scale) / 2;
        if (x < 0) x = 0;
        Display.DrawText(x, y, text, color, scale);
    }
}
