using System.Collections.Generic;
using System.Linq;

namespace RustNet.Designer.Assistant;

/// <summary>One ready-to-run prompt shown in the panel's prompt gallery.</summary>
public sealed record PromptTemplate(string Category, string Title, string Text);

/// <summary>
/// The persona used when app.config carries none, and a gallery of prompts that
/// produce a finished screen or a finished file rather than a description of
/// one. Each prompt names the panel size, says what data is on screen, and ends
/// by telling the assistant to apply the result — because a prompt that only
/// asks for "a nice dashboard" gets a paragraph back instead of a layout.
/// </summary>
public static class PromptLibrary
{
    public const string DefaultPersona =
        "You are Jack The Code Bender, the code assistant inside the RustNet UI Designer. "
        + "RustNet runs C#/.NET applications on microcontrollers through a Rust IL interpreter. "
        + "You design embedded screens and write the app code behind them.\n\n"
        + "Call get_ui_reference before writing layout XML, get_graphics_reference before writing drawing "
        + "code, and get_language_limits before using anything beyond the C# core. Validate layouts with "
        + "validate_layout_xml, then put them on the canvas with apply_layout_xml. Put app code in the code "
        + "pane with set_generated_code. Prefer doing over describing: produce the artefact, then explain "
        + "the choices in two or three sentences. Use markdown — tables for pin maps and properties, fenced "
        + "blocks with a language tag for code.";

    public static IReadOnlyList<PromptTemplate> All { get; } = new List<PromptTemplate>
    {
        // ---- Dashboards -------------------------------------------------
        new("Dashboards",
            "Boiler telemetry dashboard",
            "Design a 320x240 boiler dashboard: a title band, flow temperature, return temperature and "
            + "pressure as large readouts with small grey captions, a burner-load progress bar, and a "
            + "status line at the bottom. Give every live value an id I can set from code. Use a dark "
            + "background with one accent colour and keep the readouts on an 8 px grid. Validate it, apply "
            + "it to the canvas, then list the ids and what each expects."),

        new("Dashboards",
            "Solar inverter overview",
            "Design a 320x240 solar inverter screen: PV power now, today's yield, grid import/export, and "
            + "battery state of charge. Battery as a progress bar that reads as a battery, the rest as "
            + "labelled numbers in a two-column grid. Apply it and tell me which ids to update each second."),

        new("Dashboards",
            "Cold-chain monitor",
            "Design a 320x240 cold-chain monitor for four refrigerated compartments: each row shows the "
            + "compartment name, its temperature, and a bar for how far it is from the setpoint. Rows must "
            + "read as in-range or out-of-range at a glance without using colour alone. Apply it."),

        new("Dashboards",
            "Air quality station",
            "Design a 320x240 air quality display: PM2.5, PM10, CO2, temperature and humidity. Lead with "
            + "PM2.5 as the largest element, the rest as a compact grid underneath, and a one-line verdict "
            + "at the bottom. Apply it and give me the colour thresholds you chose as a table."),

        new("Dashboards",
            "Water tank telemetry",
            "Design a 320x240 water system screen: tank level as a tall vertical bar on the left, pump "
            + "state, inlet pressure and litres-per-minute on the right, and a 24-hour usage figure. Apply "
            + "it, then write the C# that refreshes it from Adc.ReadMillivolts every 500 ms."),

        // ---- Controls ---------------------------------------------------
        new("Controls",
            "Thermostat control panel",
            "Design a 320x240 thermostat: current temperature large, a setpoint slider from 10 to 30, an "
            + "Eco mode checkbox, a mode radio group of Heat/Cool/Auto, and an Apply button. Everything must "
            + "be tappable with a fingertip, so nothing smaller than 24 px tall. Apply it, then write the "
            + "Ui.Tap handling loop that reacts to each control."),

        new("Controls",
            "Lighting zone controller",
            "Design a 320x240 lighting controller: six zones as a two-column grid of buttons that show "
            + "on/off state, a master brightness slider, and a scene listbox with Morning, Day, Evening, "
            + "Night. Apply it and write the tap handler that toggles a zone and reports which one."),

        new("Controls",
            "Motor jog panel",
            "Design a 320x240 motor jog panel: direction radio group, a speed slider with the RPM shown "
            + "numerically beside it, jog and stop buttons with stop visually dominant, and a fault line. "
            + "Apply it and explain how you made stop the obvious target."),

        new("Controls",
            "Irrigation scheduler",
            "Design a 320x240 irrigation screen: four valves as rows with name, next run time and a "
            + "run-now button, plus a scrollviewer because there will eventually be twelve. Apply it and "
            + "show me how Ui.Scroll moves it."),

        new("Controls",
            "Access keypad",
            "Design a 320x240 keypad: a masked entry field, digits 0-9 plus clear and enter in a 3-column "
            + "grid, and a status line. Buttons must be at least 60x40. Apply it, then write the C# that "
            + "collects digits and clears the field after four."),

        // ---- Instruments -----------------------------------------------
        new("Instruments",
            "Analogue gauge with RustNet.Graphics",
            "Write RustNet.Graphics code for a 320x240 analogue gauge: an arc scale from 0 to 100 with "
            + "ticks every 10, a needle drawn from the centre, the numeric value under the hub, and a "
            + "coloured arc segment for the alarm band. Redraw only what changes each frame and explain "
            + "which calls are native intrinsics. Put it in the code pane."),

        new("Instruments",
            "Scrolling waveform",
            "Write RustNet.Graphics code that plots a scrolling waveform across a 320x240 panel: a fixed "
            + "axis and grid, a trace that scrolls right to left, and min/max/now readouts in the corner. "
            + "Keep it above 10 fps and say what you did to get there. Put it in the code pane."),

        new("Instruments",
            "Bar-graph VU meter",
            "Write RustNet.Graphics code for a stereo bar-graph level meter on a 320x240 panel: segmented "
            + "bars, a peak-hold marker that decays, and dB labels. Use FillRect rather than per-pixel "
            + "drawing and explain the frame budget. Put it in the code pane."),

        new("Instruments",
            "Compass rose",
            "Write RustNet.Graphics code that draws a compass rose on a 320x240 panel with cardinal "
            + "labels, a heading needle and the heading in degrees, updated from a magnetometer reading. "
            + "Use FillCircle rather than DrawCircle and say why. Put it in the code pane."),

        new("Instruments",
            "Sparkline strip",
            "Write RustNet.Graphics code for a strip of four sparklines stacked on a 320x240 panel, each "
            + "with a label and its latest value, fed from a ring buffer of 80 samples. Put it in the code "
            + "pane and note the memory it uses."),

        // ---- Status & connectivity -------------------------------------
        new("Status",
            "MQTT session panel",
            "Design a 320x240 MQTT status screen: a connection lamp, broker, client id, the real SSID and "
            + "IP, published and received counters, and a scrolling inbox of the last messages. Read the "
            + "mqtt-dashboard template first and stay consistent with it. Apply the layout, then write the "
            + "loop that keeps it current — remember Mqtt.Poll blocks for up to ten seconds."),

        new("Status",
            "Network diagnostics screen",
            "Design a 320x240 network screen: interface kind, link state, IP, gateway, MAC, RSSI as a bar, "
            + "and a last-error line. Read docs/networking.md first so the fields match what the HAL "
            + "actually reports. Apply it."),

        new("Status",
            "Device health page",
            "Design a 320x240 device health page: uptime, free memory, CPU temperature, watchdog state, "
            + "reset reason and firmware version. Read docs/system.md so the values are ones RustNet.Sys "
            + "can really provide. Apply it and write the refresh code."),

        new("Status",
            "Boot splash and progress",
            "Design a 320x240 boot sequence: a splash with the product name, a progress bar and a step "
            + "caption that changes through five named stages. Apply the layout and write the C# that "
            + "advances it, keeping each stage on screen long enough to read."),

        new("Status",
            "Fault banner and recovery",
            "Design a 320x240 fault screen: a banner that names the fault, a plain-language explanation, "
            + "the recovery action as a button, and a fault code in small mono text. No apologising in the "
            + "copy, and the person must be able to tell what to do next. Apply it."),

        // ---- Data & logs -----------------------------------------------
        new("Data",
            "Event log viewer",
            "Design a 320x240 event log: a header row, a scrollviewer of timestamped lines, and a footer "
            + "showing how many entries there are. Work out how many characters fit per line at scale 1 "
            + "before you choose the column widths. Apply it."),

        new("Data",
            "Recipe / setpoint table",
            "Design a 320x240 screen showing a table of six process steps with duration and target "
            + "temperature, the active step highlighted, and previous/next buttons. Apply it and write the "
            + "code that advances the step and re-renders."),

        new("Data",
            "Energy meter with tariff",
            "Design a 320x240 energy meter: kWh today, current power, tariff band, and cost so far. Use "
            + "the calculate function for the cost arithmetic and show your working. Apply the layout."),

        new("Data",
            "Inventory picking screen",
            "Design a 320x240 warehouse picking screen: order number, item, bin location large enough to "
            + "read at arm's length, quantity, and confirm/skip buttons. Apply it and explain the size "
            + "choices in terms of the 8x8 font."),

        // ---- Backend ---------------------------------------------------
        new("Backend",
            "Sensor loop with backoff",
            "Write the backend for a RustNet app that reads a sensor over I2C every second, publishes it "
            + "to MQTT, and retries with backoff when the publish fails. Detect the dropped session on "
            + "publish, not on poll. Check the language limits first — remember catch clauses are untyped. "
            + "Put it in the code pane."),

        new("Backend",
            "HTTP JSON poller",
            "Write a RustNet app that fetches a JSON endpoint every 30 seconds, parses two fields out of "
            + "it, and shows them on the panel. Confirm the Http and serializer APIs with find_managed_api "
            + "before you use them. Put it in the code pane."),

        new("Backend",
            "Config file on the device VFS",
            "Write the code that loads a config file from the device filesystem at startup, falls back to "
            + "defaults when it is missing, and rewrites it when a setting changes. Check the FileSystem "
            + "API first. Put it in the code pane."),

        new("Backend",
            "Modbus register poller",
            "Write a RustNet app that polls four Modbus holding registers, scales them to engineering "
            + "units, and updates a UI layout by id. Remember byte arrays are the only array channel across "
            + "the host boundary. Put it in the code pane."),

        new("Backend",
            "Local web server for the device",
            "Write a RustNet app that serves a small status page and a JSON endpoint from the device "
            + "itself, using the WebServer API. Read docs/networking.md first. Put it in the code pane."),

        new("Backend",
            "Debounced button state machine",
            "Write a debounced button handler that distinguishes a tap, a double tap and a long press from "
            + "one GPIO input, with the timings as named constants. Put it in the code pane."),

        // ---- Composition ------------------------------------------------
        new("Composition",
            "Design system for this project",
            "Propose a small design system for a 320x240 RustNet panel: a five-colour RGB565 palette with "
            + "hex values and what each colour means, two text scales and what each is for, a spacing "
            + "step, and rules for where numbers sit relative to their captions. Give it to me as tables, "
            + "then apply a sample screen that demonstrates every rule."),

        new("Composition",
            "Adapt this layout to 160x128",
            "Take the layout currently on my canvas and adapt it to a 160x128 panel: fewer elements, "
            + "shorter labels, scale 1 text, same information priority. Tell me what you dropped and why, "
            + "then apply it."),

        new("Composition",
            "Critique what is on my canvas",
            "Look at the layout on my canvas and critique it as a designer would: information hierarchy, "
            + "whether the text fits, spacing consistency, colour use, and tap-target sizes. Then apply a "
            + "revised version and list the changes."),

        new("Composition",
            "Two-page navigation",
            "Design two 320x240 screens that belong together — an overview and a detail page — sharing a "
            + "header treatment and a back affordance. Apply the overview, put the detail page XML in the "
            + "code pane, and write the C# that swaps between them."),

        new("Composition",
            "Rebuild this from a screenshot",
            "I have attached a screenshot of a UI I like. Rebuild it as a RustNet.UI layout for a 320x240 "
            + "panel: keep the structure and hierarchy, translate the colours to RGB565, and replace "
            + "anything the toolkit cannot draw with the nearest thing it can. Apply it and list your "
            + "substitutions."),

        new("Composition",
            "Turn this sketch into a layout",
            "I have attached a hand sketch of a screen. Read it, ask me nothing, and produce the "
            + "RustNet.UI layout that matches it for a 320x240 panel. Where the sketch is ambiguous, choose "
            + "and say what you chose. Apply it."),

        new("Composition",
            "Dark and light variants",
            "Take the layout on my canvas and produce a light-background variant with the same structure, "
            + "keeping contrast usable on a cheap TFT. Give me both palettes as a table, apply the light "
            + "one, and put the dark one in the code pane."),

        // ---- Research ---------------------------------------------------
        new("Research",
            "Wire up a specific sensor",
            "I want to use a BME280 on I2C with RustNet. Search for its address and register map, confirm "
            + "the RustNet I2c API with find_managed_api, then write the driver and a 320x240 screen that "
            + "shows temperature, humidity and pressure. Apply the layout and put the driver in the code "
            + "pane."),

        new("Research",
            "Explain a protocol before using it",
            "Explain how the RNDP framing works, reading docs/protocol.md rather than recalling it, and "
            + "give me a table of the commands with what each returns."),

        new("Research",
            "Pick a panel for a project",
            "I need a 2.4 to 2.8 inch SPI TFT for an ESP32 project driven by RustNet. Search for current "
            + "options, compare controller, resolution and price in a table, and tell me which PanelDriver "
            + "value each one needs."),

        new("Research",
            "Port an Arduino sketch",
            "I will paste an Arduino sketch. Port it to a RustNet C# app: map each Arduino call to its "
            + "RustNet.Hal equivalent, confirm each one with find_managed_api, and flag anything that has "
            + "no equivalent instead of inventing it. Put the result in the code pane."),
    };

    public static IEnumerable<string> Categories => All.Select(p => p.Category).Distinct();

    public static IEnumerable<PromptTemplate> InCategory(string category)
        => All.Where(p => p.Category == category);
}
