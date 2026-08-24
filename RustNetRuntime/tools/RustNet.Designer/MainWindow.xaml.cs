using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Threading;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;
using Microsoft.Win32;
// System.Windows.Shapes.Path would shadow System.IO.Path, which this file uses
// far more often; only the one shape is needed.
using Rectangle = System.Windows.Shapes.Rectangle;
using RustNet.Designer.Assistant;
using RustNet.Designer.Deployment;
using RustNet.UI;

namespace RustNet.Designer;

public partial class MainWindow : Window, IDesignerBridge
{
    private UiElement _root = Ui.LoadXml(App.SampleXml);
    private UiElement? _selected;
    private Dictionary<FrameworkElement, UiElement> _map = new();

    // Two documents live in this window: the layout on the canvas, and the code
    // in the code pane. Every file command acts on whichever tab is in front.
    private string? _layoutFile;
    private bool _layoutDirty;
    private string? _codeFile;

    // Footer widgets built in code (see BuildFooters).
    private TextBlock _xmlHint = null!;
    private TextBlock _codeFileName = null!;

    private readonly Dictionary<ColumnDefinition, GridLength> _panelWidths = new();
    private readonly List<DeviceTarget> _targets = new();
    private DeviceTarget? _target;
    private CancellationTokenSource? _run;
    private string _workspaceRoot = "";
    private string _signingKey = "";

    // The palette shown in the toolbox, in alphabetical order.
    //
    // Sorted again in BuildToolbox rather than trusted to stay sorted here:
    // a list that has to be hand-maintained in order drifts the first time
    // someone appends a control to the end of it.
    private static readonly string[] ControlKinds =
    {
        "border", "button", "calendar", "canvas", "chart", "checkbox",
        "combobox", "datagrid", "dockpanel", "ellipse", "expander", "gauge",
        "grid", "groupbox", "image", "label", "line", "listbox",
        "messagebox", "panel", "polygon", "progress", "radio", "rect",
        "scrollviewer", "slider", "stack", "tabcontrol", "tabitem", "textbox",
        "textflow", "treeview",
    };

    public MainWindow()
    {
        InitializeComponent();
        BuildToolbox();
        BuildFooters();
        _selected = _root;

        CentreTabs.SelectionChanged += OnCentreTabChanged;
        XmlPane.Status += (_, message) => Status(message);
        CodePane.Status += (_, message) => Status(message);
        XmlPane.TextEdited += (_, _) => UpdateTabHeaders();
        CodePane.TextEdited += (_, _) => UpdateTabHeaders();

        Chat.Status += (_, message) => Status(message);
        Chat.HideRequested += (_, _) => AssistantToggle.IsChecked = false;
        Chat.Initialize(this);

        ResolvePaths();
        SeedTargets();
        RenderAll();
        UpdateTabHeaders();
        UpdateRunButton();
        SourceInitialized += (_, _) => DarkTitleBar.Apply(this);

        BindGestures();
    }

    private void BindGestures()
    {
        void Bind(Action action, Key key, ModifierKeys modifiers = ModifierKeys.None)
            => InputBindings.Add(new KeyBinding(new RelayCommand(action), key, modifiers));

        Bind(() => OnNew(this, null!), Key.N, ModifierKeys.Control);
        Bind(() => OnOpen(this, null!), Key.O, ModifierKeys.Control);
        Bind(() => OnSave(this, null!), Key.S, ModifierKeys.Control);
        Bind(() => OnClose(this, null!), Key.W, ModifierKeys.Control);
        Bind(() => OnRun(this, null!), Key.F5);
        Bind(() => Toggle(ToolboxToggle), Key.D1, ModifierKeys.Control);
        Bind(() => Toggle(InspectorToggle), Key.D2, ModifierKeys.Control);
        Bind(() => Toggle(OutputToggle), Key.D3, ModifierKeys.Control);
        Bind(() => Toggle(AssistantToggle), Key.J, ModifierKeys.Control);

        KeyDown += (_, e) =>
        {
            // Del deletes the selected element — but not while the person is
            // typing in a property box, the composer or an editor.
            if (e.Key == Key.Delete
                && Keyboard.FocusedElement is not System.Windows.Controls.Primitives.TextBoxBase
                && Keyboard.FocusedElement is not ICSharpCode.AvalonEdit.Editing.TextArea)
            {
                OnDelete(this, null!);
            }
        };
    }

    /// <summary>
    /// Where the checkout and the signing key are. Both come from the
    /// assistant's settings so there is one place to configure them.
    /// </summary>
    private void ResolvePaths()
    {
        AssistantOptions options = AssistantOptions.Load();
        _workspaceRoot = options.WorkspaceRoot;
        _signingKey = Path.Combine(_workspaceRoot, "keys", "rustnet-signing.key");
    }

    // ---- toolbox -----------------------------------------------------

    private void BuildToolbox()
    {
        foreach (string kind in ControlKinds.OrderBy(k => k, StringComparer.Ordinal))
        {
            var btn = new Button
            {
                Content = "+ " + kind,
                Style = (Style)FindResource("ToolButton"),
                Tag = kind,
                ToolTip = $"Add a <{kind}> to the selected container",
            };
            btn.Click += OnAddControl;
            Toolbox.Children.Add(btn);
        }
    }

    /// <summary>
    /// The tab-specific actions along the bottom of each editor pane. Built here
    /// rather than in markup because a UserControl owns its own name scope.
    /// </summary>
    private void BuildFooters()
    {
        _xmlHint = new TextBlock
        {
            Style = (Style)FindResource("Readout"),
            Margin = new Thickness(12, 0, 0, 0),
            VerticalAlignment = VerticalAlignment.Center,
        };
        var xmlFooter = new StackPanel { Orientation = Orientation.Horizontal };
        xmlFooter.Children.Add(Action("Apply to canvas", "PrimaryButton", OnApplyXml,
            "Parse this XML and replace the design"));
        xmlFooter.Children.Add(Action("Reload from canvas", "FlatButton", OnReloadXml,
            "Throw away the edits here and re-read the canvas"));
        xmlFooter.Children.Add(_xmlHint);
        XmlPane.FooterContent = xmlFooter;

        _codeFileName = new TextBlock
        {
            Style = (Style)FindResource("Readout"),
            Margin = new Thickness(8, 0, 0, 0),
            VerticalAlignment = VerticalAlignment.Center,
            Text = "(nothing generated yet)",
        };
        var codeFooter = new StackPanel { Orientation = Orientation.Horizontal };
        codeFooter.Children.Add(Action("Send to assistant", "FlatButton", OnSendCodeToAssistant,
            "Ask Jack to review or extend what is in this pane"));
        codeFooter.Children.Add(_codeFileName);
        CodePane.FooterContent = codeFooter;

        Button Action(string text, string style, RoutedEventHandler click, string tip)
        {
            var button = new Button
            {
                Content = text,
                Style = (Style)FindResource(style),
                Margin = new Thickness(0, 0, 4, 0),
                ToolTip = tip,
            };
            button.Click += click;
            return button;
        }
    }

    private void OnAddControl(object sender, RoutedEventArgs e)
    {
        string kind = (string)((Button)sender).Tag;
        UiElement child = MakeDefault(kind);

        // Add into the selected container, else its parent, else the root.
        UiElement parent = _selected ?? _root;
        if (!IsContainer(parent))
        {
            parent = FindParent(_root, parent) ?? _root;
        }
        parent.Children.Add(child);
        _selected = child;
        TouchLayout();
        RenderAll();
        Status($"Added {kind}");
    }

    private static UiElement MakeDefault(string kind)
    {
        UiElement e = UiElement.Make(kind);
        e.Id = kind + Environment.TickCount % 1000;
        switch (kind)
        {
            case "label":
            case "textblock":
                e.Text = "Label";
                break;
            case "button":
                e.Text = "Button";
                e.Background = UiColors.DarkGray;
                e.Width = 60;
                break;
            case "textbox":
                e.Text = "text";
                e.Width = 80;
                break;
            case "checkbox":
                e.Text = "Check";
                break;
            case "radio":
                e.Text = "Option";
                e.Group = "group1";
                break;
            case "slider":
                e.Width = 100;
                e.Max = 100;
                e.Value = 50;
                break;
            case "progress":
                e.Width = 100;
                e.Value = 60;
                e.Foreground = UiColors.Green;
                break;
            case "listbox":
                e.Items.Add("Item 1");
                e.Items.Add("Item 2");
                e.Width = 100;
                break;
            case "rect":
            case "image":
                e.Width = 40;
                e.Height = 24;
                e.Background = UiColors.Blue;
                break;
            case "grid":
                e.Columns = 2;
                break;
            case "scrollviewer":
                e.Height = 60;
                break;

            // Everything below arrives on the canvas already showing
            // something. A control dropped from a toolbox that renders as an
            // empty rectangle tells you nothing about whether you wanted it.
            case "combobox":
                e.Items.Add("slow");
                e.Items.Add("normal");
                e.Items.Add("fast");
                e.Selected = 0;
                e.Width = 110;
                break;
            case "textflow":
                e.Text = "Wrapped text flows to the width it is given.";
                e.Width = 160;
                break;
            case "gauge":
                e.Width = 90;
                e.Height = 64;
                e.Max = 100;
                e.Value = 72;
                e.Foreground = UiColors.Green;
                break;
            case "chart":
                e.Width = 140;
                e.Height = 56;
                e.Foreground = UiColors.Cyan;
                foreach (int sample in new[] { 12, 18, 14, 26, 22, 31, 27, 38 })
                {
                    e.Series.Add(sample);
                }
                break;
            case "datagrid":
                e.Columns = 3;
                e.Width = 200;
                e.Items.Add("Sensor|Value|Unit");
                e.Items.Add("temp|21.4|C");
                e.Items.Add("rh|48|%");
                break;
            case "treeview":
                e.Width = 140;
                UiElement branch = UiElement.Make("label");
                branch.Text = "sensors";
                branch.Checked = true;
                UiElement leafA = UiElement.Make("label");
                leafA.Text = "temp";
                UiElement leafB = UiElement.Make("label");
                leafB.Text = "humidity";
                branch.Add(leafA);
                branch.Add(leafB);
                e.Add(branch);
                break;
            case "calendar":
                e.Width = 180;
                e.Year = DateTime.Now.Year;
                e.Month = DateTime.Now.Month;
                e.Value = DateTime.Now.Day;
                break;
            case "messagebox":
                e.Text = "Saved to the device.";
                e.Width = 200;
                e.Height = 100;
                e.Background = UiColors.DarkGray;
                break;
            case "groupbox":
                e.Text = "Group";
                e.Width = 160;
                break;
            case "expander":
                e.Text = "Advanced";
                e.Checked = true;
                e.Width = 160;
                break;
            case "tabcontrol":
                e.Width = 200;
                e.Height = 100;
                e.Selected = 0;
                UiElement first = UiElement.Make("tabitem");
                first.Text = "One";
                UiElement second = UiElement.Make("tabitem");
                second.Text = "Two";
                e.Add(first);
                e.Add(second);
                break;
            case "tabitem":
                e.Text = "Tab";
                break;
            case "dockpanel":
                e.Width = 180;
                e.Height = 100;
                break;
            case "ellipse":
                e.Width = 40;
                e.Height = 40;
                e.Background = UiColors.Blue;
                break;
            case "line":
                e.X2 = 60;
                e.Y2 = 20;
                e.Width = 60;
                e.Height = 20;
                break;
            case "polygon":
                e.Width = 48;
                e.Height = 40;
                foreach (int coord in new[] { 24, 0, 48, 40, 0, 40 })
                {
                    e.Points.Add(coord);
                }
                break;
        }
        return e;
    }

    private static bool IsContainer(UiElement e)
    {
        return e.Kind == "window" || e.Kind == "stack" || e.Kind == "panel"
            || e.Kind == "border" || e.Kind == "canvas" || e.Kind == "grid"
            || e.Kind == "scrollviewer" || e.Kind == "dockpanel"
            || e.Kind == "groupbox" || e.Kind == "expander"
            || e.Kind == "tabcontrol" || e.Kind == "tabitem"
            || e.Kind == "treeview" || e.Kind == "messagebox";
    }

    private static UiElement? FindParent(UiElement node, UiElement target)
    {
        for (int i = 0; i < node.Children.Count; i++)
        {
            if (node.Children[i] == target)
            {
                return node;
            }
            UiElement? deep = FindParent(node.Children[i], target);
            if (deep != null)
            {
                return deep;
            }
        }
        return null;
    }

    // ---- rendering ---------------------------------------------------

    private void RenderAll()
    {
        _map = DesignRenderer.Render(DesignCanvas, _root);
        DrawSelectionAdorner();
        BuildTree();
        BuildProperties();
        UpdateReadout();
        if (ReferenceEquals(CentreTabs.SelectedItem, XmlTab))
        {
            ReloadXmlPane();
        }
    }

    private void DrawSelectionAdorner()
    {
        if (_selected == null)
        {
            return;
        }
        var box = new Rectangle
        {
            Width = Math.Max(2, _selected.LayoutW),
            Height = Math.Max(2, _selected.LayoutH),
            Stroke = (Brush)FindResource("Amber"),
            StrokeThickness = 1,
            StrokeDashArray = new DoubleCollection { 3, 2 },
            Fill = System.Windows.Media.Brushes.Transparent,
            IsHitTestVisible = false,
        };
        Canvas.SetLeft(box, _selected.LayoutX);
        Canvas.SetTop(box, _selected.LayoutY);
        DesignCanvas.Children.Add(box);
    }

    /// <summary>
    /// The instrument line: what is selected, where it landed and in which
    /// colours — the numbers you would otherwise hunt for in the property grid.
    /// </summary>
    private void UpdateReadout()
    {
        (int w, int h) = GetPanelSize();
        PanelReadout.Text = $"PANEL {w}×{h}";

        if (_selected == null)
        {
            SelReadout.Text = "—";
            PosReadout.Text = SizeReadout.Text = FgReadout.Text = BgReadout.Text = "—";
            return;
        }
        SelReadout.Text = _selected.Id.Length > 0 ? $"{_selected.Kind} #{_selected.Id}" : _selected.Kind;
        PosReadout.Text = $"{_selected.LayoutX},{_selected.LayoutY}";
        SizeReadout.Text = $"{_selected.LayoutW}×{_selected.LayoutH}";
        FgReadout.Text = Hex(_selected.Foreground);
        BgReadout.Text = Hex(_selected.Background);
    }

    // ---- selection + drag-to-move -----------------------------------

    private bool _dragging;
    private Point _dragLast;
    private bool _dragMoved;

    private void OnCanvasMouseDown(object sender, MouseButtonEventArgs e)
    {
        Point p = e.GetPosition(DesignCanvas);
        // The model's own hit-test picks the topmost element at the point.
        UiElement? hit = _root.HitTest((int)p.X, (int)p.Y);
        _selected = hit ?? _root;

        // Begin a drag if the picked element lives in a canvas (freely moved).
        if (_selected != null && DragTool.CanMove(_root, _selected))
        {
            _dragging = true;
            _dragMoved = false;
            _dragLast = p;
            DesignCanvas.CaptureMouse();
        }
        RenderAll();
        e.Handled = true;
    }

    private void OnCanvasMouseMove(object sender, MouseEventArgs e)
    {
        if (!_dragging || _selected == null)
        {
            return;
        }
        Point p = e.GetPosition(DesignCanvas);
        int dx = (int)(p.X - _dragLast.X);
        int dy = (int)(p.Y - _dragLast.Y);
        if (DragTool.MoveBy(_root, _selected, dx, dy))
        {
            _dragLast = p;
            _dragMoved = true;
            TouchLayout();
            RenderAll();
        }
    }

    private void OnCanvasMouseUp(object sender, MouseButtonEventArgs e)
    {
        if (_dragging)
        {
            _dragging = false;
            DesignCanvas.ReleaseMouseCapture();
            if (_dragMoved && _selected != null)
            {
                Status($"Moved {_selected.Kind} to ({_selected.X}, {_selected.Y})");
            }
        }
    }

    // ---- tree --------------------------------------------------------

    private void BuildTree()
    {
        Tree.Items.Clear();
        Tree.Items.Add(BuildTreeNode(_root));
    }

    private TreeViewItem BuildTreeNode(UiElement e)
    {
        var item = new TreeViewItem
        {
            Header = e.Id.Length > 0 ? $"{e.Kind}  #{e.Id}" : e.Kind,
            Tag = e,
            IsExpanded = true,
        };
        if (e == _selected)
        {
            item.IsSelected = true;
        }
        for (int i = 0; i < e.Children.Count; i++)
        {
            item.Items.Add(BuildTreeNode(e.Children[i]));
        }
        return item;
    }

    private bool _syncingTree;

    private void OnTreeSelect(object sender, RoutedPropertyChangedEventArgs<object> e)
    {
        if (_syncingTree)
        {
            return;
        }
        if (e.NewValue is TreeViewItem item && item.Tag is UiElement el)
        {
            _selected = el;
            _syncingTree = true;
            _map = DesignRenderer.Render(DesignCanvas, _root);
            DrawSelectionAdorner();
            BuildProperties();
            UpdateReadout();
            _syncingTree = false;
        }
    }

    // ---- property grid ----------------------------------------------

    private void BuildProperties()
    {
        Properties.Children.Clear();
        if (_selected == null)
        {
            return;
        }
        UiElement s = _selected;

        AddPropRow("kind", s.Kind, v => { }, readOnly: true);
        AddPropRow("id", s.Id, v => s.Id = v);
        AddPropRow("text", s.Text, v => s.Text = v);
        AddIntRow("x", s.X, v => s.X = v);
        AddIntRow("y", s.Y, v => s.Y = v);
        AddIntRow("width", s.Width, v => s.Width = v);
        AddIntRow("height", s.Height, v => s.Height = v);
        AddIntRow("scale", s.Scale, v => s.Scale = v);
        AddColorRow("fg", s.Foreground, v => s.Foreground = v);
        AddColorRow("bg", s.Background, v => s.Background = v);
        AddColorRow("border", s.Border, v => s.Border = v);

        if (s.Kind == "slider" || s.Kind == "progress")
        {
            AddIntRow("min", s.Min, v => s.Min = v);
            AddIntRow("max", s.Max, v => s.Max = v);
            AddIntRow("value", s.Value, v => s.Value = v);
        }
        if (s.Kind == "checkbox" || s.Kind == "radio")
        {
            AddBoolRow("checked", s.Checked, v => s.Checked = v);
        }
        if (s.Kind == "radio")
        {
            AddPropRow("group", s.Group, v => s.Group = v);
        }
        if (s.Kind == "grid")
        {
            AddIntRow("columns", s.Columns, v => s.Columns = v);
        }
        if (IsContainer(s))
        {
            AddIntRow("pad", s.Padding, v => s.Padding = v);
            AddIntRow("gap", s.Gap, v => s.Gap = v);
            AddBoolRow("horizontal", s.Horizontal, v => s.Horizontal = v);
        }
        if (s.Kind == "listbox")
        {
            AddPropRow("items (; sep)", string.Join(";", s.Items), v =>
            {
                s.Items.Clear();
                foreach (string part in v.Split(';'))
                {
                    s.Items.Add(part);
                }
            });
            AddIntRow("selected", s.Selected, v => s.Selected = v);
        }
    }

    private void AddPropRow(string label, string value, Action<string> set, bool readOnly = false)
    {
        var panel = new DockPanel { Margin = new Thickness(0, 0, 0, 3) };
        panel.Children.Add(new TextBlock
        {
            Text = label,
            Width = 92,
            VerticalAlignment = VerticalAlignment.Center,
            Style = (Style)FindResource("Readout"),
        });
        var box = new TextBox { Text = value, IsReadOnly = readOnly };
        if (!readOnly)
        {
            box.LostFocus += (_, _) => { set(box.Text); TouchLayout(); RenderAll(); };
            box.KeyDown += (_, e) =>
            {
                if (e.Key == Key.Enter)
                {
                    set(box.Text);
                    TouchLayout();
                    RenderAll();
                }
            };
        }
        Properties.Children.Add(panel);
        panel.Children.Add(box);
    }

    private void AddIntRow(string label, int value, Action<int> set)
    {
        AddPropRow(label, value.ToString(), v =>
        {
            if (int.TryParse(v, out int n))
            {
                set(n);
            }
        });
    }

    private void AddColorRow(string label, int value, Action<int> set)
    {
        var panel = new DockPanel { Margin = new Thickness(0, 0, 0, 3) };
        panel.Children.Add(new TextBlock
        {
            Text = label,
            Width = 92,
            VerticalAlignment = VerticalAlignment.Center,
            Style = (Style)FindResource("Readout"),
        });
        // A swatch beside the hex: RGB565 is unreadable as four digits, and the
        // quantised colour is what the panel will actually show.
        var swatch = new Border
        {
            Width = 18,
            Height = 18,
            CornerRadius = new CornerRadius(2),
            BorderBrush = (Brush)FindResource("Rail"),
            BorderThickness = new Thickness(1),
            Background = new SolidColorBrush(DesignRenderer.FromRgb565(value)),
            Margin = new Thickness(0, 0, 4, 0),
        };
        DockPanel.SetDock(swatch, Dock.Right);
        panel.Children.Add(swatch);

        var box = new TextBox { Text = Hex(value) };
        box.LostFocus += (_, _) => Commit();
        box.KeyDown += (_, e) =>
        {
            if (e.Key == Key.Enter)
            {
                Commit();
            }
        };
        panel.Children.Add(box);
        Properties.Children.Add(panel);

        void Commit()
        {
            set(ParseHex(box.Text));
            TouchLayout();
            RenderAll();
        }
    }

    private void AddBoolRow(string label, bool value, Action<bool> set)
    {
        var panel = new DockPanel { Margin = new Thickness(0, 1, 0, 4) };
        panel.Children.Add(new TextBlock
        {
            Text = label,
            Width = 92,
            VerticalAlignment = VerticalAlignment.Center,
            Style = (Style)FindResource("Readout"),
        });
        var check = new CheckBox { IsChecked = value, VerticalAlignment = VerticalAlignment.Center };
        check.Checked += (_, _) => { set(true); TouchLayout(); RenderAll(); };
        check.Unchecked += (_, _) => { set(false); TouchLayout(); RenderAll(); };
        panel.Children.Add(check);
        Properties.Children.Add(panel);
    }

    // ---- documents ---------------------------------------------------

    /// <summary>True when the code tab is in front; otherwise the layout is the active document.</summary>
    private bool CodeIsActive => ReferenceEquals(CentreTabs.SelectedItem, CodeTab);

    private void TouchLayout()
    {
        _layoutDirty = true;
        UpdateTabHeaders();
    }

    private void UpdateTabHeaders()
    {
        string layoutName = _layoutFile == null ? "LAYOUT XML" : Path.GetFileName(_layoutFile).ToUpperInvariant();
        XmlTab.Header = layoutName + (_layoutDirty || XmlPane.Dirty ? " •" : "");
        string codeName = _codeFile == null ? "CODE" : Path.GetFileName(_codeFile).ToUpperInvariant();
        CodeTab.Header = codeName + (CodePane.Dirty ? " •" : "");
    }

    private void OnNew(object sender, RoutedEventArgs e)
    {
        if (CodeIsActive)
        {
            if (!ConfirmDiscard(CodePane.Dirty, "the code"))
            {
                return;
            }
            CodePane.Text = "";
            _codeFile = null;
            _codeFileName.Text = "(new file)";
            Status("New C# file");
        }
        else
        {
            if (!ConfirmDiscard(_layoutDirty, "the layout"))
            {
                return;
            }
            _root = Ui.LoadXml("<window width=\"320\" height=\"240\" bg=\"0000\" pad=\"8\" gap=\"6\"/>");
            _selected = _root;
            _layoutFile = null;
            _layoutDirty = false;
            RenderAll();
            Status("New layout");
        }
        UpdateTabHeaders();
    }

    private void OnOpen(object sender, RoutedEventArgs e)
    {
        var dlg = new OpenFileDialog
        {
            Filter = "Layout or code (*.xml;*.cs)|*.xml;*.cs"
                + "|RustNet UI layout (*.xml)|*.xml"
                + "|C# source (*.cs)|*.cs"
                + "|All files|*.*",
        };
        if (dlg.ShowDialog() == true)
        {
            OpenFile(dlg.FileName);
        }
    }

    /// <summary>Open a layout or a C# file; the extension decides which pane it lands in.</summary>
    public void OpenFile(string path)
    {
        try
        {
            if (Path.GetExtension(path).Equals(".cs", StringComparison.OrdinalIgnoreCase))
            {
                if (!ConfirmDiscard(CodePane.Dirty, "the code"))
                {
                    return;
                }
                CodePane.Text = File.ReadAllText(path);
                CodePane.Syntax = "csharp";
                _codeFile = path;
                _codeFileName.Text = Path.GetFileName(path);
                CentreTabs.SelectedItem = CodeTab;
                Status("Opened " + path);
            }
            else
            {
                if (!ConfirmDiscard(_layoutDirty, "the layout"))
                {
                    return;
                }
                _root = Ui.LoadXml(File.ReadAllText(path));
                _selected = _root;
                _layoutFile = path;
                _layoutDirty = false;
                CentreTabs.SelectedItem = DesignTab;
                RenderAll();
                Status("Opened " + path);
            }
            UpdateTabHeaders();
        }
        catch (Exception ex)
        {
            MessageBox.Show("Could not open: " + ex.Message, "RustNet UI Designer");
        }
    }

    private void OnSave(object sender, RoutedEventArgs e)
    {
        if (CodeIsActive)
        {
            if (_codeFile == null)
            {
                OnSaveAs(sender, e);
                return;
            }
            File.WriteAllText(_codeFile, CodePane.Text);
            CodePane.MarkClean();
            Status("Saved " + _codeFile);
        }
        else
        {
            if (_layoutFile == null)
            {
                OnSaveAs(sender, e);
                return;
            }
            File.WriteAllText(_layoutFile, CurrentLayoutXml());
            _layoutDirty = false;
            XmlPane.MarkClean();
            Status("Saved " + _layoutFile);
        }
        UpdateTabHeaders();
    }

    private void OnSaveAs(object sender, RoutedEventArgs e)
    {
        if (CodeIsActive)
        {
            var dlg = new SaveFileDialog
            {
                Filter = "C# source (*.cs)|*.cs|All files|*.*",
                FileName = _codeFile != null ? Path.GetFileName(_codeFile) : "Program.cs",
            };
            if (dlg.ShowDialog() != true)
            {
                return;
            }
            _codeFile = dlg.FileName;
            File.WriteAllText(_codeFile, CodePane.Text);
            CodePane.MarkClean();
            _codeFileName.Text = Path.GetFileName(_codeFile);
            Status("Saved " + _codeFile);
        }
        else
        {
            var dlg = new SaveFileDialog
            {
                Filter = "RustNet UI layout (*.xml)|*.xml|All files|*.*",
                FileName = _layoutFile != null ? Path.GetFileName(_layoutFile) : "ui.xml",
            };
            if (dlg.ShowDialog() != true)
            {
                return;
            }
            _layoutFile = dlg.FileName;
            File.WriteAllText(_layoutFile, CurrentLayoutXml());
            _layoutDirty = false;
            XmlPane.MarkClean();
            Status("Saved " + _layoutFile);
        }
        UpdateTabHeaders();
    }

    private void OnClose(object sender, RoutedEventArgs e)
    {
        if (CodeIsActive)
        {
            if (!ConfirmDiscard(CodePane.Dirty, "the code"))
            {
                return;
            }
            CodePane.Text = "";
            _codeFile = null;
            _codeFileName.Text = "(nothing generated yet)";
            Status("Closed the code file");
        }
        else
        {
            if (!ConfirmDiscard(_layoutDirty, "the layout"))
            {
                return;
            }
            _root = Ui.LoadXml("<window width=\"320\" height=\"240\" bg=\"0000\" pad=\"8\" gap=\"6\"/>");
            _selected = _root;
            _layoutFile = null;
            _layoutDirty = false;
            RenderAll();
            Status("Closed the layout");
        }
        UpdateTabHeaders();
    }

    /// <summary>
    /// The layout as text. When the XML pane has unapplied edits its text is
    /// what the person means by "the layout", so it wins over the tree.
    /// </summary>
    private string CurrentLayoutXml()
        => XmlPane.Dirty && XmlPane.Text.Trim().Length > 0 ? XmlPane.Text : Ui.ToXml(_root);

    private bool ConfirmDiscard(bool dirty, string what)
        => !dirty || MessageBox.Show(
            $"Discard unsaved changes to {what}?", "RustNet UI Designer",
            MessageBoxButton.OKCancel, MessageBoxImage.Question) == MessageBoxResult.OK;

    // ---- edit --------------------------------------------------------

    private void OnDelete(object sender, RoutedEventArgs e)
    {
        if (_selected == null || _selected == _root)
        {
            return;
        }
        UiElement? parent = FindParent(_root, _selected);
        if (parent != null)
        {
            parent.Children.Remove(_selected);
            _selected = parent;
            TouchLayout();
            RenderAll();
            Status("Deleted");
        }
    }

    private void OnMoveUp(object sender, RoutedEventArgs e) => Move(-1);
    private void OnMoveDown(object sender, RoutedEventArgs e) => Move(1);

    private void Move(int delta)
    {
        if (_selected == null)
        {
            return;
        }
        UiElement? parent = FindParent(_root, _selected);
        if (parent == null)
        {
            return;
        }
        int i = parent.Children.IndexOf(_selected);
        int j = i + delta;
        if (j >= 0 && j < parent.Children.Count)
        {
            parent.Children.RemoveAt(i);
            parent.Children.Insert(j, _selected);
            TouchLayout();
            RenderAll();
        }
    }

    // ---- centre pane: XML and code ----------------------------------

    private void OnCentreTabChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!ReferenceEquals(e.OriginalSource, CentreTabs))
        {
            return;
        }
        if (ReferenceEquals(CentreTabs.SelectedItem, XmlTab) && !XmlPane.Dirty)
        {
            ReloadXmlPane();
        }
        UpdateRunButton();
    }

    private void ReloadXmlPane()
    {
        XmlPane.Text = Ui.ToXml(_root);
        _xmlHint.Text = "edit here, then apply";
    }

    private void OnReloadXml(object sender, RoutedEventArgs e)
    {
        ReloadXmlPane();
        UpdateTabHeaders();
        Status("Layout XML reloaded from the canvas");
    }

    private void OnApplyXml(object sender, RoutedEventArgs e)
    {
        try
        {
            ApplyLayoutXml(XmlPane.Text);
            XmlPane.MarkClean();
            _xmlHint.Text = "applied";
            Status("Layout applied from the XML pane");
        }
        catch (Exception ex)
        {
            _xmlHint.Text = "not applied";
            MessageBox.Show("That XML did not parse:\n\n" + ex.Message, "Apply to canvas");
        }
    }

    private void OnSendCodeToAssistant(object sender, RoutedEventArgs e)
    {
        if (CodePane.Text.Trim().Length == 0)
        {
            Status("The code pane is empty.");
            return;
        }
        AssistantToggle.IsChecked = true;
        Chat.Compose("Review the code in my code pane (read it with get_generated_code) and tell me what "
            + "would break on the device. Then put a corrected version back with set_generated_code.");
    }

    // ---- panels ------------------------------------------------------

    private static void Toggle(System.Windows.Controls.Primitives.ToggleButton button)
        => button.IsChecked = button.IsChecked != true;

    private void OnPanelToggled(object sender, RoutedEventArgs e)
    {
        // The toggles start checked in markup, so this fires while the rest of
        // the tree is still being built. The initial state already matches.
        if (ToolboxColumn == null || Chat == null || OutputPane == null)
        {
            return;
        }

        SetColumn(ToolboxColumn, ToolboxSplitterColumn, ToolboxPanel, ToolboxSplitter,
            ToolboxToggle.IsChecked == true, 172);
        SetColumn(InspectorColumn, InspectorSplitterColumn, InspectorPanel, InspectorSplitter,
            InspectorToggle.IsChecked == true, 292);
        SetColumn(ChatColumn, ChatSplitterColumn, Chat, ChatSplitter,
            AssistantToggle.IsChecked == true, 420);

        OutputPane.Visibility = OutputToggle.IsChecked == true ? Visibility.Visible : Visibility.Collapsed;

        if (AssistantToggle.IsChecked == true)
        {
            Chat.FocusComposer();
        }
    }

    /// <summary>
    /// Collapse or restore one panel column. The width is remembered so hiding
    /// and showing does not resize the panel the person set.
    /// </summary>
    private void SetColumn(ColumnDefinition column, ColumnDefinition splitterColumn,
        UIElement panel, UIElement splitter, bool show, double fallback)
    {
        if (!show && column.Width.Value > 0)
        {
            _panelWidths[column] = column.Width;
        }
        GridLength width = show
            ? _panelWidths.TryGetValue(column, out GridLength remembered) ? remembered : new GridLength(fallback)
            : new GridLength(0);

        column.Width = width;
        splitterColumn.Width = new GridLength(show ? 1 : 0);
        panel.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
        splitter.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
    }

    // ---- output ------------------------------------------------------

    /// <summary>Append a line to the output pane, opening it on first use.</summary>
    private void Output(string line) => Dispatcher.Invoke(() =>
    {
        if (OutputToggle.IsChecked != true)
        {
            OutputToggle.IsChecked = true;
        }
        OutputText.AppendText(line + Environment.NewLine);
        OutputScroll.ScrollToEnd();
    });

    private void OnClearOutput(object sender, RoutedEventArgs e) => OutputText.Clear();

    private void OnCopyOutput(object sender, RoutedEventArgs e)
    {
        if (OutputText.Text.Length > 0)
        {
            Clipboard.SetText(OutputText.Text);
            Status("Output copied");
        }
    }

    private void OnCloseOutput(object sender, RoutedEventArgs e) => OutputToggle.IsChecked = false;

    // ---- devices and deployment --------------------------------------

    /// <summary>
    /// Offer the candidates before anything has been probed, so Run works
    /// immediately against the virtual device.
    /// </summary>
    private void SeedTargets()
    {
        _targets.Clear();
        foreach (string spec in DeviceDiscovery.Candidates())
        {
            string label = spec == DeviceDiscovery.VirtualDeviceSpec
                ? "virtual device — not probed"
                : spec.Replace("serial:", "") + " — not probed";
            _targets.Add(new DeviceTarget(spec, label));
        }
        RebuildTargetBox(_targets[0]);
    }

    private void RebuildTargetBox(DeviceTarget? select)
    {
        TargetBox.ItemsSource = null;
        TargetBox.ItemsSource = _targets;
        TargetBox.SelectedItem = select ?? (_targets.Count > 0 ? _targets[0] : null);
    }

    private void OnTargetChanged(object sender, SelectionChangedEventArgs e)
    {
        _target = TargetBox.SelectedItem as DeviceTarget;
        DeployState.Text = _target == null
            ? ""
            : _target.Answered ? $"chip {_target.Chip}" : "not probed";
    }

    private async void OnDetectDevices(object sender, RoutedEventArgs e)
    {
        Status("Probing devices…");
        Output("--- detecting devices ---");
        try
        {
            List<DeviceTarget> found = await DeviceDiscovery.ScanAsync(Output, CancellationToken.None);
            if (found.Count > 0)
            {
                _targets.Clear();
                _targets.AddRange(found);
                // Keep the unprobed candidates too: a board can be plugged in
                // after a scan, and re-scanning for it should not be the only way.
                foreach (string spec in DeviceDiscovery.Candidates())
                {
                    if (!found.Exists(t => t.Spec == spec))
                    {
                        _targets.Add(new DeviceTarget(spec, spec.Replace("serial:", "") + " — no answer"));
                    }
                }
                RebuildTargetBox(found[0]);
                Status($"{found.Count} device(s) answered.");
            }
            else
            {
                Status("No device answered.");
            }
        }
        catch (Exception ex)
        {
            Output("detect failed: " + ex.Message);
            Status("Detect failed.");
        }
    }

    private void UpdateRunButton()
    {
        bool code = CodeIsActive;
        RunButton.Content = code ? "Run ▸" : "Push layout ▸";
        RunButton.ToolTip = code
            ? "Build the code pane, sign it and flash it to the target, then start it (F5)"
            : $"Push the layout to {Deployer.DefaultLayoutPath} on the target (F5)";
    }

    private async void OnRun(object sender, RoutedEventArgs e)
    {
        if (_run != null)
        {
            return;
        }
        if (_target == null)
        {
            Status("Pick a target first.");
            return;
        }

        _run = new CancellationTokenSource();
        RunButton.Visibility = Visibility.Collapsed;
        CancelRunButton.Visibility = Visibility.Visible;
        var deployer = new Deployer(Output);
        try
        {
            Deployer.Result result = CodeIsActive
                ? await deployer.DeployCodeAsync(
                    CodePane.Text, AppNameBox.Text, _target, _signingKey, _workspaceRoot,
                    start: true, _run.Token)
                : await deployer.PushLayoutAsync(
                    CurrentLayoutXml(), Deployer.DefaultLayoutPath, _target, _run.Token);
            Status(result.Summary);
        }
        catch (OperationCanceledException)
        {
            Output("cancelled");
            Status("Cancelled.");
        }
        catch (Exception ex)
        {
            Output("failed: " + ex.Message);
            Status("Run failed: " + ex.Message);
        }
        finally
        {
            _run.Dispose();
            _run = null;
            RunButton.Visibility = Visibility.Visible;
            CancelRunButton.Visibility = Visibility.Collapsed;
        }
    }

    private void OnCancelRun(object sender, RoutedEventArgs e) => _run?.Cancel();

    private async void OnStopApp(object sender, RoutedEventArgs e)
    {
        if (_target == null)
        {
            return;
        }
        Deployer.Result result = await new Deployer(Output).StopAsync(_target, CancellationToken.None);
        Status(result.Summary);
    }

    private async void OnReadLogs(object sender, RoutedEventArgs e)
    {
        if (_target == null)
        {
            return;
        }
        Output($"--- device log ({_target.Spec}) ---");
        Deployer.Result result = await new Deployer(Output).ReadLogsAsync(_target, 100, CancellationToken.None);
        Status(result.Summary);
    }

    // ---- IDesignerBridge --------------------------------------------

    public string GetLayoutXml() => Dispatcher.Invoke(() => Ui.ToXml(_root));

    public void ApplyLayoutXml(string xml)
    {
        // Parse before touching the canvas so a bad document leaves the design
        // alone and the caller gets the parser's message.
        UiElement parsed = Ui.LoadXml(xml);
        Dispatcher.Invoke(() =>
        {
            _root = parsed;
            _selected = _root;
            _layoutDirty = true;
            CentreTabs.SelectedItem = DesignTab;
            RenderAll();
            UpdateTabHeaders();
        });
    }

    public (int Width, int Height) GetPanelSize() => Dispatcher.Invoke(() =>
        (_root.Width > 0 ? _root.Width : 160, _root.Height > 0 ? _root.Height : 128));

    public string DescribeSelection() => Dispatcher.Invoke(() =>
    {
        if (_selected == null)
        {
            return "none";
        }
        string name = _selected.Id.Length > 0 ? $"{_selected.Kind} #{_selected.Id}" : _selected.Kind;
        return $"{name} at {_selected.LayoutX},{_selected.LayoutY} sized {_selected.LayoutW}x{_selected.LayoutH}";
    });

    public void SetGeneratedCode(string fileName, string language, string code) => Dispatcher.Invoke(() =>
    {
        CodePane.Syntax = language.Length > 0 ? language : "csharp";
        CodePane.Text = code;
        _codeFile = null;
        _codeFileName.Text = $"{fileName} · {code.Split('\n').Length} lines · unsaved";
        CentreTabs.SelectedItem = CodeTab;
        UpdateTabHeaders();
        Status("Generated " + fileName);
    });

    public string GetGeneratedCode() => Dispatcher.Invoke(() => CodePane.Text);

    // ---- helpers -----------------------------------------------------

    private void Status(string msg) => Dispatcher.Invoke(() => StatusText.Text = msg);

    private static string Hex(int v)
    {
        return v.ToString("X4");
    }

    private static int ParseHex(string s)
    {
        try
        {
            return Convert.ToInt32(s.Trim().TrimStart('#'), 16);
        }
        catch
        {
            return 0;
        }
    }

    /// <summary>Minimal ICommand so a keyboard gesture can call a method.</summary>
    private sealed class RelayCommand : ICommand
    {
        private readonly Action _run;

        public RelayCommand(Action run) => _run = run;

        // Always executable, so there is nothing to notify about.
        public event EventHandler? CanExecuteChanged
        {
            add { }
            remove { }
        }

        public bool CanExecute(object? parameter) => true;

        public void Execute(object? parameter) => _run();
    }
}
