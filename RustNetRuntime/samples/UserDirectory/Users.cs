namespace UserDirectory;

/// <summary>One person in the sample directory.</summary>
public sealed class User
{
    public int Id;
    public string Username = "";
    public string DisplayName = "";
    public string Role = "";
    public string Team = "";
    public bool Active;
    public string Joined = "";

    /// <summary>The dates are ISO, so the year is the first four characters.</summary>
    public int JoinYear()
    {
        if (Joined.Length < 4) return 0;
        int year = 0;
        for (int i = 0; i < 4; i++)
        {
            year = year * 10 + (Joined[i] - 48);
        }
        return year;
    }
}

/// <summary>
/// The sample directory, mirroring samples/data/users.json.
///
/// The rows are embedded rather than read from disk: RustBCL does not implement
/// System.IO yet, and a sample that ran on only one of the two runtimes would
/// defeat the point of shipping it here.
/// </summary>
public static class Directory
{
    public static User[] Load()
    {
        var users = new User[15];
        users[0] = Make(1, "kang.fadhil", "Kang Fadhil", "owner", "Runtime", true, "2024-01-15");
        users[1] = Make(2, "sari.wulandari", "Sari Wulandari", "admin", "Runtime", true, "2024-02-02");
        users[2] = Make(3, "bagus.p", "Bagus Prakoso", "developer", "Runtime", true, "2024-03-11");
        users[3] = Make(4, "intan.k", "Intan Kusuma", "developer", "Tooling", true, "2024-03-28");
        users[4] = Make(5, "rizky.h", "Rizky Hamdani", "developer", "Tooling", true, "2024-05-06");
        users[5] = Make(6, "dewi.a", "Dewi Anggraini", "qa", "Quality", true, "2024-06-17");
        users[6] = Make(7, "yoga.s", "Yoga Saputra", "qa", "Quality", false, "2024-07-01");
        users[7] = Make(8, "nadia.f", "Nadia Fitriani", "designer", "Product", true, "2024-08-19");
        users[8] = Make(9, "arif.m", "Arif Maulana", "developer", "Embedded", true, "2024-09-30");
        users[9] = Make(10, "putri.l", "Putri Lestari", "developer", "Embedded", true, "2024-10-14");
        users[10] = Make(11, "hendra.w", "Hendra Wijaya", "analyst", "Product", true, "2025-01-08");
        users[11] = Make(12, "citra.n", "Citra Nuraini", "developer", "Runtime", true, "2025-02-24");
        users[12] = Make(13, "gilang.r", "Gilang Ramadhan", "devops", "Platform", true, "2025-04-02");
        users[13] = Make(14, "maya.s", "Maya Safitri", "writer", "Product", true, "2025-05-19");
        users[14] = Make(15, "teguh.b", "Teguh Budiman", "developer", "Tooling", false, "2025-06-30");
        return users;
    }

    private static User Make(
        int id, string username, string name, string role, string team, bool active, string joined)
    {
        var user = new User();
        user.Id = id;
        user.Username = username;
        user.DisplayName = name;
        user.Role = role;
        user.Team = team;
        user.Active = active;
        user.Joined = joined;
        return user;
    }
}
