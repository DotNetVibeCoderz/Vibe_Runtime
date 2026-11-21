using System;
using System.Threading.Tasks;
using Akka.Actor;

namespace AkkaSample;

// Milestone 8.6: an unmodified Akka.NET program. Deterministic output only,
// so the two runtimes can be diffed byte for byte.
public sealed class Greet
{
    public Greet(string who) { Who = who; }
    public string Who { get; }
}

public sealed class Greeter : ReceiveActor
{
    private int _count;

    public Greeter()
    {
        Receive<Greet>(g =>
        {
            _count++;
            Sender.Tell("Hello, " + g.Who + " #" + _count);
        });
        Receive<string>(s => s == "count", _ => Sender.Tell(_count));
    }
}

public static class Program
{
    public static async Task<int> Main()
    {
        var system = ActorSystem.Create("sample", "akka { loglevel = WARNING, stdout-loglevel = WARNING }");
        var greeter = system.ActorOf(Props.Create(() => new Greeter()), "greeter");
        for (int i = 0; i < 3; i++)
        {
            string reply = await greeter.Ask<string>(new Greet("world"), TimeSpan.FromSeconds(10));
            Console.WriteLine(reply);
        }
        int count = await greeter.Ask<int>("count", TimeSpan.FromSeconds(10));
        Console.WriteLine("count=" + count);
        await system.Terminate();
        Console.WriteLine("terminated");
        return 0;
    }
}
