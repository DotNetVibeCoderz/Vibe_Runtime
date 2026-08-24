using RustNet.Media;
using RustNet.Net;

// A bring-up check for the two device surfaces that need a physical world to
// verify: the radio and the camera.
//
// It carries no credentials. An empty Connect means "use whatever `rustnet
// wifi --ssid ... --psk ...` provisioned", and provisioned credentials live in
// the firmware's RAM rather than in flash — so they arrive after this app has
// already started, and the join has to be retried rather than attempted once.
// That retry is not defensive coding; it is the whole shape of the flow.
class Program
{
    static void Main()
    {
        Camera.Configure(320, 240);
        byte[] frame = Camera.Capture();
        int lit = 0;
        for (int i = 0; i < frame.Length; i += 2)
        {
            if (frame[i] != 0 || frame[i + 1] != 0) lit++;
        }
        Console.WriteLine("camera: " + Camera.Width() + "x" + Camera.Height()
            + ", " + frame.Length + " bytes, " + (lit * 100 / (frame.Length / 2)) + "% lit");

        int attempt = 0;
        while (true)
        {
            attempt++;
            Console.WriteLine("wifi: join attempt " + attempt);
            if (Wifi.Connect("", ""))
            {
                Console.WriteLine("wifi: '" + Wifi.GetSsid() + "' at " + Wifi.GetIp());
                break;
            }
            RustNet.Threading.Sleep.Ms(5000);
        }

        while (true)
        {
            RustNet.Threading.Sleep.Ms(1000);
        }
    }
}
