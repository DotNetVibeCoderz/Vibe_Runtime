using System.Runtime.CompilerServices;

namespace RustNet.Media;

/// <summary>
/// PCM audio playback over the I2S HAL (v0.8, chip-gated). Configure the
/// sink, then push little-endian 16-bit PCM buffers. On the virtual device
/// the samples flow into the I2S simulator so pipelines are testable without
/// a DAC/amp; on real silicon they reach the I2S peripheral.
/// </summary>
public static class Audio
{
    /// <summary>Configure the audio sink before playing.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern void Configure(int sampleRate, int bitsPerSample, int channels);

    /// <summary>Queue a little-endian 16-bit PCM buffer for playback.
    /// Returns the number of samples the sink accepted.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int Play(byte[] pcm);

    /// <summary>Cumulative PCM samples the sink has accepted since boot.</summary>
    [MethodImpl(MethodImplOptions.InternalCall)]
    public static extern int SamplesPlayed();
}
