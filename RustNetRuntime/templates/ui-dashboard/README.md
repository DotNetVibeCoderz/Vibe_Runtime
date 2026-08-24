# __NAME__ — XML UI dashboard

Loads a **WPF/Glide-style UI** from an XML layout on the device
filesystem (`/data/ui.xml`, seeded on first boot), binds it to live ADC
readings and renders to the device display. Edit the XML on-device
(`rustnet data push`) to change the layout without reflashing.

```bash
dotnet build
rustnet flash bin/Debug/net10.0/__NAME__.dll --name dash --key <your.key> --start
rustnet display capture -o dash.ppm     # or watch in the VSCode simulator panel
```
