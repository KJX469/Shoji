<#
 Byobu upstream watch. Lists and scriptlets update themselves at runtime; this script covers
 the parts that need a rebuild: the matcher crate and upstream release notes.

   pwsh scripts/byobu-sync.ps1            # report what changed, update the adblock crate, run tests
   pwsh scripts/byobu-sync.ps1 -ReportOnly
#>
param([switch]$ReportOnly)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$pinFile = Join-Path $root 'crates/byobu/UPSTREAM.json'
$pins = Get-Content $pinFile -Raw | ConvertFrom-Json
$headers = @{ 'User-Agent' = 'shoji-byobu-sync' }
if ($env:GITHUB_TOKEN) { $headers.Authorization = "Bearer $env:GITHUB_TOKEN" }

$changed = $false
foreach ($repo in @($pins.watch.PSObject.Properties.Name)) {
    $latest = $null
    try {
        $rel = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -Headers $headers
        $latest = $rel.tag_name
    } catch {
        # Repos without releases (uAssets, adblock-resources): use the newest commit.
        $c = Invoke-RestMethod "https://api.github.com/repos/$repo/commits?per_page=1" -Headers $headers
        $latest = $c[0].sha.Substring(0, 10)
    }
    $old = $pins.watch.$repo
    if ($old -ne $latest) {
        Write-Host ("{0,-28} {1} -> {2}" -f $repo, ($old ?? '(unset)'), $latest) -ForegroundColor Yellow
        Write-Host "  https://github.com/$repo/compare/$old...$latest"
        $pins.watch.$repo = $latest
        $changed = $true
    } else {
        Write-Host ("{0,-28} {1} (unchanged)" -f $repo, $latest)
    }
}

if (-not $ReportOnly) {
    Push-Location $root
    try {
        cargo update -p adblock
        cargo test -p shoji-byobu
        $v = (cargo tree -p shoji-byobu --depth 1 | Select-String 'adblock v(\S+)').Matches[0].Groups[1].Value
        if ($pins.adblock -ne $v) { Write-Host "adblock crate: $($pins.adblock) -> $v" -ForegroundColor Yellow; $pins.adblock = $v; $changed = $true }
    } finally { Pop-Location }
    if ($changed) { $pins | ConvertTo-Json -Depth 4 | Set-Content $pinFile -Encoding utf8 }
}
if ($changed) { Write-Host "`nUpstream moved. Review the compare links, rebuild and re-test." -ForegroundColor Cyan }
