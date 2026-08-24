using System;
using System.IO;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using RustNet.Deploy;

namespace RustNet.Designer.Deployment;

/// <summary>
/// The deploy path, in process: C# → assembly → RNX → signed RNSB → flashed over
/// RNDP → started. Same libraries the <c>rustnet</c> CLI uses, so what the
/// Designer sends is byte-identical to <c>rustnet flash --start</c>.
///
/// A layout deploys differently: RustNet.UI XML is data, so it is pushed to the
/// device filesystem where an app can load it with <c>Ui.LoadXml</c> — no
/// reflash, which is the whole point of the XML format.
/// </summary>
public sealed class Deployer
{
    private readonly Action<string> _log;

    public Deployer(Action<string> log) => _log = log;

    /// <summary>Where a pushed layout lands unless told otherwise.</summary>
    public const string DefaultLayoutPath = "/data/ui.xml";

    public sealed record Result(bool Ok, string Summary);

    /// <summary>
    /// Build, sign, flash and optionally start. The chip family is the one the
    /// device reported when it was probed — signing with the wrong family makes
    /// the device reject a perfectly good image.
    /// </summary>
    public Task<Result> DeployCodeAsync(
        string source,
        string appName,
        DeviceTarget target,
        string signingKeyPath,
        string workspaceRoot,
        bool start,
        CancellationToken cancellationToken)
        => Task.Run(async () =>
        {
            if (source.Trim().Length == 0)
            {
                return Fail("The code pane is empty.");
            }
            if (!File.Exists(signingKeyPath))
            {
                return Fail($"No signing key at {signingKeyPath}. Generate one with "
                    + "`rustnet keys generate --out keys`, and provision the device with its .pub half.");
            }

            string name = AppBuilder.Sanitize(appName);
            _log($"building {name} …");
            AppBuilder.BuildResult build = await AppBuilder
                .BuildAsync(source, name, workspaceRoot, _log, cancellationToken)
                .ConfigureAwait(false);
            if (!build.Ok)
            {
                return Fail("Build failed — see the output above.");
            }

            // No intermediate format. RustCLR reads the PE/ECMA-335 assembly
            // the SDK produced, so what is signed and sent is exactly what was
            // built — one fewer metadata writer to keep in step with Roslyn,
            // and one fewer place for the two to disagree.
            byte[] assembly;
            try
            {
                assembly = File.ReadAllBytes(build.AssemblyPath);
            }
            catch (Exception ex)
            {
                return Fail("Could not read the built assembly: " + ex.Message);
            }
            _log($"assembly: {assembly.Length} bytes");

            ChipFamily chip = ResolveChip(target);
            byte[] sealedApp;
            try
            {
                sealedApp = Signing.Seal(ImageKind.App, chip, assembly, File.ReadAllBytes(signingKeyPath));
            }
            catch (Exception ex)
            {
                return Fail("Signing failed: " + ex.Message);
            }
            _log($"signed for chip={chip} ({sealedApp.Length} bytes)");

            try
            {
                using RndpClient client = RndpClient.Connect(target.Spec);
                client.FlashApp(name, sealedApp);
                _log($"flashed '{name}' to {target.Spec}");
                if (start)
                {
                    client.StartApp(name);
                    _log("started");
                }
                return new Result(true, start ? $"'{name}' flashed and started." : $"'{name}' flashed.");
            }
            catch (Exception ex)
            {
                return Fail("Deploy failed: " + ex.Message);
            }
        }, cancellationToken);

    /// <summary>Push a RustNet.UI layout onto the device filesystem.</summary>
    public Task<Result> PushLayoutAsync(
        string xml,
        string remotePath,
        DeviceTarget target,
        CancellationToken cancellationToken)
        => Task.Run(() =>
        {
            if (xml.Trim().Length == 0)
            {
                return Fail("There is no layout to push.");
            }
            // Parse first: a layout the device cannot read is worse than no
            // layout, because the app will fail at startup instead of here.
            try
            {
                RustNet.UI.Ui.LoadXml(xml);
            }
            catch (Exception ex)
            {
                return Fail("That layout does not parse: " + ex.Message);
            }

            byte[] bytes = Encoding.UTF8.GetBytes(xml);
            try
            {
                using RndpClient client = RndpClient.Connect(target.Spec);
                client.FlashData(remotePath, bytes);
                _log($"pushed {bytes.Length} bytes to {remotePath} on {target.Spec}");
                return new Result(true, $"Layout pushed to {remotePath}.");
            }
            catch (Exception ex)
            {
                return Fail("Push failed: " + ex.Message);
            }
        }, cancellationToken);

    /// <summary>Stop whatever app is running on the target.</summary>
    public Task<Result> StopAsync(DeviceTarget target, CancellationToken cancellationToken)
        => Task.Run(() =>
        {
            try
            {
                using RndpClient client = RndpClient.Connect(target.Spec);
                client.StopApp();
                _log("stopped the running app");
                return new Result(true, "App stopped.");
            }
            catch (Exception ex)
            {
                return Fail("Stop failed: " + ex.Message);
            }
        }, cancellationToken);

    /// <summary>The last <paramref name="lines"/> of the device log.</summary>
    public Task<Result> ReadLogsAsync(DeviceTarget target, int lines, CancellationToken cancellationToken)
        => Task.Run(() =>
        {
            try
            {
                using RndpClient client = RndpClient.Connect(target.Spec);
                string logs = client.GetLogs(lines);
                foreach (string line in logs.Split('\n'))
                {
                    if (line.Trim().Length > 0)
                    {
                        _log(line.TrimEnd());
                    }
                }
                return new Result(true, "Device log read.");
            }
            catch (Exception ex)
            {
                return Fail("Could not read the log: " + ex.Message);
            }
        }, cancellationToken);

    private static ChipFamily ResolveChip(DeviceTarget target)
    {
        if (target.Chip.Length == 0)
        {
            // Unprobed target: Any verifies on every family.
            return ChipFamily.Any;
        }
        try
        {
            return Signing.ParseChip(target.Chip);
        }
        catch (ArgumentException)
        {
            return ChipFamily.Any;
        }
    }

    private Result Fail(string message)
    {
        _log(message);
        return new Result(false, message);
    }
}
