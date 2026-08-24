# __NAME__

Weather check over WiFi + HTTP.

Test server (PowerShell, separate terminal):

```powershell
$listener = [System.Net.HttpListener]::new()
$listener.Prefixes.Add('http://127.0.0.1:8085/')
$listener.Start()
while ($true) {
  $ctx = $listener.GetContext()
  $bytes = [Text.Encoding]::UTF8.GetBytes('temp_c=27;condition=Sunny;city=Bandung')
  $ctx.Response.OutputStream.Write($bytes, 0, $bytes.Length)
  $ctx.Response.Close()
}
```
