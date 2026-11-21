// SemaphoreSlim and ThreadLocal<T>, over Monitor and TaskCompletionSource.

using System.Collections.Generic;
using System.Threading.Tasks;

namespace System.Threading
{
    public class SemaphoreFullException : SystemException
    {
        public SemaphoreFullException() : base("Adding the specified count to the semaphore would cause it to exceed its maximum count.") { }
        public SemaphoreFullException(string message) : base(message) { }
        public SemaphoreFullException(string message, Exception innerException) : base(message, innerException) { }
    }

    /// <summary>
    /// A counting semaphore that can be waited on synchronously or with await.
    /// </summary>
    /// <remarks>
    /// Synchronous waiters block on the monitor; asynchronous ones queue a
    /// completion source, first come first served. A release hands its counts
    /// to queued asynchronous waiters before anything else can take them, so
    /// an awaiting caller is not starved by a loop of synchronous Wait(0)s.
    /// </remarks>
    public class SemaphoreSlim : IDisposable
    {
        private readonly object _sync = new object();
        private readonly int _max;
        private int _count;
        private int _syncWaiters;
        private readonly List<Waiter> _asyncWaiters = new List<Waiter>();

        /// <summary>An asynchronous waiter, and whether it is still queued.</summary>
        private sealed class Waiter
        {
            public readonly TaskCompletionSource<bool> Source =
                new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
            public bool Queued = true;
        }

        public SemaphoreSlim(int initialCount) : this(initialCount, int.MaxValue) { }

        public SemaphoreSlim(int initialCount, int maxCount)
        {
            if (initialCount < 0 || initialCount > maxCount)
                throw new ArgumentOutOfRangeException(nameof(initialCount));
            if (maxCount <= 0)
                throw new ArgumentOutOfRangeException(nameof(maxCount));
            _count = initialCount;
            _max = maxCount;
        }

        public int CurrentCount { get { lock (_sync) return _count; } }

        public void Wait() => Wait(Timeout.Infinite, default);

        public void Wait(CancellationToken cancellationToken) => Wait(Timeout.Infinite, cancellationToken);

        public bool Wait(int millisecondsTimeout) => Wait(millisecondsTimeout, default);

        public bool Wait(TimeSpan timeout) => Wait(ToMillis(timeout), default);

        public bool Wait(TimeSpan timeout, CancellationToken cancellationToken) => Wait(ToMillis(timeout), cancellationToken);

        public bool Wait(int millisecondsTimeout, CancellationToken cancellationToken)
        {
            if (millisecondsTimeout < -1) throw new ArgumentOutOfRangeException(nameof(millisecondsTimeout));
            cancellationToken.ThrowIfCancellationRequested();
            long deadline = millisecondsTimeout == Timeout.Infinite ? long.MaxValue : Environment.TickCount64 + millisecondsTimeout;
            lock (_sync)
            {
                _syncWaiters++;
                try
                {
                    while (_count == 0)
                    {
                        int slice = cancellationToken.CanBeCanceled ? 50 : Timeout.Infinite;
                        if (deadline != long.MaxValue)
                        {
                            long left = deadline - Environment.TickCount64;
                            if (left <= 0) return false;
                            if (slice == Timeout.Infinite || left < slice) slice = (int)left;
                        }
                        Monitor.Wait(_sync, slice);
                        cancellationToken.ThrowIfCancellationRequested();
                    }
                    _count--;
                    return true;
                }
                finally
                {
                    _syncWaiters--;
                }
            }
        }

        public Task WaitAsync() => WaitAsync(Timeout.Infinite, default);

        public Task WaitAsync(CancellationToken cancellationToken) => WaitAsync(Timeout.Infinite, cancellationToken);

        public Task<bool> WaitAsync(int millisecondsTimeout) => WaitAsync(millisecondsTimeout, default);

        public Task<bool> WaitAsync(TimeSpan timeout) => WaitAsync(ToMillis(timeout), default);

        public Task<bool> WaitAsync(TimeSpan timeout, CancellationToken cancellationToken) => WaitAsync(ToMillis(timeout), cancellationToken);

        public Task<bool> WaitAsync(int millisecondsTimeout, CancellationToken cancellationToken)
        {
            if (millisecondsTimeout < -1) throw new ArgumentOutOfRangeException(nameof(millisecondsTimeout));
            if (cancellationToken.IsCancellationRequested)
            {
                var canceled = new TaskCompletionSource<bool>();
                canceled.TrySetCanceled(cancellationToken);
                return canceled.Task;
            }

            Waiter waiter;
            lock (_sync)
            {
                if (_count > 0)
                {
                    _count--;
                    return Task.FromResult(true);
                }
                if (millisecondsTimeout == 0)
                    return Task.FromResult(false);
                waiter = new Waiter();
                _asyncWaiters.Add(waiter);
            }

            if (millisecondsTimeout != Timeout.Infinite)
            {
                Task.Delay(millisecondsTimeout).ContinueWith(_ =>
                {
                    if (Withdraw(waiter)) waiter.Source.TrySetResult(false);
                });
            }
            if (cancellationToken.CanBeCanceled)
            {
                var registration = cancellationToken.Register(() =>
                {
                    if (Withdraw(waiter)) waiter.Source.TrySetCanceled(cancellationToken);
                });
                waiter.Source.Task.ContinueWith(_ => registration.Dispose());
            }
            return waiter.Source.Task;
        }

        /// <summary>Removes a waiter that has not been granted a count.</summary>
        private bool Withdraw(Waiter waiter)
        {
            lock (_sync)
            {
                if (!waiter.Queued) return false;
                waiter.Queued = false;
                _asyncWaiters.Remove(waiter);
                return true;
            }
        }

        public int Release() => Release(1);

        public int Release(int releaseCount)
        {
            if (releaseCount < 1) throw new ArgumentOutOfRangeException(nameof(releaseCount));
            var granted = new List<TaskCompletionSource<bool>>();
            int previous;
            lock (_sync)
            {
                previous = _count;
                if (_max - _count < releaseCount)
                    throw new SemaphoreFullException();
                int remaining = releaseCount;
                while (remaining > 0 && _asyncWaiters.Count > 0)
                {
                    var first = _asyncWaiters[0];
                    _asyncWaiters.RemoveAt(0);
                    first.Queued = false;
                    granted.Add(first.Source);
                    remaining--;
                }
                _count += remaining;
                if (remaining > 0 && _syncWaiters > 0) Monitor.PulseAll(_sync);
            }
            // Completed outside the lock: an awaiting caller's continuation
            // must not run while this holds it.
            foreach (var waiter in granted) waiter.TrySetResult(true);
            return previous;
        }

        public void Dispose() { }

        private static int ToMillis(TimeSpan timeout)
        {
            long ms = (long)timeout.TotalMilliseconds;
            if (ms == -1) return Timeout.Infinite;
            if (ms < -1 || ms > int.MaxValue) throw new ArgumentOutOfRangeException(nameof(timeout));
            return (int)ms;
        }
    }

    /// <summary>One value per thread.</summary>
    /// <remarks>
    /// Kept per managed thread id. A thread that exits leaves its value until
    /// the ThreadLocal is disposed, which docs/limitations.md states.
    /// </remarks>
    public class ThreadLocal<T> : IDisposable
    {
        private readonly object _sync = new object();
        private readonly Func<T> _factory;
        private readonly bool _trackAllValues;
        private Dictionary<int, T> _values = new Dictionary<int, T>();

        public ThreadLocal() : this(null, false) { }

        public ThreadLocal(bool trackAllValues) : this(null, trackAllValues) { }

        public ThreadLocal(Func<T> valueFactory) : this(valueFactory, false) { }

        public ThreadLocal(Func<T> valueFactory, bool trackAllValues)
        {
            _factory = valueFactory;
            _trackAllValues = trackAllValues;
        }

        private Dictionary<int, T> Values_
        {
            get
            {
                var values = _values;
                if (values == null) throw new ObjectDisposedException(nameof(ThreadLocal<T>));
                return values;
            }
        }

        public T Value
        {
            get
            {
                int id = Environment.CurrentManagedThreadId;
                lock (_sync)
                {
                    if (Values_.TryGetValue(id, out var existing)) return existing;
                }
                // The factory runs outside the lock: it is user code, and may
                // itself read this or another ThreadLocal.
                var made = _factory == null ? default : _factory();
                lock (_sync)
                {
                    Values_[id] = made;
                }
                return made;
            }
            set
            {
                int id = Environment.CurrentManagedThreadId;
                lock (_sync) Values_[id] = value;
            }
        }

        public bool IsValueCreated
        {
            get
            {
                int id = Environment.CurrentManagedThreadId;
                lock (_sync) return Values_.ContainsKey(id);
            }
        }

        public IList<T> Values
        {
            get
            {
                if (!_trackAllValues)
                    throw new InvalidOperationException("The ThreadLocal object is not tracking values. To use the Values property, use a ThreadLocal constructor that accepts the trackAllValues parameter and set the parameter to true.");
                lock (_sync) return new List<T>(Values_.Values);
            }
        }

        public override string ToString() => Value?.ToString();

        public void Dispose()
        {
            lock (_sync) _values = null;
        }
    }
}
