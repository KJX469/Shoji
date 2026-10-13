# scripts/build-installer.ps1
# Builds the Shoji browser and packages it into dist/ShojiSetup.exe.

$ErrorActionPreference = "Stop"
Set-Location (Resolve-Path "$PSScriptRoot/..")

# Strip local machine paths from panic messages and symbols. The encoded form (0x1F-separated)
# keeps paths containing spaces intact.
$repo = (Get-Location).Path
$env:CARGO_ENCODED_RUSTFLAGS = @(
    "--remap-path-prefix=$repo=shoji",
    "--remap-path-prefix=$($repo.Replace('\', '/'))=shoji",
    "--remap-path-prefix=$env:USERPROFILE=~"
) -join [char]0x1F

Write-Host "[1/3] Building shoji.exe" -ForegroundColor Cyan
cargo build --release -p shoji
if ($LASTEXITCODE -ne 0) { throw "Browser build failed." }

Write-Host "[2/3] Embedding it in the installer" -ForegroundColor Cyan
New-Item -ItemType Directory -Force "crates/installer/assets" | Out-Null
Copy-Item "target/release/shoji.exe" "crates/installer/assets/shoji.exe" -Force
cargo build --release -p shoji-setup
if ($LASTEXITCODE -ne 0) { throw "Installer build failed." }

Write-Host "[3/3] Writing dist/ShojiSetup.exe" -ForegroundColor Cyan
New-Item -ItemType Directory -Force "dist" | Out-Null
Copy-Item "target/release/shoji-setup.exe" "dist/ShojiSetup.exe" -Force

$browser = (Get-Item "target/release/shoji.exe").Length / 1MB
$setup = (Get-Item "dist/ShojiSetup.exe").Length / 1MB
Write-Host ("Done. shoji.exe {0:N2} MB, ShojiSetup.exe {1:N2} MB" -f $browser, $setup) -ForegroundColor Green
