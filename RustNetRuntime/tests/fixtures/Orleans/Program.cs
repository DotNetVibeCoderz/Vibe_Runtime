using System;
using System.Threading.Tasks;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Hosting;
using Microsoft.Extensions.Logging;
using Orleans;
using Orleans.Hosting;

namespace OrleansSample;

// Milestone 8.6: an unmodified Orleans program with an in-process silo.
// Logging is turned down so the output is deterministic and diffable.
public interface ICounterGrain : IGrainWithStringKey
{
    Task<int> Increment();
}

public sealed class CounterGrain : Grain, ICounterGrain
{
    private int _value;
    public Task<int> Increment() => Task.FromResult(++_value);
}

public static class Program
{
    public static async Task<int> Main()
    {
        using var host = Host.CreateDefaultBuilder()
            .UseOrleans(silo => silo.UseLocalhostClustering())
            .ConfigureLogging(logging => logging.SetMinimumLevel(LogLevel.Error))
            .Build();
        await host.StartAsync();
        var grains = host.Services.GetRequiredService<IGrainFactory>();
        var counter = grains.GetGrain<ICounterGrain>("c");
        for (int i = 0; i < 3; i++) Console.WriteLine("count=" + await counter.Increment());
        await host.StopAsync();
        Console.WriteLine("stopped");
        return 0;
    }
}
