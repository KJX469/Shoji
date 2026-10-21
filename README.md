<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
  <img src="docs/assets/banner-light.svg" alt="Shoji" width="100%">
</picture>

**障子** · A small, quiet browser for Windows.

[**Download**](../../releases/latest)

</div>

<p align="center">
  <img src="docs/assets/browser-dark.png" width="49%" alt="Shoji, dark theme">
  <img src="docs/assets/browser-light.png" width="49%" alt="Shoji, light theme">
</p>

Shoji runs on the WebView2 engine Windows already ships and keeps updated. It blocks ads and
trackers, darkens websites, and has no account, sync or telemetry.

## Quick setup

In PowerShell on Windows 10 or 11:

```powershell
iwr https://github.com/KJX469/Shoji/releases/latest/download/ShojiSetup.exe -OutFile $env:TEMP\ShojiSetup.exe; & $env:TEMP\ShojiSetup.exe
```

Official builds come only from [Releases](../../releases) and update themselves. Builds from
anywhere else are unsupported.

## License

Code is [MIT](LICENSE). The Shoji name and crane logo are not, so builds made by others must not use
them. Third-party components: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
