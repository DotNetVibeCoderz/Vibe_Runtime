using System;
using System.Threading;

namespace RustNet.Timers;

/// <summary>
/// Periodic callback timer in the System.Timers spirit, built on the
/// runtime's cooperative green threads. The callback runs on a dedicated
/// thread; Stop() takes effect at the next tick boundary.
/// </summary>
public class Timer
{
    private readonly int _intervalMs;
    private readonly Action _callback;
    private bool _running;

    public Timer(int intervalMs, Action callback)
    {
        _intervalMs = intervalMs;
        _callback = callback;
        _running = false;
    }

    public bool IsRunning => _running;

    public void Start()
    {
        if (_running)
        {
            return;
        }
        _running = true;
        Thread t = new Thread(Loop);
        t.Start();
    }

    public void Stop()
    {
        _running = false;
    }

    private void Loop()
    {
        while (_running)
        {
            RustNet.Threading.Sleep.Ms(_intervalMs);
            if (_running)
            {
                _callback();
            }
        }
    }
}
