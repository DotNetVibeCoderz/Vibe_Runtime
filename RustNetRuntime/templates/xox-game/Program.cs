using RustNet.Graphics;

namespace __NAME__;

/// <summary>
/// XoX (tic-tac-toe): X plays a center-corner strategy, O answers with
/// random moves. The board renders to both console and the device display.
/// </summary>
public static class Program
{
    private static readonly int[] Board = new int[9]; // 0 empty, 1 X, 2 O

    public static void Main()
    {
        Console.WriteLine("__NAME__ - XoX");
        Display.Init(160, 128);
        Random rng = new Random(42);

        int turn = 1;
        int moves = 0;
        while (moves < 9)
        {
            int cell = turn == 1 ? PickForX() : PickRandom(rng);
            Board[cell] = turn;
            moves = moves + 1;
            Render();
            int winner = Winner();
            if (winner != 0)
            {
                Console.WriteLine(string.Concat("winner: ", winner == 1 ? "X" : "O"));
                Display.DrawText(8, 110, winner == 1 ? "X WINS!" : "O WINS!", Color.Yellow, 1);
                Display.Present();
                return;
            }
            turn = 3 - turn;
        }
        Console.WriteLine("draw");
        Display.DrawText(8, 110, "DRAW", Color.Cyan, 1);
        Display.Present();
    }

    private static int PickForX()
    {
        // center, then corners, then edges
        int[] preference = new int[9];
        preference[0] = 4;
        preference[1] = 0;
        preference[2] = 2;
        preference[3] = 6;
        preference[4] = 8;
        preference[5] = 1;
        preference[6] = 3;
        preference[7] = 5;
        preference[8] = 7;
        for (int i = 0; i < 9; i++)
        {
            if (Board[preference[i]] == 0)
            {
                return preference[i];
            }
        }
        return 0;
    }

    private static int PickRandom(Random rng)
    {
        while (true)
        {
            int cell = rng.Next(9);
            if (Board[cell] == 0)
            {
                return cell;
            }
        }
    }

    private static int Winner()
    {
        int[] lines = new int[24];
        // rows, cols, diagonals as triples
        int[] data = new int[24];
        data[0] = 0; data[1] = 1; data[2] = 2;
        data[3] = 3; data[4] = 4; data[5] = 5;
        data[6] = 6; data[7] = 7; data[8] = 8;
        data[9] = 0; data[10] = 3; data[11] = 6;
        data[12] = 1; data[13] = 4; data[14] = 7;
        data[15] = 2; data[16] = 5; data[17] = 8;
        data[18] = 0; data[19] = 4; data[20] = 8;
        data[21] = 2; data[22] = 4; data[23] = 6;
        for (int i = 0; i < 24; i = i + 3)
        {
            int a = Board[data[i]];
            if (a != 0 && a == Board[data[i + 1]] && a == Board[data[i + 2]])
            {
                return a;
            }
        }
        lines[0] = 0;
        return 0;
    }

    private static void Render()
    {
        // Console
        for (int row = 0; row < 3; row++)
        {
            string line = "";
            for (int col = 0; col < 3; col++)
            {
                int v = Board[row * 3 + col];
                line = string.Concat(line, v == 0 ? "." : (v == 1 ? "X" : "O"));
            }
            Console.WriteLine(line);
        }
        Console.WriteLine("---");

        // Display: 3x3 grid of 32px cells at (28, 8)
        Display.Clear(Color.Black);
        for (int i = 0; i <= 3; i++)
        {
            Display.DrawLine(28, 8 + i * 32, 124, 8 + i * 32, Color.White);
            Display.DrawLine(28 + i * 32, 8, 28 + i * 32, 104, Color.White);
        }
        for (int cell = 0; cell < 9; cell++)
        {
            int cx = 28 + (cell % 3) * 32 + 16;
            int cy = 8 + (cell / 3) * 32 + 16;
            if (Board[cell] == 1)
            {
                Display.DrawLine(cx - 8, cy - 8, cx + 8, cy + 8, Color.Red);
                Display.DrawLine(cx - 8, cy + 8, cx + 8, cy - 8, Color.Red);
            }
            else if (Board[cell] == 2)
            {
                Display.DrawCircle(cx, cy, 9, Color.Green);
            }
        }
        Display.Present();
    }
}
