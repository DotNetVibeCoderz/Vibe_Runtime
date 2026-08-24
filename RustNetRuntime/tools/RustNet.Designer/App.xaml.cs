using System;
using System.Windows;
using System.Windows.Controls;
using RustNet.UI;

namespace RustNet.Designer;

public partial class App : Application
{
    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);

        // Headless verification: load an XML, render it to an in-memory
        // canvas, round-trip it through ToXml/LoadXml, and report — so the
        // designer's core is testable in CI without a display.
        if (e.Args.Length >= 1 && e.Args[0] == "--selftest")
        {
            int code = SelfTest(e.Args.Length >= 2 ? e.Args[1] : null);
            Shutdown(code);
            return;
        }

        // Headless: one assistant turn on stdout, so the model path can be
        // exercised (and scripted) without the window.
        if (e.Args.Length >= 1 && Array.IndexOf(e.Args, "--ask") >= 0)
        {
            int code = Assistant.HeadlessAsk.Run(e.Args, Console.Out);
            Shutdown(code);
            return;
        }

        // Headless: round-trip the sample (or an input file) through the
        // designer's load/save and write the RustNet.UI XML out.
        if (e.Args.Length >= 2 && e.Args[0] == "--export")
        {
            string src = e.Args.Length >= 3 && System.IO.File.Exists(e.Args[2])
                ? System.IO.File.ReadAllText(e.Args[2])
                : SampleXml;
            UiElement root = Ui.LoadXml(src);
            System.IO.File.WriteAllText(e.Args[1], Ui.ToXml(root));
            Console.WriteLine("exported " + e.Args[1]);
            Shutdown(0);
            return;
        }

        var win = new MainWindow();
        if (e.Args.Length >= 1 && System.IO.File.Exists(e.Args[0]))
        {
            win.OpenFile(e.Args[0]);
        }
        win.Show();
    }

    private static int SelfTest(string? path)
    {
        try
        {
            string xml = path != null && System.IO.File.Exists(path)
                ? System.IO.File.ReadAllText(path)
                : SampleXml;

            UiElement root = Ui.LoadXml(xml);

            // Render to an off-screen canvas (exercises the full renderer).
            var canvas = new Canvas();
            var map = DesignRenderer.Render(canvas, root);
            if (canvas.Children.Count == 0)
            {
                Console.Error.WriteLine("selftest FAIL: nothing rendered");
                return 1;
            }

            // Round-trip: ToXml → LoadXml must preserve structure.
            string saved = Ui.ToXml(root);
            UiElement again = Ui.LoadXml(saved);
            if (again.FindById("title") == null && root.FindById("title") != null)
            {
                Console.Error.WriteLine("selftest FAIL: round-trip lost 'title'");
                return 1;
            }

            // Drag-to-move: a canvas child moves by the drag delta (clamped
            // to >= 0); a layout-managed child does not.
            UiElement cv = UiElement.Make("canvas");
            UiElement dot = UiElement.Make("rect");
            dot.X = 5;
            dot.Y = 5;
            cv.Add(dot);
            if (!DragTool.MoveBy(cv, dot, 10, 3) || dot.X != 15 || dot.Y != 8)
            {
                Console.Error.WriteLine("selftest FAIL: canvas child did not drag");
                return 1;
            }
            DragTool.MoveBy(cv, dot, -100, -100); // clamps at 0
            if (dot.X != 0 || dot.Y != 0)
            {
                Console.Error.WriteLine("selftest FAIL: drag not clamped");
                return 1;
            }
            UiElement stack = UiElement.Make("stack");
            UiElement fixedChild = UiElement.Make("label");
            stack.Add(fixedChild);
            if (DragTool.MoveBy(stack, fixedChild, 10, 10))
            {
                Console.Error.WriteLine("selftest FAIL: stack child should not drag");
                return 1;
            }

            Console.WriteLine($"selftest OK: {canvas.Children.Count} visuals, "
                + $"{map.Count} selectable, round-trip preserved, drag-to-move works");

            // The assistant's own headless checks: settings, sessions, uploads,
            // rendering, and that a kernel builds for every provider.
            return Assistant.AssistantSelfTest.Run(Console.Out) ? 0 : 1;
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine("selftest FAIL: " + ex);
            return 1;
        }
    }

    public const string SampleXml =
        "<window width=\"160\" height=\"128\" bg=\"0000\" pad=\"4\" gap=\"4\">\n" +
        "  <label id=\"title\" text=\"Thermostat\" scale=\"2\" fg=\"07FF\"/>\n" +
        "  <slider id=\"setpoint\" min=\"10\" max=\"30\" value=\"21\" fg=\"F800\"/>\n" +
        "  <checkbox id=\"eco\" text=\"Eco mode\" checked=\"true\"/>\n" +
        "  <listbox id=\"zones\" items=\"Kitchen;Garage;Attic\" selected=\"0\"/>\n" +
        "  <button id=\"apply\" text=\"Apply\" bg=\"4208\"/>\n" +
        "</window>\n";
}
