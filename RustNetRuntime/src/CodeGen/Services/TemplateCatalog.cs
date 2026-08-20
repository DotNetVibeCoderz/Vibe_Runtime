using CodeGen.Models;

namespace CodeGen.Services;

/// <summary>
/// The built-in project templates.
///
/// Every template compiles as written and most run on RustCLR as well as .NET —
/// the console and library ones stay inside the IL subset the interpreter
/// supports, which is why they use explicit loops and string building rather
/// than LINQ.
/// </summary>
public static class TemplateCatalog
{
    private const string ConsoleProject = """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <OutputType>Exe</OutputType>
            <TargetFramework>net10.0</TargetFramework>
            <Nullable>enable</Nullable>
            <ImplicitUsings>enable</ImplicitUsings>
            <RootNamespace>{NAMESPACE}</RootNamespace>
            <AssemblyName>{NAME}</AssemblyName>
          </PropertyGroup>
        </Project>
        """;

    private const string WebProject = """
        <Project Sdk="Microsoft.NET.Sdk.Web">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <Nullable>enable</Nullable>
            <ImplicitUsings>enable</ImplicitUsings>
            <RootNamespace>{NAMESPACE}</RootNamespace>
            <AssemblyName>{NAME}</AssemblyName>
          </PropertyGroup>
          <ItemGroup>
            <PackageReference Include="Swashbuckle.AspNetCore" Version="7.2.0" />
          </ItemGroup>
        </Project>
        """;

    private const string DesktopProject = """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <OutputType>WinExe</OutputType>
            <TargetFramework>net10.0</TargetFramework>
            <Nullable>enable</Nullable>
            <ImplicitUsings>enable</ImplicitUsings>
            <RootNamespace>{NAMESPACE}</RootNamespace>
            <AssemblyName>{NAME}</AssemblyName>
            <AvaloniaUseCompiledBindingsByDefault>true</AvaloniaUseCompiledBindingsByDefault>
          </PropertyGroup>
          <ItemGroup>
            <PackageReference Include="Avalonia" Version="11.2.3" />
            <PackageReference Include="Avalonia.Desktop" Version="11.2.3" />
            <PackageReference Include="Avalonia.Themes.Fluent" Version="11.2.3" />
          </ItemGroup>
        </Project>
        """;

    private const string LibraryProject = """
        <Project Sdk="Microsoft.NET.Sdk">
          <PropertyGroup>
            <TargetFramework>net10.0</TargetFramework>
            <Nullable>enable</Nullable>
            <ImplicitUsings>enable</ImplicitUsings>
            <RootNamespace>{NAMESPACE}</RootNamespace>
            <AssemblyName>{NAME}</AssemblyName>
          </PropertyGroup>
        </Project>
        """;

    /// <summary>The blank template: a project file and an empty entry point.</summary>
    public static ProjectTemplate Blank { get; } = new()
    {
        Id = "blank",
        Name = "Blank",
        Summary = "An empty console project. Nothing but an entry point.",
        Category = TemplateCategory.Console,
        Domain = TemplateDomain.Runtime,
        Files =
        [
            new("{NAME}.csproj", ConsoleProject),
            new("Program.cs", """
                namespace {NAMESPACE};

                public static class Program
                {
                    public static void Main()
                    {
                        Console.WriteLine("Hello from {NAME}.");
                    }
                }
                """),
            new("README.md", """
                # {NAME}

                Created with CodeGen. Runs on .NET and on RustCLR.

                ```
                dotnet build -c Release
                rustnet run bin/Release/net10.0/{NAME}.dll
                ```
                """),
        ],
    };

    /// <summary>Every template, blank first.</summary>
    public static IReadOnlyList<ProjectTemplate> All { get; } = BuildAll();

    public static IEnumerable<ProjectTemplate> ByCategory(TemplateCategory category) =>
        All.Where(t => t.Category == category);

    public static ProjectTemplate? Find(string id) =>
        All.FirstOrDefault(t => t.Id.Equals(id, StringComparison.OrdinalIgnoreCase));

    private static List<ProjectTemplate> BuildAll()
    {
        var templates = new List<ProjectTemplate> { Blank };

        // ── Console ────────────────────────────────────────────────────────
        templates.Add(new ProjectTemplate
        {
            Id = "console-invoice",
            Name = "Invoice Calculator",
            Summary = "Line items, tax bands and a printed total. A small billing core.",
            Category = TemplateCategory.Console,
            Domain = TemplateDomain.Business,
            Files =
            [
                new("{NAME}.csproj", ConsoleProject),
                new("Invoice.cs", """
                    namespace {NAMESPACE};

                    public sealed class LineItem
                    {
                        public string Description = "";
                        public int Quantity;
                        public double UnitPrice;

                        public double Subtotal() { return Quantity * UnitPrice; }
                    }

                    public sealed class Invoice
                    {
                        public string Customer = "";
                        public LineItem[] Items = new LineItem[0];

                        /// <summary>Tax is charged per band, not on the whole total.</summary>
                        public double TaxRateFor(double subtotal)
                        {
                            if (subtotal < 1000) return 0.05;
                            if (subtotal < 10000) return 0.10;
                            return 0.11;
                        }

                        public double Subtotal()
                        {
                            double total = 0;
                            for (int i = 0; i < Items.Length; i++) total += Items[i].Subtotal();
                            return total;
                        }

                        public double Tax() { return Subtotal() * TaxRateFor(Subtotal()); }
                        public double Total() { return Subtotal() + Tax(); }
                    }
                    """),
                new("Program.cs", """
                    namespace {NAMESPACE};

                    public static class Program
                    {
                        public static void Main()
                        {
                            var invoice = new Invoice();
                            invoice.Customer = "Gravicode Studios";
                            invoice.Items = new LineItem[3];
                            invoice.Items[0] = new LineItem { Description = "Runtime license", Quantity = 2, UnitPrice = 2500 };
                            invoice.Items[1] = new LineItem { Description = "Support hours", Quantity = 10, UnitPrice = 150 };
                            invoice.Items[2] = new LineItem { Description = "Onboarding", Quantity = 1, UnitPrice = 750 };

                            Console.WriteLine("INVOICE  " + invoice.Customer);
                            Console.WriteLine("--------------------------------------------");
                            for (int i = 0; i < invoice.Items.Length; i++)
                            {
                                var item = invoice.Items[i];
                                Console.WriteLine(item.Description + "  x" + item.Quantity + "  = " + item.Subtotal());
                            }
                            Console.WriteLine("--------------------------------------------");
                            Console.WriteLine("Subtotal " + invoice.Subtotal());
                            Console.WriteLine("Tax      " + invoice.Tax());
                            Console.WriteLine("Total    " + invoice.Total());
                        }
                    }
                    """),
            ],
        });

        templates.Add(new ProjectTemplate
        {
            Id = "console-numerics",
            Name = "Numerical Methods",
            Summary = "Root finding, numerical integration and a convergence report.",
            Category = TemplateCategory.Console,
            Domain = TemplateDomain.Science,
            Files =
            [
                new("{NAME}.csproj", ConsoleProject),
                new("Solvers.cs", """
                    namespace {NAMESPACE};

                    public static class Solvers
                    {
                        /// <summary>Bisection: slow but guaranteed when the bracket is valid.</summary>
                        public static double Bisect(double lo, double hi, double tolerance, out int steps)
                        {
                            steps = 0;
                            double a = lo;
                            double b = hi;
                            while (b - a > tolerance && steps < 200)
                            {
                                double mid = (a + b) / 2;
                                if (F(a) * F(mid) <= 0) b = mid; else a = mid;
                                steps++;
                            }
                            return (a + b) / 2;
                        }

                        /// <summary>Composite Simpson's rule over n intervals (n must be even).</summary>
                        public static double Integrate(double lo, double hi, int intervals)
                        {
                            if (intervals % 2 != 0) intervals++;
                            double h = (hi - lo) / intervals;
                            double sum = F(lo) + F(hi);
                            for (int i = 1; i < intervals; i++)
                            {
                                double x = lo + i * h;
                                sum += F(x) * (i % 2 == 0 ? 2 : 4);
                            }
                            return sum * h / 3;
                        }

                        /// <summary>The function under study: x^3 - 2x - 5.</summary>
                        public static double F(double x) { return x * x * x - 2 * x - 5; }
                    }
                    """),
                new("Program.cs", """
                    namespace {NAMESPACE};

                    public static class Program
                    {
                        public static void Main()
                        {
                            int steps;
                            double root = Solvers.Bisect(2.0, 3.0, 1e-10, out steps);
                            Console.WriteLine("root of x^3 - 2x - 5");
                            Console.WriteLine("  x        = " + root);
                            Console.WriteLine("  f(x)     = " + Solvers.F(root));
                            Console.WriteLine("  steps    = " + steps);

                            Console.WriteLine("integral over [0, 2]");
                            Console.WriteLine("  n=10     " + Solvers.Integrate(0, 2, 10));
                            Console.WriteLine("  n=1000   " + Solvers.Integrate(0, 2, 1000));
                        }
                    }
                    """),
            ],
        });

        templates.Add(new ProjectTemplate
        {
            Id = "console-quiz",
            Name = "Quiz Engine",
            Summary = "Multiple-choice questions, scoring and a per-topic breakdown.",
            Category = TemplateCategory.Console,
            Domain = TemplateDomain.Education,
            Files =
            [
                new("{NAME}.csproj", ConsoleProject),
                new("Quiz.cs", """
                    namespace {NAMESPACE};

                    public sealed class Question
                    {
                        public string Topic = "";
                        public string Prompt = "";
                        public string[] Choices = new string[0];
                        public int CorrectIndex;
                    }

                    public sealed class Quiz
                    {
                        public Question[] Questions = new Question[0];

                        public int Ask(Question q, int number)
                        {
                            Console.WriteLine();
                            Console.WriteLine(number + ". " + q.Prompt + "  [" + q.Topic + "]");
                            for (int i = 0; i < q.Choices.Length; i++)
                            {
                                Console.WriteLine("   " + (i + 1) + ") " + q.Choices[i]);
                            }
                            Console.Write("   Your answer: ");
                            string line = Console.ReadLine();
                            int picked = 0;
                            if (!int.TryParse(line, out picked)) return 0;
                            return picked - 1 == q.CorrectIndex ? 1 : 0;
                        }
                    }
                    """),
                new("Program.cs", """
                    namespace {NAMESPACE};

                    public static class Program
                    {
                        public static void Main()
                        {
                            var quiz = new Quiz();
                            quiz.Questions = new Question[3];
                            quiz.Questions[0] = new Question
                            {
                                Topic = "Runtime",
                                Prompt = "What does the CLR execute directly?",
                                Choices = new string[] { "C# source", "IL bytecode", "Machine code" },
                                CorrectIndex = 1,
                            };
                            quiz.Questions[1] = new Question
                            {
                                Topic = "Memory",
                                Prompt = "What does a tracing collector start from?",
                                Choices = new string[] { "The roots", "The oldest object", "The largest object" },
                                CorrectIndex = 0,
                            };
                            quiz.Questions[2] = new Question
                            {
                                Topic = "Rust",
                                Prompt = "What does Rust use instead of a garbage collector?",
                                Choices = new string[] { "Reference counting only", "Ownership and borrowing", "Manual free()" },
                                CorrectIndex = 1,
                            };

                            int score = 0;
                            for (int i = 0; i < quiz.Questions.Length; i++)
                            {
                                score += quiz.Ask(quiz.Questions[i], i + 1);
                            }

                            Console.WriteLine();
                            Console.WriteLine("Score: " + score + " of " + quiz.Questions.Length);
                        }
                    }
                    """),
            ],
        });

        templates.Add(new ProjectTemplate
        {
            Id = "console-adventure",
            Name = "Text Adventure",
            Summary = "Rooms, exits and an inventory. A complete small game loop.",
            Category = TemplateCategory.Console,
            Domain = TemplateDomain.Games,
            Files =
            [
                new("{NAME}.csproj", ConsoleProject),
                new("World.cs", """
                    namespace {NAMESPACE};

                    public sealed class Room
                    {
                        public string Name = "";
                        public string Description = "";
                        public string North = "";
                        public string South = "";
                        public string Item = "";
                    }

                    public sealed class World
                    {
                        public Room[] Rooms = new Room[0];

                        public Room Find(string name)
                        {
                            for (int i = 0; i < Rooms.Length; i++)
                            {
                                if (Rooms[i].Name == name) return Rooms[i];
                            }
                            return Rooms[0];
                        }

                        public static World Build()
                        {
                            var world = new World();
                            world.Rooms = new Room[3];
                            world.Rooms[0] = new Room
                            {
                                Name = "foundry",
                                Description = "A cold foundry. Iron dust on every surface.",
                                North = "gallery",
                                Item = "hammer",
                            };
                            world.Rooms[1] = new Room
                            {
                                Name = "gallery",
                                Description = "A gallery of unfinished castings.",
                                North = "vault",
                                South = "foundry",
                            };
                            world.Rooms[2] = new Room
                            {
                                Name = "vault",
                                Description = "The vault. A sealed door, and a keyhole shaped like a hammer.",
                                South = "gallery",
                            };
                            return world;
                        }
                    }
                    """),
                new("Program.cs", """
                    namespace {NAMESPACE};

                    public static class Program
                    {
                        public static void Main()
                        {
                            var world = World.Build();
                            Room here = world.Find("foundry");
                            bool carryingHammer = false;

                            Console.WriteLine("THE FOUNDRY");
                            Console.WriteLine("Commands: north, south, take, quit");

                            while (true)
                            {
                                Console.WriteLine();
                                Console.WriteLine(here.Description);
                                if (here.Item != "" && !carryingHammer)
                                {
                                    Console.WriteLine("There is a " + here.Item + " here.");
                                }
                                if (here.Name == "vault" && carryingHammer)
                                {
                                    Console.WriteLine("The hammer fits. The door opens. You win.");
                                    return;
                                }

                                Console.Write("> ");
                                string command = Console.ReadLine();
                                if (command == null || command == "quit") return;

                                if (command == "north" && here.North != "") here = world.Find(here.North);
                                else if (command == "south" && here.South != "") here = world.Find(here.South);
                                else if (command == "take" && here.Item != "")
                                {
                                    carryingHammer = true;
                                    Console.WriteLine("Taken.");
                                }
                                else Console.WriteLine("Nothing happens.");
                            }
                        }
                    }
                    """),
            ],
        });

        // ── Web ────────────────────────────────────────────────────────────
        templates.Add(new ProjectTemplate
        {
            Id = "web-inventory",
            Name = "Inventory API",
            Summary = "Minimal API with Swagger over an in-memory stock ledger.",
            Category = TemplateCategory.Web,
            Domain = TemplateDomain.Business,
            RunHint = "dotnet run  →  http://localhost:5000/swagger",
            RunsOnRustClr = false,
            Files =
            [
                new("{NAME}.csproj", WebProject),
                new("Program.cs", """
                    using System.Collections.Concurrent;

                    var builder = WebApplication.CreateBuilder(args);
                    builder.Services.AddEndpointsApiExplorer();
                    builder.Services.AddSwaggerGen();

                    var app = builder.Build();
                    app.UseSwagger();
                    app.UseSwaggerUI();

                    var stock = new ConcurrentDictionary<string, StockItem>();
                    stock["SKU-100"] = new StockItem("SKU-100", "Runtime license", 24, 2500m);
                    stock["SKU-200"] = new StockItem("SKU-200", "Support hours", 180, 150m);

                    app.MapGet("/items", () => stock.Values.OrderBy(i => i.Sku))
                       .WithSummary("List every stocked item");

                    app.MapGet("/items/{sku}", (string sku) =>
                        stock.TryGetValue(sku, out var item) ? Results.Ok(item) : Results.NotFound())
                       .WithSummary("Fetch one item by SKU");

                    app.MapPost("/items", (StockItem item) =>
                    {
                        stock[item.Sku] = item;
                        return Results.Created($"/items/{item.Sku}", item);
                    }).WithSummary("Add or replace an item");

                    app.MapPost("/items/{sku}/adjust", (string sku, int delta) =>
                    {
                        if (!stock.TryGetValue(sku, out var item)) return Results.NotFound();
                        var adjusted = item with { OnHand = item.OnHand + delta };
                        if (adjusted.OnHand < 0) return Results.BadRequest("Stock cannot go negative.");
                        stock[sku] = adjusted;
                        return Results.Ok(adjusted);
                    }).WithSummary("Move stock in or out");

                    app.Run();

                    record StockItem(string Sku, string Name, int OnHand, decimal UnitPrice);
                    """),
            ],
        });

        templates.Add(new ProjectTemplate
        {
            Id = "web-telemetry",
            Name = "Sensor Telemetry API",
            Summary = "Ingests readings, returns rolling statistics. Swagger included.",
            Category = TemplateCategory.Web,
            Domain = TemplateDomain.Science,
            RunHint = "dotnet run  →  http://localhost:5000/swagger",
            RunsOnRustClr = false,
            Files =
            [
                new("{NAME}.csproj", WebProject),
                new("Program.cs", """
                    using System.Collections.Concurrent;

                    var builder = WebApplication.CreateBuilder(args);
                    builder.Services.AddEndpointsApiExplorer();
                    builder.Services.AddSwaggerGen();

                    var app = builder.Build();
                    app.UseSwagger();
                    app.UseSwaggerUI();

                    var readings = new ConcurrentDictionary<string, List<Reading>>();

                    app.MapPost("/sensors/{id}/readings", (string id, Reading reading) =>
                    {
                        var series = readings.GetOrAdd(id, _ => new List<Reading>());
                        lock (series) { series.Add(reading); }
                        return Results.Accepted();
                    }).WithSummary("Record one reading");

                    app.MapGet("/sensors/{id}/stats", (string id) =>
                    {
                        if (!readings.TryGetValue(id, out var series)) return Results.NotFound();
                        Reading[] snapshot;
                        lock (series) { snapshot = series.ToArray(); }
                        if (snapshot.Length == 0) return Results.NoContent();

                        var values = snapshot.Select(r => r.Value).ToArray();
                        var mean = values.Average();
                        // Sample standard deviation; a single reading has none.
                        var deviation = values.Length < 2
                            ? 0
                            : Math.Sqrt(values.Sum(v => (v - mean) * (v - mean)) / (values.Length - 1));

                        return Results.Ok(new
                        {
                            count = values.Length,
                            min = values.Min(),
                            max = values.Max(),
                            mean,
                            stdDev = deviation,
                        });
                    }).WithSummary("Rolling statistics for one sensor");

                    app.Run();

                    record Reading(DateTimeOffset At, double Value);
                    """),
            ],
        });

        templates.Add(new ProjectTemplate
        {
            Id = "web-courses",
            Name = "Course Catalog API",
            Summary = "Courses, enrolment and capacity checks over Minimal API.",
            Category = TemplateCategory.Web,
            Domain = TemplateDomain.Education,
            RunHint = "dotnet run  →  http://localhost:5000/swagger",
            RunsOnRustClr = false,
            Files =
            [
                new("{NAME}.csproj", WebProject),
                new("Program.cs", """
                    using System.Collections.Concurrent;

                    var builder = WebApplication.CreateBuilder(args);
                    builder.Services.AddEndpointsApiExplorer();
                    builder.Services.AddSwaggerGen();

                    var app = builder.Build();
                    app.UseSwagger();
                    app.UseSwaggerUI();

                    var courses = new ConcurrentDictionary<string, Course>
                    {
                        ["RT101"] = new("RT101", "Runtime Internals", 30, new()),
                        ["RS201"] = new("RS201", "Systems Rust", 24, new()),
                    };

                    app.MapGet("/courses", () => courses.Values).WithSummary("List courses");

                    app.MapPost("/courses/{code}/enrol", (string code, string student) =>
                    {
                        if (!courses.TryGetValue(code, out var course)) return Results.NotFound();
                        lock (course.Enrolled)
                        {
                            if (course.Enrolled.Contains(student))
                                return Results.Conflict("Already enrolled.");
                            if (course.Enrolled.Count >= course.Capacity)
                                return Results.BadRequest("Course is full.");
                            course.Enrolled.Add(student);
                        }
                        return Results.Ok(course);
                    }).WithSummary("Enrol a student, respecting capacity");

                    app.Run();

                    record Course(string Code, string Title, int Capacity, List<string> Enrolled);
                    """),
            ],
        });

        // ── Desktop ────────────────────────────────────────────────────────
        templates.Add(new ProjectTemplate
        {
            Id = "desktop-pos",
            Name = "Point of Sale",
            Summary = "Avalonia desktop till: item grid, running total, cash tendered.",
            Category = TemplateCategory.Desktop,
            Domain = TemplateDomain.Business,
            RunHint = "dotnet run",
            RunsOnRustClr = false,
            Files =
            [
                new("{NAME}.csproj", DesktopProject),
                new("Program.cs", """
                    using Avalonia;

                    namespace {NAMESPACE};

                    internal static class Program
                    {
                        [STAThread]
                        public static void Main(string[] args) =>
                            BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);

                        public static AppBuilder BuildAvaloniaApp() =>
                            AppBuilder.Configure<App>().UsePlatformDetect().LogToTrace();
                    }
                    """),
                new("App.axaml", """
                    <Application xmlns="https://github.com/avaloniaui"
                                 xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
                                 x:Class="{NAMESPACE}.App">
                      <Application.Styles>
                        <FluentTheme />
                      </Application.Styles>
                    </Application>
                    """),
                new("App.axaml.cs", """
                    using Avalonia;
                    using Avalonia.Controls.ApplicationLifetimes;
                    using Avalonia.Markup.Xaml;

                    namespace {NAMESPACE};

                    public partial class App : Application
                    {
                        public override void Initialize() => AvaloniaXamlLoader.Load(this);

                        public override void OnFrameworkInitializationCompleted()
                        {
                            if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
                            {
                                desktop.MainWindow = new MainWindow();
                            }
                            base.OnFrameworkInitializationCompleted();
                        }
                    }
                    """),
                new("MainWindow.axaml", """
                    <Window xmlns="https://github.com/avaloniaui"
                            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
                            x:Class="{NAMESPACE}.MainWindow"
                            Title="{NAME}" Width="720" Height="520">
                      <Grid RowDefinitions="Auto,*,Auto" Margin="16" >
                        <TextBlock Grid.Row="0" Text="Point of Sale" FontSize="22" FontWeight="600" Margin="0,0,0,12" />
                        <ListBox Grid.Row="1" x:Name="Basket" />
                        <StackPanel Grid.Row="2" Orientation="Horizontal" Spacing="8" Margin="0,12,0,0">
                          <TextBox x:Name="ItemName" Watermark="Item" Width="200" />
                          <TextBox x:Name="ItemPrice" Watermark="Price" Width="100" />
                          <Button Content="Add" Click="OnAdd" />
                          <TextBlock x:Name="Total" VerticalAlignment="Center" FontWeight="600" Text="Total 0.00" />
                        </StackPanel>
                      </Grid>
                    </Window>
                    """),
                new("MainWindow.axaml.cs", """
                    using Avalonia.Controls;
                    using Avalonia.Interactivity;
                    using Avalonia.Markup.Xaml;

                    namespace {NAMESPACE};

                    public partial class MainWindow : Window
                    {
                        private decimal _total;

                        public MainWindow()
                        {
                            AvaloniaXamlLoader.Load(this);
                        }

                        private void OnAdd(object? sender, RoutedEventArgs e)
                        {
                            var name = this.FindControl<TextBox>("ItemName");
                            var price = this.FindControl<TextBox>("ItemPrice");
                            var basket = this.FindControl<ListBox>("Basket");
                            var total = this.FindControl<TextBlock>("Total");
                            if (name is null || price is null || basket is null || total is null) return;

                            if (!decimal.TryParse(price.Text, out var amount)) return;
                            basket.Items.Add($"{name.Text}  {amount:0.00}");
                            _total += amount;
                            total.Text = $"Total {_total:0.00}";
                            name.Text = "";
                            price.Text = "";
                        }
                    }
                    """),
            ],
        });

        templates.Add(new ProjectTemplate
        {
            Id = "desktop-flashcards",
            Name = "Flashcards",
            Summary = "Avalonia study app with a spaced-repetition schedule.",
            Category = TemplateCategory.Desktop,
            Domain = TemplateDomain.Education,
            RunHint = "dotnet run",
            RunsOnRustClr = false,
            Files =
            [
                new("{NAME}.csproj", DesktopProject),
                new("Program.cs", """
                    using Avalonia;

                    namespace {NAMESPACE};

                    internal static class Program
                    {
                        [STAThread]
                        public static void Main(string[] args) =>
                            AppBuilder.Configure<App>().UsePlatformDetect().LogToTrace()
                                .StartWithClassicDesktopLifetime(args);
                    }
                    """),
                new("App.axaml", """
                    <Application xmlns="https://github.com/avaloniaui"
                                 xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
                                 x:Class="{NAMESPACE}.App">
                      <Application.Styles>
                        <FluentTheme />
                      </Application.Styles>
                    </Application>
                    """),
                new("App.axaml.cs", """
                    using Avalonia;
                    using Avalonia.Controls.ApplicationLifetimes;
                    using Avalonia.Markup.Xaml;

                    namespace {NAMESPACE};

                    public partial class App : Application
                    {
                        public override void Initialize() => AvaloniaXamlLoader.Load(this);

                        public override void OnFrameworkInitializationCompleted()
                        {
                            if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
                                desktop.MainWindow = new MainWindow();
                            base.OnFrameworkInitializationCompleted();
                        }
                    }
                    """),
                new("Scheduler.cs", """
                    namespace {NAMESPACE};

                    public sealed class Card
                    {
                        public string Front = "";
                        public string Back = "";
                        /// <summary>Days until this card is due again.</summary>
                        public int Interval = 1;
                        public DateTime Due = DateTime.Today;
                    }

                    public static class Scheduler
                    {
                        /// <summary>
                        /// A cut-down SM-2: a correct answer roughly doubles the interval,
                        /// a wrong one sends the card back to tomorrow.
                        /// </summary>
                        public static void Review(Card card, bool correct)
                        {
                            card.Interval = correct ? Math.Min(card.Interval * 2, 180) : 1;
                            card.Due = DateTime.Today.AddDays(card.Interval);
                        }
                    }
                    """),
                new("MainWindow.axaml", """
                    <Window xmlns="https://github.com/avaloniaui"
                            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
                            x:Class="{NAMESPACE}.MainWindow"
                            Title="{NAME}" Width="560" Height="360">
                      <StackPanel Margin="24" Spacing="16">
                        <TextBlock x:Name="Prompt" FontSize="24" TextWrapping="Wrap" />
                        <TextBlock x:Name="Answer" FontSize="18" Opacity="0.7" IsVisible="False" TextWrapping="Wrap" />
                        <StackPanel Orientation="Horizontal" Spacing="8">
                          <Button Content="Show answer" Click="OnReveal" />
                          <Button Content="Got it" Click="OnCorrect" />
                          <Button Content="Missed it" Click="OnWrong" />
                        </StackPanel>
                      </StackPanel>
                    </Window>
                    """),
                new("MainWindow.axaml.cs", """
                    using Avalonia.Controls;
                    using Avalonia.Interactivity;
                    using Avalonia.Markup.Xaml;

                    namespace {NAMESPACE};

                    public partial class MainWindow : Window
                    {
                        private readonly List<Card> _deck =
                        [
                            new() { Front = "What runs IL?", Back = "The runtime's execution engine." },
                            new() { Front = "What replaces free() in Rust?", Back = "Ownership: values drop at scope exit." },
                        ];
                        private int _index;

                        public MainWindow()
                        {
                            AvaloniaXamlLoader.Load(this);
                            Show(_index);
                        }

                        private void Show(int index)
                        {
                            var prompt = this.FindControl<TextBlock>("Prompt");
                            var answer = this.FindControl<TextBlock>("Answer");
                            if (prompt is null || answer is null) return;
                            prompt.Text = _deck[index % _deck.Count].Front;
                            answer.Text = _deck[index % _deck.Count].Back;
                            answer.IsVisible = false;
                        }

                        private void OnReveal(object? sender, RoutedEventArgs e)
                        {
                            var answer = this.FindControl<TextBlock>("Answer");
                            if (answer is not null) answer.IsVisible = true;
                        }

                        private void OnCorrect(object? sender, RoutedEventArgs e) => Advance(true);
                        private void OnWrong(object? sender, RoutedEventArgs e) => Advance(false);

                        private void Advance(bool correct)
                        {
                            Scheduler.Review(_deck[_index % _deck.Count], correct);
                            _index++;
                            Show(_index);
                        }
                    }
                    """),
            ],
        });

        // ── Mobile ─────────────────────────────────────────────────────────
        templates.Add(new ProjectTemplate
        {
            Id = "mobile-fieldlog",
            Name = "Field Logger",
            Summary =
                "Touch-first Avalonia layout for recording site observations. "
                + "Runs on desktop as-is; add the Avalonia Android/iOS heads to deploy.",
            Category = TemplateCategory.Mobile,
            Domain = TemplateDomain.Business,
            RunHint = "dotnet run",
            RunsOnRustClr = false,
            Files =
            [
                new("{NAME}.csproj", DesktopProject),
                new("Program.cs", """
                    using Avalonia;

                    namespace {NAMESPACE};

                    internal static class Program
                    {
                        [STAThread]
                        public static void Main(string[] args) =>
                            AppBuilder.Configure<App>().UsePlatformDetect().LogToTrace()
                                .StartWithClassicDesktopLifetime(args);
                    }
                    """),
                new("App.axaml", """
                    <Application xmlns="https://github.com/avaloniaui"
                                 xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
                                 x:Class="{NAMESPACE}.App">
                      <Application.Styles>
                        <FluentTheme />
                      </Application.Styles>
                    </Application>
                    """),
                new("App.axaml.cs", """
                    using Avalonia;
                    using Avalonia.Controls.ApplicationLifetimes;
                    using Avalonia.Markup.Xaml;

                    namespace {NAMESPACE};

                    public partial class App : Application
                    {
                        public override void Initialize() => AvaloniaXamlLoader.Load(this);

                        public override void OnFrameworkInitializationCompleted()
                        {
                            if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
                                desktop.MainWindow = new MainWindow();
                            base.OnFrameworkInitializationCompleted();
                        }
                    }
                    """),
                new("MainWindow.axaml", """
                    <Window xmlns="https://github.com/avaloniaui"
                            xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml"
                            x:Class="{NAMESPACE}.MainWindow"
                            Title="{NAME}" Width="400" Height="720">
                      <!-- Sized to a phone viewport; every touch target is at least 48px. -->
                      <Grid RowDefinitions="Auto,*,Auto" Margin="16">
                        <TextBlock Grid.Row="0" Text="Field Log" FontSize="24" FontWeight="600" Margin="0,0,0,16" />
                        <ListBox Grid.Row="1" x:Name="Entries" />
                        <StackPanel Grid.Row="2" Spacing="12" Margin="0,16,0,0">
                          <TextBox x:Name="Note" Watermark="What did you observe?" MinHeight="48" AcceptsReturn="True" />
                          <Button Content="Record" Click="OnRecord" HorizontalAlignment="Stretch" MinHeight="48" />
                        </StackPanel>
                      </Grid>
                    </Window>
                    """),
                new("MainWindow.axaml.cs", """
                    using Avalonia.Controls;
                    using Avalonia.Interactivity;
                    using Avalonia.Markup.Xaml;

                    namespace {NAMESPACE};

                    public partial class MainWindow : Window
                    {
                        public MainWindow() => AvaloniaXamlLoader.Load(this);

                        private void OnRecord(object? sender, RoutedEventArgs e)
                        {
                            var note = this.FindControl<TextBox>("Note");
                            var entries = this.FindControl<ListBox>("Entries");
                            if (note is null || entries is null) return;
                            if (string.IsNullOrWhiteSpace(note.Text)) return;

                            entries.Items.Insert(0, $"{DateTime.Now:HH:mm}  {note.Text}");
                            note.Text = "";
                        }
                    }
                    """),
            ],
        });

        // ── IoT ────────────────────────────────────────────────────────────
        templates.Add(new ProjectTemplate
        {
            Id = "iot-gateway",
            Name = "Sensor Gateway",
            Summary =
                "Polls sensors, applies calibration and batches readings. "
                + "Written for the IL subset RustCLR runs on microcontrollers.",
            Category = TemplateCategory.IoT,
            Domain = TemplateDomain.Science,
            RunHint = "rustnet run bin/Release/net10.0/{NAME}.dll",
            Files =
            [
                new("{NAME}.csproj", ConsoleProject),
                new("Gateway.cs", """
                    namespace {NAMESPACE};

                    public sealed class Sensor
                    {
                        public string Id = "";
                        /// <summary>Raw ADC counts are converted with offset and scale.</summary>
                        public double Offset;
                        public double Scale = 1.0;

                        public double Calibrate(int raw) { return (raw + Offset) * Scale; }
                    }

                    public sealed class Gateway
                    {
                        public Sensor[] Sensors = new Sensor[0];
                        private double[] _batch = new double[0];
                        private int _count;

                        public void Begin(int batchSize) { _batch = new double[batchSize]; _count = 0; }

                        /// <summary>Returns true when the batch is full and ready to publish.</summary>
                        public bool Record(double value)
                        {
                            if (_count >= _batch.Length) return true;
                            _batch[_count] = value;
                            _count++;
                            return _count == _batch.Length;
                        }

                        public double Mean()
                        {
                            if (_count == 0) return 0;
                            double total = 0;
                            for (int i = 0; i < _count; i++) total += _batch[i];
                            return total / _count;
                        }

                        public void Reset() { _count = 0; }
                    }
                    """),
                new("Program.cs", """
                    namespace {NAMESPACE};

                    public static class Program
                    {
                        public static void Main()
                        {
                            var gateway = new Gateway();
                            gateway.Sensors = new Sensor[2];
                            gateway.Sensors[0] = new Sensor { Id = "temp-1", Offset = -12, Scale = 0.0625 };
                            gateway.Sensors[1] = new Sensor { Id = "hum-1", Offset = 0, Scale = 0.05 };
                            gateway.Begin(8);

                            // Stand-in for an ADC read loop.
                            int reading = 500;
                            for (int tick = 0; tick < 20; tick++)
                            {
                                double calibrated = gateway.Sensors[0].Calibrate(reading + tick * 3);
                                if (gateway.Record(calibrated))
                                {
                                    Console.WriteLine("publish temp-1 mean=" + gateway.Mean());
                                    gateway.Reset();
                                }
                            }
                        }
                    }
                    """),
            ],
        });

        templates.Add(new ProjectTemplate
        {
            Id = "iot-controller",
            Name = "Thermostat Controller",
            Summary = "A hysteresis control loop with duty-cycle limits, for embedded targets.",
            Category = TemplateCategory.IoT,
            Domain = TemplateDomain.Business,
            RunHint = "rustnet run bin/Release/net10.0/{NAME}.dll",
            Files =
            [
                new("{NAME}.csproj", ConsoleProject),
                new("Thermostat.cs", """
                    namespace {NAMESPACE};

                    public sealed class Thermostat
                    {
                        public double Target = 21.0;
                        /// <summary>Dead band, so the relay does not chatter around the setpoint.</summary>
                        public double Hysteresis = 0.5;
                        public bool HeaterOn;

                        /// <summary>Minimum ticks the heater must stay in one state.</summary>
                        public int MinimumDwell = 3;
                        private int _dwell;

                        public bool Step(double temperature)
                        {
                            _dwell++;
                            if (_dwell < MinimumDwell) return HeaterOn;

                            if (HeaterOn && temperature > Target + Hysteresis)
                            {
                                HeaterOn = false;
                                _dwell = 0;
                            }
                            else if (!HeaterOn && temperature < Target - Hysteresis)
                            {
                                HeaterOn = true;
                                _dwell = 0;
                            }
                            return HeaterOn;
                        }
                    }
                    """),
                new("Program.cs", """
                    namespace {NAMESPACE};

                    public static class Program
                    {
                        public static void Main()
                        {
                            var thermostat = new Thermostat();
                            double temperature = 18.0;

                            for (int tick = 0; tick < 30; tick++)
                            {
                                bool heating = thermostat.Step(temperature);
                                temperature += heating ? 0.4 : -0.25;
                                Console.WriteLine(tick + "  " + temperature + "  heater=" + heating);
                            }
                        }
                    }
                    """),
            ],
        });

        // ── Library ────────────────────────────────────────────────────────
        templates.Add(new ProjectTemplate
        {
            Id = "library-core",
            Name = "Class Library",
            Summary = "A reusable library plus a matching test entry point.",
            Category = TemplateCategory.Library,
            Domain = TemplateDomain.Runtime,
            RunHint = "dotnet build",
            Files =
            [
                new("{NAME}.csproj", LibraryProject),
                new("Calculator.cs", """
                    namespace {NAMESPACE};

                    /// <summary>A worked example of the shape a library takes.</summary>
                    public static class Calculator
                    {
                        public static int Add(int a, int b) { return a + b; }

                        public static int Factorial(int n)
                        {
                            if (n < 0) throw new ArgumentOutOfRangeException(nameof(n));
                            int acc = 1;
                            for (int i = 2; i <= n; i++) acc *= i;
                            return acc;
                        }
                    }
                    """),
            ],
        });

        return templates;
    }
}
