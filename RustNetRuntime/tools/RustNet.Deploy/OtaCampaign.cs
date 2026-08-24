namespace RustNet.Deploy;

/// <summary>Outcome of pushing an OTA image to one device.</summary>
public enum OtaStatus
{
    /// <summary>Image uploaded, verified and staged (not yet confirmed).</summary>
    Updated,
    /// <summary>Uploaded and confirmed into the active slot.</summary>
    Confirmed,
    /// <summary>Push or confirm failed.</summary>
    Failed,
    /// <summary>Not attempted because the rollout was aborted.</summary>
    Skipped,
}

public sealed record DeviceOutcome(string Device, OtaStatus Status, string? Error = null);

/// <summary>Rollout policy for a fleet OTA campaign.</summary>
public sealed record OtaCampaignPolicy
{
    /// <summary>Devices updated first as a canary before the rest of the fleet.</summary>
    public int CanarySize { get; init; } = 1;
    /// <summary>Devices per batch after the canary (0 = all remaining at once).</summary>
    public int BatchSize { get; init; }
    /// <summary>Abort the rollout once this many devices have failed.</summary>
    public int AbortAfterFailures { get; init; } = 1;
}

public sealed record CampaignResult(IReadOnlyList<DeviceOutcome> Outcomes, bool Aborted)
{
    public int Succeeded =>
        Outcomes.Count(o => o.Status is OtaStatus.Updated or OtaStatus.Confirmed);
    public int Failed => Outcomes.Count(o => o.Status == OtaStatus.Failed);
    public int Skipped => Outcomes.Count(o => o.Status == OtaStatus.Skipped);
}

/// <summary>
/// Orchestrates a staged OTA rollout across a fleet: a canary batch first, then
/// the rest in batches, aborting (and skipping the remainder) once too many
/// devices fail. The per-device push is injected, so the orchestration is
/// testable without hardware and reusable across transports.
/// </summary>
public static class OtaCampaign
{
    public static CampaignResult Run(
        IReadOnlyList<string> devices,
        OtaCampaignPolicy policy,
        Func<string, DeviceOutcome> pushDevice)
    {
        var outcomes = new List<DeviceOutcome>();
        int failures = 0;
        bool aborted = false;

        foreach (var (start, count) in Batches(devices.Count, policy))
        {
            if (aborted)
            {
                break;
            }
            for (int k = start; k < start + count; k++)
            {
                DeviceOutcome outcome;
                try
                {
                    outcome = pushDevice(devices[k]);
                }
                catch (Exception ex)
                {
                    outcome = new DeviceOutcome(devices[k], OtaStatus.Failed, ex.Message);
                }
                outcomes.Add(outcome);
                if (outcome.Status == OtaStatus.Failed)
                {
                    failures++;
                    if (failures >= policy.AbortAfterFailures)
                    {
                        aborted = true;
                        break;
                    }
                }
            }
        }

        // Anything not attempted (after an abort) is recorded as skipped.
        while (outcomes.Count < devices.Count)
        {
            outcomes.Add(new DeviceOutcome(devices[outcomes.Count], OtaStatus.Skipped));
        }
        return new CampaignResult(outcomes, aborted);
    }

    /// <summary>(start, count) ranges: the canary first, then the remainder in
    /// batches of the policy's size (all at once when the size is 0).</summary>
    private static IEnumerable<(int Start, int Count)> Batches(int total, OtaCampaignPolicy policy)
    {
        int canary = Math.Clamp(policy.CanarySize, 0, total);
        if (canary > 0)
        {
            yield return (0, canary);
        }
        int step = policy.BatchSize > 0 ? policy.BatchSize : Math.Max(1, total - canary);
        for (int i = canary; i < total; i += step)
        {
            yield return (i, Math.Min(step, total - i));
        }
    }
}
