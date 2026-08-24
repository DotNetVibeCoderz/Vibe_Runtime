namespace RustNet.Designer.Assistant;

/// <summary>
/// Ground truth the assistant is handed on request, rather than left to recall.
/// These strings track the real contracts: the XML attributes
/// <c>RustNet.UI.Ui.LoadXml</c> parses, the intrinsics
/// <c>RustNet.Graphics.Display</c> exposes, and the language subset the RNX
/// interpreter accepts. When those change, change these.
/// </summary>
public static class RustNetReference
{
    public const string UiMarkup = """
        # RustNet.UI XML layout format

        One root `<window>`; every node is an element `kind` with attributes. Unknown
        attributes are ignored silently, so only use the ones listed here.

        ## Kinds

        | kind | renders | kind-specific attributes |
        |---|---|---|
        | `window` | root container | `width` `height` `bg` `pad` `gap` |
        | `stack` | vertical (default) or horizontal run | `orient="horizontal"` `pad` `gap` |
        | `panel` | filled container | `bg` `pad` `gap` |
        | `border` | filled + outlined container | `bg` `border` `pad` `gap` |
        | `canvas` | absolute placement; children use `x` `y` | `pad` |
        | `grid` | N equal columns | `columns` `gap` `pad` |
        | `scrollviewer` | fixed-height clipped viewport | `height` `scroll` |
        | `label` / `textblock` | one line of text | `text` `fg` `scale` |
        | `button` | box + text | `text` `fg` `bg` `border` |
        | `textbox` | input box + text | `text` `fg` `border` |
        | `checkbox` | box + tick + label | `text` `checked` |
        | `radio` | circle + dot + label | `text` `checked` `group` |
        | `slider` | track + knob | `min` `max` `value` `fg` |
        | `progress` | filled bar | `min` `max` `value` `fg` |
        | `listbox` | rows, selected row highlighted | `items="A;B;C"` `selected` |
        | `image` | image placeholder box | `width` `height` `bg` |
        | `rect` | filled rectangle | `width` `height` `bg` |

        ## Attributes on every element

        `id` (unique, how app code finds it), `text`, `x`, `y`, `width`, `height`,
        `scale` (integer text multiplier, 1 or 2 in practice), `fg`, `bg`, `border`.

        Sizes are pixels; `0` means auto. `x`/`y` only matter inside a `canvas` —
        stack/grid/panel children are placed by the container, so setting `x` there
        does nothing and cannot be dragged in the designer either.

        ## Colors

        RGB565, four hex digits, no `#`: `0000` black, `FFFF` white, `F800` red,
        `07E0` green, `001F` blue, `FFE0` yellow, `07FF` cyan, `F81F` magenta,
        `8410` gray, `4208` dark gray, `C618` light gray, `05BF` accent blue.
        Any other colour: take 8-bit r,g,b and pack `((r&0xF8)<<8)|((g&0xFC)<<3)|(b>>3)`,
        or call the `rgb565` function.

        ## Text metrics

        The device font is 8x8 and advances `8 * scale` pixels per character. A
        320 px wide panel fits 40 characters at `scale="1"` and 20 at `scale="2"`.
        Budget label widths from that — text is not wrapped or ellipsised.

        The font covers **ASCII only**. A degree sign, an arrow or an accented
        letter will not draw — write `C`, `->` and plain letters instead.

        ## Example

        ```xml
        <window width="320" height="240" bg="0000" pad="8" gap="6">
          <label id="title" text="BOILER" scale="2" fg="07FF"/>
          <grid columns="2" gap="6">
            <label text="Flow" fg="8410"/>
            <label id="flow" text="-- l/m" fg="FFFF"/>
            <label text="Return" fg="8410"/>
            <label id="return" text="-- C" fg="FFFF"/>
          </grid>
          <progress id="load" min="0" max="100" value="0" fg="07E0" height="10"/>
          <button id="reset" text="RESET" bg="4208" fg="FFFF" width="80"/>
        </window>
        ```

        ## Driving it from app code

        ```csharp
        UiElement screen = Ui.LoadXml(FileSystem.ReadAllText("/data/ui.xml"));
        UiElement load = screen.FindById("load");
        load.Value = 62;
        Ui.Render(screen);                        // clear + layout + draw + present
        UiElement hit = Ui.Tap(screen, tx, ty);   // toggles/selects, returns the element
        Ui.Scroll(screen, "log", 8);              // move a scrollviewer
        ```
        """;

    public const string Graphics = """
        # RustNet.Graphics.Display

        Immediate-mode drawing into the runtime's framebuffer. Everything below is
        a native intrinsic (one host call) unless marked managed.

        ```csharp
        Display.Init(width, height);
        Display.Configure(PanelDriver.Ili9341, 320, 240, rotation: 0);
        int w = Display.Width();  int h = Display.Height();   // rotation-aware
        Display.SetClip(x, y, w, h);  Display.ClearClip();
        Display.Clear(color);
        Display.SetPixel(x, y, color);
        Display.FillRect(x, y, w, h, color);
        Display.FillCircle(cx, cy, r, color);
        Display.DrawLine(x0, y0, x1, y1, color);
        Display.DrawText(x, y, text, color, scale);
        Display.DrawImage(x, y, w, h, byte[] rgb565);          // little-endian pairs
        Display.BlendImage(x, y, w, h, byte[] rgb565, alpha);  // alpha 0..255
        Display.FillGradient(x, y, w, h, c0, c1, vertical);
        Display.Present();                                     // push the frame
        ```

        Managed helpers — ordinary C# compiled into the app, so they cost one host
        call per primitive they emit:

        - `Display.DrawRect` = 4 × `DrawLine`.
        - `Display.DrawCircle` = Bresenham, ~8 `SetPixel` calls per octant step.
          For anything animated prefer two `FillCircle` calls (a larger disc in the
          border colour, a smaller one in the fill) over `DrawCircle`.

        Colours are RGB565 ints. `Color.FromRgb(r, g, b)` packs 8-bit channels;
        `Color.Red`, `.Green`, `.Blue`, `.White`, `.Black`, `.Yellow`, `.Cyan`,
        `.Magenta` are the named constants.

        The font is 8x8 and `DrawText` advances `8 * scale` per character.

        ## Frame loop shape

        ```csharp
        Display.Init(320, 240);
        while (true)
        {
            Display.Clear(Color.Black);
            Display.FillGradient(0, 0, 320, 40, 0x0008, 0x0010, true);
            Display.DrawText(8, 12, "TELEMETRY", Color.Cyan, 2);
            Display.Present();
            RustNet.Threading.Sleep.Ms(200);
        }
        ```

        Draw into the frame, then `Present()` once. Presenting per primitive is the
        usual cause of a flickering panel.
        """;

    public const string LanguageLimits = """
        # What the RNX interpreter accepts

        The entry point must be `static void Main()` with no arguments, in a normal
        class. Debug configuration only for anything async.

        Supported: the language core (classes, structs, interfaces, inheritance,
        virtual dispatch and overrides, generics by erasure, delegates and lambdas,
        `try`/`catch`/`finally`, `catch when` filters), `async`/`await`, BCL
        collections and `foreach`, a LINQ subset, `StringBuilder`, `Regex`, string
        interpolation, `string.Concat` with any number of arguments, JSON/XML/binary
        serializers, streams.

        Watch out for:

        - **Catch clauses are untyped.** `catch (IOException ex)` catches
          everything. Discriminate with a `when` filter on `ex.Message`.
        - **Reflection is partial.** `GetType()`, `typeof(T)`, `Type.Name`/
          `FullName`/`Namespace`/`BaseType`, method/field/property enumeration and
          `MethodInfo.Invoke` work. Attributes work on types only, not on methods
          or fields. `ldtoken` of a method or field does not — which means array
          initialiser syntax for large arrays can fail; fill arrays with a loop.
        - **`ref` is same-frame only.** Never pass a `ref` local into another
          managed method.
        - No Release-config async state machines.

        Use the RustNet equivalent, not the BCL one:

        | Instead of | Use |
        |---|---|
        | `Thread.Sleep(ms)` | `RustNet.Threading.Sleep.Ms(ms)` |
        | `System.IO.File` | `RustNet.IO.FileSystem` |
        | `HttpClient` | `RustNet.Net.Http` |
        | `DateTime.Now` on the device | `RustNet.Sys.Rtc` |
        | `System.Drawing` | `RustNet.Graphics.Display` |

        Devices are reached through the `RustNet.*` libraries: `RustNet.Hal`
        (Gpio, Adc, Pwm, I2c, Spi, Uart), `RustNet.Graphics`, `RustNet.UI`,
        `RustNet.Net` (Wifi, Ethernet, Http, Mqtt, WebServer), `RustNet.IO`
        (FileSystem), `RustNet.Data`, `RustNet.Devices`, `RustNet.Buses`
        (Can, Modbus, OneWire), `RustNet.Sys` (Power, Rtc, Watchdog),
        `RustNet.Threading` (`Sleep.Ms`), `RustNet.Serialization`.

        Two device facts that trip up generated code:

        - On ESP32 the firmware joins WiFi **at boot** from credentials stored with
          `rustnet wifi --ssid <s> --psk <p>`. Managed `Wifi.Connect` is
          bookkeeping there. Use `Wifi.GetSsid()` / `Wifi.GetIp()` to show the
          network the device is really on.
        - `Mqtt.Poll()` blocks until a message arrives or its 10 second read
          timeout expires. Repaint before polling, not after.
        """;
}
