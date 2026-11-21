// System.Threading.Channels, over one lock per channel.
//
// Waiters are completion sources made with RunContinuationsAsynchronously, so
// a reader's continuation never runs inside the writer's call - which is what
// .NET does by default (AllowSynchronousContinuations = false), and what stops
// a continuation from running under a lock the writer happens to hold.

using System.Collections.Generic;
using System.Runtime.CompilerServices;
using System.Threading.Tasks;

namespace System.Threading.Channels
{
    public enum BoundedChannelFullMode
    {
        Wait = 0,
        DropNewest = 1,
        DropOldest = 2,
        DropWrite = 3,
    }

    public abstract class ChannelOptions
    {
        public bool SingleWriter { get; set; }
        public bool SingleReader { get; set; }
        public bool AllowSynchronousContinuations { get; set; }
    }

    public sealed class UnboundedChannelOptions : ChannelOptions
    {
    }

    public sealed class BoundedChannelOptions : ChannelOptions
    {
        private int _capacity;

        public BoundedChannelOptions(int capacity)
        {
            if (capacity < 1) throw new ArgumentOutOfRangeException(nameof(capacity));
            _capacity = capacity;
        }

        public int Capacity
        {
            get => _capacity;
            set
            {
                if (value < 1) throw new ArgumentOutOfRangeException(nameof(value));
                _capacity = value;
            }
        }

        public BoundedChannelFullMode FullMode { get; set; } = BoundedChannelFullMode.Wait;
    }

    public class ChannelClosedException : InvalidOperationException
    {
        public ChannelClosedException() : base("The channel has been closed.") { }
        public ChannelClosedException(string message) : base(message) { }
        public ChannelClosedException(Exception innerException) : base("The channel has been closed.", innerException) { }
        public ChannelClosedException(string message, Exception innerException) : base(message, innerException) { }
    }

    public abstract class ChannelReader<T>
    {
        public virtual Task Completion => new TaskCompletionSource<bool>().Task;
        public virtual bool CanCount => false;
        public virtual bool CanPeek => false;
        public virtual int Count => throw new NotSupportedException();

        public abstract bool TryRead(out T item);

        public virtual bool TryPeek(out T item)
        {
            item = default;
            return false;
        }

        public abstract ValueTask<bool> WaitToReadAsync(CancellationToken cancellationToken = default);

        public virtual ValueTask<T> ReadAsync(CancellationToken cancellationToken = default)
        {
            if (cancellationToken.IsCancellationRequested)
                return new ValueTask<T>(Canceled<T>(cancellationToken));
            if (TryRead(out var item))
                return new ValueTask<T>(item);
            return new ValueTask<T>(ReadSlowly(cancellationToken));
        }

        private async Task<T> ReadSlowly(CancellationToken cancellationToken)
        {
            while (await WaitToReadAsync(cancellationToken).ConfigureAwait(false))
            {
                if (TryRead(out var item)) return item;
            }
            throw new ChannelClosedException();
        }

        public virtual async IAsyncEnumerable<T> ReadAllAsync([EnumeratorCancellation] CancellationToken cancellationToken = default)
        {
            while (await WaitToReadAsync(cancellationToken).ConfigureAwait(false))
            {
                while (TryRead(out var item))
                    yield return item;
            }
        }

        internal static Task<TResult> Canceled<TResult>(CancellationToken token)
        {
            var source = new TaskCompletionSource<TResult>();
            source.TrySetCanceled(token);
            return source.Task;
        }

        internal static Task<TResult> Faulted<TResult>(Exception exception)
        {
            var source = new TaskCompletionSource<TResult>();
            source.TrySetException(exception);
            return source.Task;
        }
    }

    public abstract class ChannelWriter<T>
    {
        public virtual bool TryComplete(Exception error = null) => false;

        public abstract bool TryWrite(T item);

        public abstract ValueTask<bool> WaitToWriteAsync(CancellationToken cancellationToken = default);

        public virtual ValueTask WriteAsync(T item, CancellationToken cancellationToken = default)
        {
            if (cancellationToken.IsCancellationRequested)
                return new ValueTask(ChannelReader<T>.Canceled<bool>(cancellationToken));
            if (TryWrite(item))
                return default;
            return new ValueTask(WriteSlowly(item, cancellationToken));
        }

        private async Task WriteSlowly(T item, CancellationToken cancellationToken)
        {
            while (await WaitToWriteAsync(cancellationToken).ConfigureAwait(false))
            {
                if (TryWrite(item)) return;
            }
            throw new ChannelClosedException();
        }

        public void Complete(Exception error = null)
        {
            if (!TryComplete(error))
                throw new ChannelClosedException();
        }
    }

    public abstract class Channel<TWrite, TRead>
    {
        public ChannelReader<TRead> Reader { get; protected set; }
        public ChannelWriter<TWrite> Writer { get; protected set; }

        public static implicit operator ChannelReader<TRead>(Channel<TWrite, TRead> channel) => channel.Reader;
        public static implicit operator ChannelWriter<TWrite>(Channel<TWrite, TRead> channel) => channel.Writer;
    }

    public abstract class Channel<T> : Channel<T, T>
    {
    }

    public static class Channel
    {
        public static Channel<T> CreateUnbounded<T>() => new BufferedChannel<T>(-1, BoundedChannelFullMode.Wait, null);

        public static Channel<T> CreateUnbounded<T>(UnboundedChannelOptions options)
        {
            if (options == null) throw new ArgumentNullException(nameof(options));
            return new BufferedChannel<T>(-1, BoundedChannelFullMode.Wait, null);
        }

        public static Channel<T> CreateBounded<T>(int capacity)
        {
            if (capacity < 1) throw new ArgumentOutOfRangeException(nameof(capacity));
            return new BufferedChannel<T>(capacity, BoundedChannelFullMode.Wait, null);
        }

        public static Channel<T> CreateBounded<T>(BoundedChannelOptions options) => CreateBounded<T>(options, null);

        public static Channel<T> CreateBounded<T>(BoundedChannelOptions options, Action<T> itemDropped)
        {
            if (options == null) throw new ArgumentNullException(nameof(options));
            return new BufferedChannel<T>(options.Capacity, options.FullMode, itemDropped);
        }
    }

    /// <summary>
    /// Both channel kinds: unbounded is a capacity of -1. One lock guards the
    /// buffer and every waiter list; waiters are completed after it is released.
    /// </summary>
    internal sealed class BufferedChannel<T> : Channel<T>
    {
        private readonly object _sync = new object();
        private readonly int _capacity;
        private readonly BoundedChannelFullMode _mode;
        private readonly Action<T> _itemDropped;

        // The buffer: a ring, because DropOldest takes from the front and
        // DropNewest from the back.
        private T[] _ring = new T[4];
        private int _head;
        private int _count;

        private bool _doneWriting;
        private Exception _error;
        private readonly TaskCompletionSource<bool> _completion =
            new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);

        private readonly List<ReadWaiter> _readers = new List<ReadWaiter>();
        private readonly List<TaskCompletionSource<bool>> _waitingToRead = new List<TaskCompletionSource<bool>>();
        private readonly List<WriteWaiter> _writers = new List<WriteWaiter>();
        private readonly List<TaskCompletionSource<bool>> _waitingToWrite = new List<TaskCompletionSource<bool>>();

        private sealed class ReadWaiter
        {
            public readonly TaskCompletionSource<T> Source = new TaskCompletionSource<T>(TaskCreationOptions.RunContinuationsAsynchronously);
        }

        private sealed class WriteWaiter
        {
            public T Item;
            public readonly TaskCompletionSource<bool> Source = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        }

        internal BufferedChannel(int capacity, BoundedChannelFullMode mode, Action<T> itemDropped)
        {
            _capacity = capacity;
            _mode = mode;
            _itemDropped = itemDropped;
            Reader = new BufferedReader(this);
            Writer = new BufferedWriter(this);
        }

        private bool Bounded => _capacity > 0;
        private bool HasRoom => !Bounded || _count < _capacity;

        // ── the ring ────────────────────────────────────────────────────────

        private void PushBack(T item)
        {
            if (_count == _ring.Length)
            {
                var grown = new T[_ring.Length * 2];
                for (int i = 0; i < _count; i++) grown[i] = _ring[(_head + i) % _ring.Length];
                _ring = grown;
                _head = 0;
            }
            _ring[(_head + _count) % _ring.Length] = item;
            _count++;
        }

        private T PopFront()
        {
            var item = _ring[_head];
            _ring[_head] = default;
            _head = (_head + 1) % _ring.Length;
            _count--;
            return item;
        }

        private T PopBack()
        {
            int last = (_head + _count - 1) % _ring.Length;
            var item = _ring[last];
            _ring[last] = default;
            _count--;
            return item;
        }

        // ── completion of waiters, outside the lock ─────────────────────────

        private sealed class Handoff
        {
            public TaskCompletionSource<T> Source;
            public T Item;
        }

        private sealed class Wakeups
        {
            public List<TaskCompletionSource<bool>> True;
            public List<TaskCompletionSource<bool>> False;
            public List<Handoff> Items;
            public List<TaskCompletionSource<bool>> Written;
            public bool Complete;
            public bool HasDropped;
            public T Dropped;
        }

        private static List<TaskCompletionSource<bool>> TakeAll(List<TaskCompletionSource<bool>> list)
        {
            if (list.Count == 0) return null;
            var taken = new List<TaskCompletionSource<bool>>(list);
            list.Clear();
            return taken;
        }

        private void Run(Wakeups w)
        {
            if (w == null) return;
            if (w.Items != null) foreach (var handoff in w.Items) handoff.Source.TrySetResult(handoff.Item);
            if (w.Written != null) foreach (var source in w.Written) source.TrySetResult(true);
            if (w.True != null) foreach (var source in w.True) source.TrySetResult(true);
            if (w.False != null)
            {
                foreach (var source in w.False)
                {
                    if (_error != null) source.TrySetException(_error);
                    else source.TrySetResult(false);
                }
            }
            if (w.Complete) CompleteCompletion();
            if (w.HasDropped) _itemDropped?.Invoke(w.Dropped);
        }

        private void CompleteCompletion()
        {
            var error = _error;
            if (error == null) _completion.TrySetResult(true);
            else if (error is OperationCanceledException) _completion.TrySetCanceled();
            else _completion.TrySetException(error);
        }

        private Exception Closed() => _error == null ? new ChannelClosedException() : new ChannelClosedException(_error);

        // ── writing ─────────────────────────────────────────────────────────

        /// <summary>Under the lock: hands an item on, buffers it, or drops one.</summary>
        private bool WriteLocked(T item, Wakeups w)
        {
            // A reader already waiting takes the item directly.
            while (_readers.Count > 0 && _count == 0)
            {
                var reader = _readers[0];
                _readers.RemoveAt(0);
                if (reader.Source.Task.IsCompleted) continue; // cancelled meanwhile
                (w.Items ??= new List<Handoff>()).Add(new Handoff { Source = reader.Source, Item = item });
                return true;
            }
            if (HasRoom)
            {
                PushBack(item);
                w.True = TakeAll(_waitingToRead);
                return true;
            }
            switch (_mode)
            {
                case BoundedChannelFullMode.DropWrite:
                    w.HasDropped = true;
                    w.Dropped = item;
                    return true;
                case BoundedChannelFullMode.DropNewest:
                    w.HasDropped = true;
                    w.Dropped = PopBack();
                    PushBack(item);
                    return true;
                case BoundedChannelFullMode.DropOldest:
                    w.HasDropped = true;
                    w.Dropped = PopFront();
                    PushBack(item);
                    return true;
                default:
                    return false;
            }
        }

        private bool TryWrite(T item)
        {
            var w = new Wakeups();
            bool written;
            lock (_sync)
            {
                if (_doneWriting) return false;
                written = WriteLocked(item, w);
            }
            Run(w);
            return written;
        }

        private ValueTask WriteAsync(T item, CancellationToken token)
        {
            if (token.IsCancellationRequested)
                return new ValueTask(ChannelReader<T>.Canceled<bool>(token));
            var w = new Wakeups();
            WriteWaiter waiter;
            lock (_sync)
            {
                if (_doneWriting)
                    return new ValueTask(ChannelReader<T>.Faulted<bool>(Closed()));
                if (WriteLocked(item, w))
                {
                    waiter = null;
                }
                else
                {
                    waiter = new WriteWaiter { Item = item };
                    _writers.Add(waiter);
                }
            }
            Run(w);
            if (waiter == null) return default;
            if (token.CanBeCanceled)
            {
                var registration = token.Register(() =>
                {
                    bool withdrawn;
                    lock (_sync) withdrawn = _writers.Remove(waiter);
                    if (withdrawn) waiter.Source.TrySetCanceled(token);
                });
                waiter.Source.Task.ContinueWith(_ => registration.Dispose());
            }
            return new ValueTask(waiter.Source.Task);
        }

        private ValueTask<bool> WaitToWriteAsync(CancellationToken token)
        {
            if (token.IsCancellationRequested)
                return new ValueTask<bool>(ChannelReader<T>.Canceled<bool>(token));
            TaskCompletionSource<bool> waiter;
            lock (_sync)
            {
                if (_doneWriting)
                {
                    if (_error != null) return new ValueTask<bool>(ChannelReader<T>.Faulted<bool>(_error));
                    return new ValueTask<bool>(false);
                }
                if (HasRoom || _mode != BoundedChannelFullMode.Wait) return new ValueTask<bool>(true);
                waiter = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
                _waitingToWrite.Add(waiter);
            }
            Cancellable(waiter, _waitingToWrite, token);
            return new ValueTask<bool>(waiter.Task);
        }

        private bool TryComplete(Exception error)
        {
            var w = new Wakeups();
            List<ReadWaiter> readers;
            List<WriteWaiter> writers;
            lock (_sync)
            {
                if (_doneWriting) return false;
                _doneWriting = true;
                _error = error;
                readers = new List<ReadWaiter>(_readers);
                _readers.Clear();
                writers = new List<WriteWaiter>(_writers);
                _writers.Clear();
                var wake = new List<TaskCompletionSource<bool>>(_waitingToRead);
                wake.AddRange(_waitingToWrite);
                _waitingToRead.Clear();
                _waitingToWrite.Clear();
                w.False = wake;
                w.Complete = _count == 0;
            }
            foreach (var reader in readers) reader.Source.TrySetException(Closed());
            foreach (var writer in writers) writer.Source.TrySetException(Closed());
            Run(w);
            return true;
        }

        // ── reading ─────────────────────────────────────────────────────────

        /// <summary>Under the lock: takes an item, letting a blocked writer in.</summary>
        private bool ReadLocked(out T item, Wakeups w)
        {
            if (_count == 0)
            {
                item = default;
                return false;
            }
            item = PopFront();
            // Room again: the longest-blocked writer's item goes in.
            while (_writers.Count > 0 && HasRoom)
            {
                var writer = _writers[0];
                _writers.RemoveAt(0);
                PushBack(writer.Item);
                (w.Written ??= new List<TaskCompletionSource<bool>>()).Add(writer.Source);
            }
            if (HasRoom) w.True = TakeAll(_waitingToWrite);
            if (_doneWriting && _count == 0) w.Complete = true;
            return true;
        }

        private bool TryRead(out T item)
        {
            var w = new Wakeups();
            bool read;
            lock (_sync) read = ReadLocked(out item, w);
            Run(w);
            return read;
        }

        private bool TryPeek(out T item)
        {
            lock (_sync)
            {
                if (_count == 0) { item = default; return false; }
                item = _ring[_head];
                return true;
            }
        }

        private ValueTask<T> ReadAsync(CancellationToken token)
        {
            if (token.IsCancellationRequested)
                return new ValueTask<T>(ChannelReader<T>.Canceled<T>(token));
            var w = new Wakeups();
            ReadWaiter waiter;
            T item;
            lock (_sync)
            {
                if (ReadLocked(out item, w))
                {
                    waiter = null;
                }
                else if (_doneWriting)
                {
                    return new ValueTask<T>(ChannelReader<T>.Faulted<T>(Closed()));
                }
                else
                {
                    waiter = new ReadWaiter();
                    _readers.Add(waiter);
                }
            }
            Run(w);
            if (waiter == null) return new ValueTask<T>(item);
            if (token.CanBeCanceled)
            {
                var registration = token.Register(() =>
                {
                    bool withdrawn;
                    lock (_sync) withdrawn = _readers.Remove(waiter);
                    if (withdrawn) waiter.Source.TrySetCanceled(token);
                });
                waiter.Source.Task.ContinueWith(_ => registration.Dispose());
            }
            return new ValueTask<T>(waiter.Source.Task);
        }

        private ValueTask<bool> WaitToReadAsync(CancellationToken token)
        {
            if (token.IsCancellationRequested)
                return new ValueTask<bool>(ChannelReader<T>.Canceled<bool>(token));
            TaskCompletionSource<bool> waiter;
            lock (_sync)
            {
                if (_count > 0) return new ValueTask<bool>(true);
                if (_doneWriting)
                {
                    if (_error != null) return new ValueTask<bool>(ChannelReader<T>.Faulted<bool>(_error));
                    return new ValueTask<bool>(false);
                }
                waiter = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
                _waitingToRead.Add(waiter);
            }
            Cancellable(waiter, _waitingToRead, token);
            return new ValueTask<bool>(waiter.Task);
        }

        private void Cancellable(TaskCompletionSource<bool> waiter, List<TaskCompletionSource<bool>> list, CancellationToken token)
        {
            if (!token.CanBeCanceled) return;
            var registration = token.Register(() =>
            {
                bool withdrawn;
                lock (_sync) withdrawn = list.Remove(waiter);
                if (withdrawn) waiter.TrySetCanceled(token);
            });
            waiter.Task.ContinueWith(_ => registration.Dispose());
        }

        private int CountNow()
        {
            lock (_sync) return _count;
        }

        // ── the two faces ───────────────────────────────────────────────────

        private sealed class BufferedReader : ChannelReader<T>
        {
            private readonly BufferedChannel<T> _channel;
            public BufferedReader(BufferedChannel<T> channel) { _channel = channel; }

            public override Task Completion => _channel._completion.Task;
            public override bool CanCount => true;
            public override bool CanPeek => true;
            public override int Count => _channel.CountNow();
            public override bool TryRead(out T item) => _channel.TryRead(out item);
            public override bool TryPeek(out T item) => _channel.TryPeek(out item);
            public override ValueTask<T> ReadAsync(CancellationToken cancellationToken = default) => _channel.ReadAsync(cancellationToken);
            public override ValueTask<bool> WaitToReadAsync(CancellationToken cancellationToken = default) => _channel.WaitToReadAsync(cancellationToken);
        }

        private sealed class BufferedWriter : ChannelWriter<T>
        {
            private readonly BufferedChannel<T> _channel;
            public BufferedWriter(BufferedChannel<T> channel) { _channel = channel; }

            public override bool TryComplete(Exception error = null) => _channel.TryComplete(error);
            public override bool TryWrite(T item) => _channel.TryWrite(item);
            public override ValueTask WriteAsync(T item, CancellationToken cancellationToken = default) => _channel.WriteAsync(item, cancellationToken);
            public override ValueTask<bool> WaitToWriteAsync(CancellationToken cancellationToken = default) => _channel.WaitToWriteAsync(cancellationToken);
        }
    }
}
