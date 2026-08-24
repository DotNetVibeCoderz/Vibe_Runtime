using System.Security.Cryptography;

namespace RustNet.Deploy;

public enum ImageKind : byte
{
    Firmware = 0,
    App = 1,
    Data = 2,
    BootImage = 3,
}

public enum ChipFamily : byte
{
    Any = 0,
    Esp32 = 1,
    Stm32 = 2,
    Ti = 3,
    Nxp = 4,
    HostSim = 5,
    /// <summary>ESP32-C3 (RISC-V RV32IMC).</summary>
    Esp32C3 = 6,
    /// <summary>Kendryte K210 (RISC-V RV64GC).</summary>
    K210 = 7,

    /// <summary>Raspberry Pi RP2040 (dual Cortex-M0+).</summary>
    Rp2040 = 8,
}

/// <summary>
/// Creates RNSB signed containers (byte-compatible with
/// runtime/rustnet-secureboot): RSA PKCS#1 v1.5 + SHA-256 over the header
/// (sig_len zeroed) + payload.
/// </summary>
public static class Signing
{
    public static ChipFamily ParseChip(string name) => name.ToLowerInvariant() switch
    {
        "any" => ChipFamily.Any,
        "esp32" => ChipFamily.Esp32,
        "stm32" => ChipFamily.Stm32,
        "ti" => ChipFamily.Ti,
        "nxp" => ChipFamily.Nxp,
        "host" or "host-sim" => ChipFamily.HostSim,
        "esp32c3" or "esp32-c3" => ChipFamily.Esp32C3,
        "k210" or "kendryte" => ChipFamily.K210,
        "rp2040" or "pico" or "rpi-pico" => ChipFamily.Rp2040,
        _ => throw new ArgumentException($"unknown chip '{name}' (esp32|esp32c3|k210|rp2040|stm32|ti|nxp|host-sim|any)"),
    };

    /// <summary>Generate an RSA-2048 keypair as (privateKeyPkcs1Der, publicKeyPkcs1Der).</summary>
    public static (byte[] PrivateDer, byte[] PublicDer) GenerateKeypair()
    {
        using var rsa = RSA.Create(2048);
        return (rsa.ExportRSAPrivateKey(), rsa.ExportRSAPublicKey());
    }

    public static byte[] Seal(ImageKind kind, ChipFamily chip, byte[] payload, byte[] privateKeyDer)
    {
        byte[] unsigned = new byte[16 + payload.Length];
        unsigned[0] = (byte)'R';
        unsigned[1] = (byte)'N';
        unsigned[2] = (byte)'S';
        unsigned[3] = (byte)'B';
        BitConverter.TryWriteBytes(unsigned.AsSpan(4, 2), (ushort)1); // version
        unsigned[6] = (byte)kind;
        unsigned[7] = (byte)chip;
        BitConverter.TryWriteBytes(unsigned.AsSpan(8, 4), (uint)payload.Length);
        // bytes 12..16 remain zero (sig_len) while signing
        payload.CopyTo(unsigned, 16);

        using var rsa = RSA.Create();
        rsa.ImportRSAPrivateKey(privateKeyDer, out _);
        byte[] signature = rsa.SignData(unsigned, HashAlgorithmName.SHA256, RSASignaturePadding.Pkcs1);

        byte[] sealedImage = new byte[unsigned.Length + signature.Length];
        unsigned.CopyTo(sealedImage, 0);
        BitConverter.TryWriteBytes(sealedImage.AsSpan(12, 4), (uint)signature.Length);
        signature.CopyTo(sealedImage, unsigned.Length);
        return sealedImage;
    }
}
