// System.Collections.Concurrent, written over a lock.
//
// .NET's versions are lock-free (segmented queues, per-thread bag lists,
// striped dictionary locks). These hold one Monitor per collection instead.
// The contract a program can observe is the same: every operation is atomic,
// nothing is lost or duplicated across threads, and enumeration is a snapshot
// that later writes do not disturb. What differs is throughput under heavy
// contention, which docs/limitations.md states.

using System.Collections;
using System.Collections.Generic;
using System.Threading;

namespace System.Collections.Concurrent
{
    public interface IProducerConsumerCollection<T> : IEnumerable<T>, ICollection
    {
        void CopyTo(T[] array, int index);
        T[] ToArray();
        bool TryAdd(T item);
        bool TryTake(out T item);
    }

    /// <summary>A thread-safe first-in, first-out collection.</summary>
    public class ConcurrentQueue<T> : IProducerConsumerCollection<T>, IReadOnlyCollection<T>
    {
        private readonly object _sync = new object();
        private T[] _items = new T[4];
        private int _head;
        private int _count;

        public ConcurrentQueue() { }

        public ConcurrentQueue(IEnumerable<T> collection)
        {
            if (collection == null) throw new ArgumentNullException(nameof(collection));
            foreach (var item in collection) EnqueueLocked(item);
        }

        public int Count { get { lock (_sync) return _count; } }
        public bool IsEmpty { get { lock (_sync) return _count == 0; } }

        public void Enqueue(T item)
        {
            lock (_sync) EnqueueLocked(item);
        }

        private void EnqueueLocked(T item)
        {
            if (_count == _items.Length)
            {
                var grown = new T[_items.Length * 2];
                for (int i = 0; i < _count; i++) grown[i] = _items[(_head + i) % _items.Length];
                _items = grown;
                _head = 0;
            }
            _items[(_head + _count) % _items.Length] = item;
            _count++;
        }

        public bool TryDequeue(out T result)
        {
            lock (_sync)
            {
                if (_count == 0) { result = default; return false; }
                result = _items[_head];
                _items[_head] = default;
                _head = (_head + 1) % _items.Length;
                _count--;
                return true;
            }
        }

        public bool TryPeek(out T result)
        {
            lock (_sync)
            {
                if (_count == 0) { result = default; return false; }
                result = _items[_head];
                return true;
            }
        }

        public void Clear()
        {
            lock (_sync)
            {
                _items = new T[4];
                _head = 0;
                _count = 0;
            }
        }

        public T[] ToArray()
        {
            lock (_sync)
            {
                var copy = new T[_count];
                for (int i = 0; i < _count; i++) copy[i] = _items[(_head + i) % _items.Length];
                return copy;
            }
        }

        public void CopyTo(T[] array, int index)
        {
            if (array == null) throw new ArgumentNullException(nameof(array));
            var copy = ToArray();
            for (int i = 0; i < copy.Length; i++) array[index + i] = copy[i];
        }

        bool IProducerConsumerCollection<T>.TryAdd(T item) { Enqueue(item); return true; }
        bool IProducerConsumerCollection<T>.TryTake(out T item) => TryDequeue(out item);

        public IEnumerator<T> GetEnumerator() => ((IEnumerable<T>)ToArray()).GetEnumerator();
        IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();

        bool ICollection.IsSynchronized => false;
        object ICollection.SyncRoot => throw new NotSupportedException("The SyncRoot property may not be used for the synchronization of concurrent collections.");
        void ICollection.CopyTo(Array array, int index) => Array.Copy(ToArray(), 0, array, index, Count);
    }

    /// <summary>A thread-safe last-in, first-out collection.</summary>
    public class ConcurrentStack<T> : IProducerConsumerCollection<T>, IReadOnlyCollection<T>
    {
        private readonly object _sync = new object();
        private readonly List<T> _items = new List<T>();

        public ConcurrentStack() { }

        public ConcurrentStack(IEnumerable<T> collection)
        {
            if (collection == null) throw new ArgumentNullException(nameof(collection));
            foreach (var item in collection) _items.Add(item);
        }

        public int Count { get { lock (_sync) return _items.Count; } }
        public bool IsEmpty { get { lock (_sync) return _items.Count == 0; } }

        public void Push(T item)
        {
            lock (_sync) _items.Add(item);
        }

        public void PushRange(T[] items)
        {
            if (items == null) throw new ArgumentNullException(nameof(items));
            PushRange(items, 0, items.Length);
        }

        public void PushRange(T[] items, int startIndex, int count)
        {
            if (items == null) throw new ArgumentNullException(nameof(items));
            if (startIndex < 0 || count < 0 || startIndex + count > items.Length)
                throw new ArgumentOutOfRangeException(nameof(startIndex));
            lock (_sync)
            {
                for (int i = 0; i < count; i++) _items.Add(items[startIndex + i]);
            }
        }

        public bool TryPop(out T result)
        {
            lock (_sync)
            {
                if (_items.Count == 0) { result = default; return false; }
                result = _items[_items.Count - 1];
                _items.RemoveAt(_items.Count - 1);
                return true;
            }
        }

        public int TryPopRange(T[] items)
        {
            if (items == null) throw new ArgumentNullException(nameof(items));
            return TryPopRange(items, 0, items.Length);
        }

        public int TryPopRange(T[] items, int startIndex, int count)
        {
            if (items == null) throw new ArgumentNullException(nameof(items));
            if (startIndex < 0 || count < 0 || startIndex + count > items.Length)
                throw new ArgumentOutOfRangeException(nameof(startIndex));
            lock (_sync)
            {
                int taken = 0;
                while (taken < count && _items.Count > 0)
                {
                    items[startIndex + taken] = _items[_items.Count - 1];
                    _items.RemoveAt(_items.Count - 1);
                    taken++;
                }
                return taken;
            }
        }

        public bool TryPeek(out T result)
        {
            lock (_sync)
            {
                if (_items.Count == 0) { result = default; return false; }
                result = _items[_items.Count - 1];
                return true;
            }
        }

        public void Clear()
        {
            lock (_sync) _items.Clear();
        }

        /// <summary>Top of the stack first, as .NET enumerates it.</summary>
        public T[] ToArray()
        {
            lock (_sync)
            {
                var copy = new T[_items.Count];
                for (int i = 0; i < copy.Length; i++) copy[i] = _items[_items.Count - 1 - i];
                return copy;
            }
        }

        public void CopyTo(T[] array, int index)
        {
            if (array == null) throw new ArgumentNullException(nameof(array));
            var copy = ToArray();
            for (int i = 0; i < copy.Length; i++) array[index + i] = copy[i];
        }

        bool IProducerConsumerCollection<T>.TryAdd(T item) { Push(item); return true; }
        bool IProducerConsumerCollection<T>.TryTake(out T item) => TryPop(out item);

        public IEnumerator<T> GetEnumerator() => ((IEnumerable<T>)ToArray()).GetEnumerator();
        IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();

        bool ICollection.IsSynchronized => false;
        object ICollection.SyncRoot => throw new NotSupportedException("The SyncRoot property may not be used for the synchronization of concurrent collections.");
        void ICollection.CopyTo(Array array, int index) => Array.Copy(ToArray(), 0, array, index, Count);
    }

    /// <summary>
    /// A thread-safe unordered collection. Taken most recently added first,
    /// which is what .NET's bag does for a single thread.
    /// </summary>
    public class ConcurrentBag<T> : IProducerConsumerCollection<T>, IReadOnlyCollection<T>
    {
        private readonly object _sync = new object();
        private readonly List<T> _items = new List<T>();

        public ConcurrentBag() { }

        public ConcurrentBag(IEnumerable<T> collection)
        {
            if (collection == null) throw new ArgumentNullException(nameof(collection));
            foreach (var item in collection) _items.Add(item);
        }

        public int Count { get { lock (_sync) return _items.Count; } }
        public bool IsEmpty { get { lock (_sync) return _items.Count == 0; } }

        public void Add(T item)
        {
            lock (_sync) _items.Add(item);
        }

        public bool TryTake(out T result)
        {
            lock (_sync)
            {
                if (_items.Count == 0) { result = default; return false; }
                result = _items[_items.Count - 1];
                _items.RemoveAt(_items.Count - 1);
                return true;
            }
        }

        public bool TryPeek(out T result)
        {
            lock (_sync)
            {
                if (_items.Count == 0) { result = default; return false; }
                result = _items[_items.Count - 1];
                return true;
            }
        }

        public void Clear()
        {
            lock (_sync) _items.Clear();
        }

        public T[] ToArray()
        {
            lock (_sync)
            {
                var copy = new T[_items.Count];
                for (int i = 0; i < copy.Length; i++) copy[i] = _items[_items.Count - 1 - i];
                return copy;
            }
        }

        public void CopyTo(T[] array, int index)
        {
            if (array == null) throw new ArgumentNullException(nameof(array));
            var copy = ToArray();
            for (int i = 0; i < copy.Length; i++) array[index + i] = copy[i];
        }

        bool IProducerConsumerCollection<T>.TryAdd(T item) { Add(item); return true; }

        public IEnumerator<T> GetEnumerator() => ((IEnumerable<T>)ToArray()).GetEnumerator();
        IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();

        bool ICollection.IsSynchronized => false;
        object ICollection.SyncRoot => throw new NotSupportedException("The SyncRoot property may not be used for the synchronization of concurrent collections.");
        void ICollection.CopyTo(Array array, int index) => Array.Copy(ToArray(), 0, array, index, Count);
    }

    /// <summary>A thread-safe dictionary.</summary>
    /// <remarks>
    /// The value factories passed to GetOrAdd and AddOrUpdate run outside the
    /// lock, as on .NET: they may run more than once under contention, and
    /// only one result is stored.
    /// </remarks>
    public class ConcurrentDictionary<TKey, TValue> : IDictionary<TKey, TValue>, IReadOnlyDictionary<TKey, TValue>
    {
        private readonly object _sync = new object();
        private readonly Dictionary<TKey, TValue> _map;

        public ConcurrentDictionary() { _map = new Dictionary<TKey, TValue>(); }

        public ConcurrentDictionary(IEqualityComparer<TKey> comparer)
        {
            _map = comparer == null ? new Dictionary<TKey, TValue>() : new Dictionary<TKey, TValue>(comparer);
        }

        public ConcurrentDictionary(IEnumerable<KeyValuePair<TKey, TValue>> collection) : this(collection, null) { }

        public ConcurrentDictionary(IEnumerable<KeyValuePair<TKey, TValue>> collection, IEqualityComparer<TKey> comparer) : this(comparer)
        {
            if (collection == null) throw new ArgumentNullException(nameof(collection));
            foreach (var pair in collection)
            {
                if (!TryAdd(pair.Key, pair.Value))
                    throw new ArgumentException("The source argument contains duplicate keys.");
            }
        }

        public ConcurrentDictionary(int concurrencyLevel, int capacity) : this() { }

        public ConcurrentDictionary(int concurrencyLevel, int capacity, IEqualityComparer<TKey> comparer) : this(comparer) { }

        public ConcurrentDictionary(int concurrencyLevel, IEnumerable<KeyValuePair<TKey, TValue>> collection, IEqualityComparer<TKey> comparer)
            : this(collection, comparer) { }

        public IEqualityComparer<TKey> Comparer => _map.Comparer;

        private static void CheckKey(TKey key)
        {
            if (key == null) throw new ArgumentNullException(nameof(key));
        }

        public int Count { get { lock (_sync) return _map.Count; } }
        public bool IsEmpty { get { lock (_sync) return _map.Count == 0; } }

        public TValue this[TKey key]
        {
            get
            {
                if (!TryGetValue(key, out var value))
                    throw new KeyNotFoundException("The given key '" + key + "' was not present in the dictionary.");
                return value;
            }
            set
            {
                CheckKey(key);
                lock (_sync) _map[key] = value;
            }
        }

        public bool TryAdd(TKey key, TValue value)
        {
            CheckKey(key);
            lock (_sync)
            {
                if (_map.ContainsKey(key)) return false;
                _map.Add(key, value);
                return true;
            }
        }

        public bool TryGetValue(TKey key, out TValue value)
        {
            CheckKey(key);
            // `default` assigned here rather than left to the native map: a
            // framework generic is erased, so on a miss it cannot know that
            // TValue is `int` and would hand back null. This class is not
            // erased, so `default` here is the right zero.
            lock (_sync)
            {
                if (_map.TryGetValue(key, out value)) return true;
            }
            value = default;
            return false;
        }

        public bool ContainsKey(TKey key)
        {
            CheckKey(key);
            lock (_sync) return _map.ContainsKey(key);
        }

        public bool TryRemove(TKey key, out TValue value)
        {
            CheckKey(key);
            lock (_sync)
            {
                if (_map.TryGetValue(key, out value))
                {
                    _map.Remove(key);
                    return true;
                }
            }
            value = default;
            return false;
        }

        public bool TryRemove(KeyValuePair<TKey, TValue> item)
        {
            CheckKey(item.Key);
            lock (_sync)
            {
                if (!_map.TryGetValue(item.Key, out var current)) return false;
                if (!EqualityComparer<TValue>.Default.Equals(current, item.Value)) return false;
                _map.Remove(item.Key);
                return true;
            }
        }

        public bool TryUpdate(TKey key, TValue newValue, TValue comparisonValue)
        {
            CheckKey(key);
            lock (_sync)
            {
                if (!_map.TryGetValue(key, out var current)) return false;
                if (!EqualityComparer<TValue>.Default.Equals(current, comparisonValue)) return false;
                _map[key] = newValue;
                return true;
            }
        }

        public TValue GetOrAdd(TKey key, Func<TKey, TValue> valueFactory)
        {
            CheckKey(key);
            if (valueFactory == null) throw new ArgumentNullException(nameof(valueFactory));
            if (TryGetValue(key, out var existing)) return existing;
            var made = valueFactory(key);
            lock (_sync)
            {
                if (_map.TryGetValue(key, out existing)) return existing;
                _map.Add(key, made);
                return made;
            }
        }

        public TValue GetOrAdd<TArg>(TKey key, Func<TKey, TArg, TValue> valueFactory, TArg factoryArgument)
        {
            CheckKey(key);
            if (valueFactory == null) throw new ArgumentNullException(nameof(valueFactory));
            if (TryGetValue(key, out var existing)) return existing;
            var made = valueFactory(key, factoryArgument);
            lock (_sync)
            {
                if (_map.TryGetValue(key, out existing)) return existing;
                _map.Add(key, made);
                return made;
            }
        }

        public TValue GetOrAdd(TKey key, TValue value)
        {
            CheckKey(key);
            lock (_sync)
            {
                if (_map.TryGetValue(key, out var existing)) return existing;
                _map.Add(key, value);
                return value;
            }
        }

        public TValue AddOrUpdate(TKey key, TValue addValue, Func<TKey, TValue, TValue> updateValueFactory)
        {
            CheckKey(key);
            if (updateValueFactory == null) throw new ArgumentNullException(nameof(updateValueFactory));
            while (true)
            {
                if (TryGetValue(key, out var old))
                {
                    var updated = updateValueFactory(key, old);
                    if (TryUpdate(key, updated, old)) return updated;
                }
                else if (TryAdd(key, addValue))
                {
                    return addValue;
                }
            }
        }

        public TValue AddOrUpdate(TKey key, Func<TKey, TValue> addValueFactory, Func<TKey, TValue, TValue> updateValueFactory)
        {
            CheckKey(key);
            if (addValueFactory == null) throw new ArgumentNullException(nameof(addValueFactory));
            if (updateValueFactory == null) throw new ArgumentNullException(nameof(updateValueFactory));
            while (true)
            {
                if (TryGetValue(key, out var old))
                {
                    var updated = updateValueFactory(key, old);
                    if (TryUpdate(key, updated, old)) return updated;
                }
                else
                {
                    var added = addValueFactory(key);
                    if (TryAdd(key, added)) return added;
                }
            }
        }

        public TValue AddOrUpdate<TArg>(TKey key, Func<TKey, TArg, TValue> addValueFactory, Func<TKey, TValue, TArg, TValue> updateValueFactory, TArg factoryArgument)
        {
            CheckKey(key);
            if (addValueFactory == null) throw new ArgumentNullException(nameof(addValueFactory));
            if (updateValueFactory == null) throw new ArgumentNullException(nameof(updateValueFactory));
            while (true)
            {
                if (TryGetValue(key, out var old))
                {
                    var updated = updateValueFactory(key, old, factoryArgument);
                    if (TryUpdate(key, updated, old)) return updated;
                }
                else
                {
                    var added = addValueFactory(key, factoryArgument);
                    if (TryAdd(key, added)) return added;
                }
            }
        }

        public void Clear()
        {
            lock (_sync) _map.Clear();
        }

        public KeyValuePair<TKey, TValue>[] ToArray()
        {
            lock (_sync)
            {
                var copy = new KeyValuePair<TKey, TValue>[_map.Count];
                int i = 0;
                foreach (var pair in _map) copy[i++] = pair;
                return copy;
            }
        }

        public ICollection<TKey> Keys
        {
            get
            {
                lock (_sync) return new List<TKey>(_map.Keys);
            }
        }

        public ICollection<TValue> Values
        {
            get
            {
                lock (_sync) return new List<TValue>(_map.Values);
            }
        }

        IEnumerable<TKey> IReadOnlyDictionary<TKey, TValue>.Keys => Keys;
        IEnumerable<TValue> IReadOnlyDictionary<TKey, TValue>.Values => Values;

        public IEnumerator<KeyValuePair<TKey, TValue>> GetEnumerator() => ((IEnumerable<KeyValuePair<TKey, TValue>>)ToArray()).GetEnumerator();
        IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();

        void IDictionary<TKey, TValue>.Add(TKey key, TValue value)
        {
            if (!TryAdd(key, value)) throw new ArgumentException("The key already existed in the dictionary.");
        }

        bool IDictionary<TKey, TValue>.Remove(TKey key) => TryRemove(key, out _);

        bool ICollection<KeyValuePair<TKey, TValue>>.IsReadOnly => false;

        void ICollection<KeyValuePair<TKey, TValue>>.Add(KeyValuePair<TKey, TValue> item)
        {
            if (!TryAdd(item.Key, item.Value)) throw new ArgumentException("The key already existed in the dictionary.");
        }

        bool ICollection<KeyValuePair<TKey, TValue>>.Contains(KeyValuePair<TKey, TValue> item)
        {
            return TryGetValue(item.Key, out var value) && EqualityComparer<TValue>.Default.Equals(value, item.Value);
        }

        void ICollection<KeyValuePair<TKey, TValue>>.CopyTo(KeyValuePair<TKey, TValue>[] array, int arrayIndex)
        {
            var copy = ToArray();
            for (int i = 0; i < copy.Length; i++) array[arrayIndex + i] = copy[i];
        }

        bool ICollection<KeyValuePair<TKey, TValue>>.Remove(KeyValuePair<TKey, TValue> item) => TryRemove(item);
    }

    /// <summary>
    /// Blocking and bounding over any producer/consumer collection.
    /// </summary>
    public class BlockingCollection<T> : IEnumerable<T>, IReadOnlyCollection<T>, IDisposable
    {
        private readonly object _sync = new object();
        private readonly IProducerConsumerCollection<T> _items;
        private readonly int _capacity;
        private int _count;
        private bool _addingCompleted;
        private bool _disposed;

        public BlockingCollection() : this(new ConcurrentQueue<T>(), -1) { }

        public BlockingCollection(int boundedCapacity) : this(new ConcurrentQueue<T>(), boundedCapacity) { }

        public BlockingCollection(IProducerConsumerCollection<T> collection) : this(collection, -1) { }

        public BlockingCollection(IProducerConsumerCollection<T> collection, int boundedCapacity)
        {
            if (collection == null) throw new ArgumentNullException(nameof(collection));
            if (boundedCapacity == 0 || boundedCapacity < -1)
                throw new ArgumentOutOfRangeException(nameof(boundedCapacity), "The boundedCapacity argument must be positive.");
            _items = collection;
            _capacity = boundedCapacity;
            _count = collection.Count;
        }

        public int BoundedCapacity => _capacity;
        public int Count { get { lock (_sync) return _count; } }
        public bool IsAddingCompleted { get { lock (_sync) return _addingCompleted; } }
        public bool IsCompleted { get { lock (_sync) return _addingCompleted && _count == 0; } }

        private void CheckDisposed()
        {
            if (_disposed) throw new ObjectDisposedException(nameof(BlockingCollection<T>));
        }

        public void Add(T item) => TryAdd(item, Timeout.Infinite, default);

        public void Add(T item, CancellationToken cancellationToken) => TryAdd(item, Timeout.Infinite, cancellationToken);

        public bool TryAdd(T item) => TryAdd(item, 0, default);

        public bool TryAdd(T item, int millisecondsTimeout) => TryAdd(item, millisecondsTimeout, default);

        public bool TryAdd(T item, TimeSpan timeout) => TryAdd(item, ToMillis(timeout), default);

        public bool TryAdd(T item, int millisecondsTimeout, CancellationToken cancellationToken)
        {
            CheckDisposed();
            long deadline = Deadline(millisecondsTimeout);
            lock (_sync)
            {
                while (true)
                {
                    if (_addingCompleted)
                        throw new InvalidOperationException("The collection has been marked as complete with regards to additions.");
                    cancellationToken.ThrowIfCancellationRequested();
                    if (_capacity == -1 || _count < _capacity) break;
                    if (!WaitLocked(deadline, cancellationToken)) return false;
                }
                if (!_items.TryAdd(item))
                    throw new InvalidOperationException("The underlying collection didn't accept the item.");
                _count++;
                Monitor.PulseAll(_sync);
                return true;
            }
        }

        public T Take() => Take(default);

        public T Take(CancellationToken cancellationToken)
        {
            if (!TryTake(out var item, Timeout.Infinite, cancellationToken))
                throw new InvalidOperationException("The collection argument is empty and has been marked as complete with regards to additions.");
            return item;
        }

        public bool TryTake(out T item) => TryTake(out item, 0, default);

        public bool TryTake(out T item, int millisecondsTimeout) => TryTake(out item, millisecondsTimeout, default);

        public bool TryTake(out T item, TimeSpan timeout) => TryTake(out item, ToMillis(timeout), default);

        public bool TryTake(out T item, int millisecondsTimeout, CancellationToken cancellationToken)
        {
            CheckDisposed();
            long deadline = Deadline(millisecondsTimeout);
            lock (_sync)
            {
                while (_count == 0)
                {
                    cancellationToken.ThrowIfCancellationRequested();
                    if (_addingCompleted || !WaitLocked(deadline, cancellationToken))
                    {
                        item = default;
                        return false;
                    }
                }
                if (!_items.TryTake(out item))
                    throw new InvalidOperationException("The underlying collection was modified from outside of the BlockingCollection<T>.");
                _count--;
                Monitor.PulseAll(_sync);
                return true;
            }
        }

        public void CompleteAdding()
        {
            lock (_sync)
            {
                _addingCompleted = true;
                Monitor.PulseAll(_sync);
            }
        }

        public IEnumerable<T> GetConsumingEnumerable() => GetConsumingEnumerable(default);

        public IEnumerable<T> GetConsumingEnumerable(CancellationToken cancellationToken)
        {
            while (TryTake(out var item, Timeout.Infinite, cancellationToken))
                yield return item;
        }

        public T[] ToArray()
        {
            lock (_sync) return _items.ToArray();
        }

        public void CopyTo(T[] array, int index)
        {
            var copy = ToArray();
            for (int i = 0; i < copy.Length; i++) array[index + i] = copy[i];
        }

        public IEnumerator<T> GetEnumerator() => ((IEnumerable<T>)ToArray()).GetEnumerator();
        IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();

        public void Dispose()
        {
            _disposed = true;
        }

        /// <summary>
        /// Waits on the monitor until pulsed, the deadline passes, or the token
        /// is cancelled. Wakes at least every 50 ms so a cancellation that
        /// pulses nothing is still seen.
        /// </summary>
        private bool WaitLocked(long deadline, CancellationToken token)
        {
            int slice = 50;
            if (deadline != long.MaxValue)
            {
                long left = deadline - Environment.TickCount64;
                if (left <= 0) return false;
                if (left < slice) slice = (int)left;
            }
            else if (!token.CanBeCanceled)
            {
                slice = Timeout.Infinite;
            }
            Monitor.Wait(_sync, slice);
            return true;
        }

        private static long Deadline(int millisecondsTimeout)
        {
            if (millisecondsTimeout == Timeout.Infinite) return long.MaxValue;
            if (millisecondsTimeout < 0) throw new ArgumentOutOfRangeException(nameof(millisecondsTimeout));
            return Environment.TickCount64 + millisecondsTimeout;
        }

        private static int ToMillis(TimeSpan timeout)
        {
            long ms = (long)timeout.TotalMilliseconds;
            if (ms == -1) return Timeout.Infinite;
            return (int)ms;
        }
    }
}
