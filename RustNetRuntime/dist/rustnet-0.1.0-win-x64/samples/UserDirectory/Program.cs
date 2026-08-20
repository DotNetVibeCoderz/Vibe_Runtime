namespace UserDirectory;

/// <summary>
/// Reports over the sample user directory.
///
/// Written with explicit loops and arrays rather than LINQ and generic
/// collections, so it runs on RustCLR as well as .NET. Compare the two:
///
///     dotnet bin/Release/net9.0/UserDirectory.dll
///     rustnet run bin/Release/net9.0/UserDirectory.dll
///
/// The output should be identical.
/// </summary>
public static class Program
{
    public static void Main()
    {
        var users = Directory.Load();
        int active = CountActive(users);

        Console.WriteLine("GRAVICODE STUDIOS - USER DIRECTORY");
        Console.WriteLine("==================================");
        Console.WriteLine("Total accounts: " + users.Length);
        Console.WriteLine("Active:         " + active);
        Console.WriteLine("Suspended:      " + (users.Length - active));
        Console.WriteLine();

        Console.WriteLine("BY TEAM");
        var teams = Distinct(users, true);
        for (int t = 0; t < teams.Length; t++)
        {
            Console.WriteLine("  " + Pad(teams[t], 12) + CountMatching(users, teams[t], true));
        }
        Console.WriteLine();

        Console.WriteLine("BY ROLE");
        var roles = Distinct(users, false);
        for (int r = 0; r < roles.Length; r++)
        {
            Console.WriteLine("  " + Pad(roles[r], 12) + CountMatching(users, roles[r], false));
        }
        Console.WriteLine();

        Console.WriteLine("JOINED IN 2025");
        for (int i = 0; i < users.Length; i++)
        {
            if (users[i].JoinYear() != 2025) continue;
            Console.WriteLine("  " + Pad(users[i].DisplayName, 20) + users[i].Joined + "  " + users[i].Team);
        }
        Console.WriteLine();

        Console.WriteLine("SUSPENDED ACCOUNTS");
        bool any = false;
        for (int i = 0; i < users.Length; i++)
        {
            if (users[i].Active) continue;
            any = true;
            Console.WriteLine("  " + Pad(users[i].Username, 20) + users[i].DisplayName);
        }
        if (!any)
        {
            Console.WriteLine("  (none)");
        }
    }

    private static int CountActive(User[] users)
    {
        int total = 0;
        for (int i = 0; i < users.Length; i++)
        {
            if (users[i].Active) total++;
        }
        return total;
    }

    /// <summary>Counts users whose team (or role) equals <paramref name="value"/>.</summary>
    private static int CountMatching(User[] users, string value, bool byTeam)
    {
        int total = 0;
        for (int i = 0; i < users.Length; i++)
        {
            string field = byTeam ? users[i].Team : users[i].Role;
            if (field == value) total++;
        }
        return total;
    }

    /// <summary>
    /// Distinct teams or roles, in first-seen order.
    ///
    /// No HashSet here — it is generic, and generics are erased on RustCLR
    /// today. A linear scan over fifteen rows costs nothing.
    /// </summary>
    private static string[] Distinct(User[] users, bool byTeam)
    {
        var seen = new string[users.Length];
        int count = 0;
        for (int i = 0; i < users.Length; i++)
        {
            string value = byTeam ? users[i].Team : users[i].Role;
            if (Contains(seen, count, value)) continue;
            seen[count] = value;
            count++;
        }

        var result = new string[count];
        for (int i = 0; i < count; i++)
        {
            result[i] = seen[i];
        }
        return result;
    }

    private static bool Contains(string[] values, int count, string value)
    {
        for (int i = 0; i < count; i++)
        {
            if (values[i] == value) return true;
        }
        return false;
    }

    private static string Pad(string text, int width)
    {
        string padded = text;
        while (padded.Length < width)
        {
            padded = padded + " ";
        }
        return padded;
    }
}
