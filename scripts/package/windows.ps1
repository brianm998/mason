# SPDX-License-Identifier: GPL-3.0-or-later
# Build mason-<ver>-x86_64-pc-windows-msvc.zip and mason-<ver>-x86_64-windows-setup.msi
# (docs/RELEASING.md). Usage: pwsh scripts/package/windows.ps1 [-Out dist] [-NoBuild] [-NoMsi]
# The MSI needs the WiX Toolset v3 (candle.exe, light.exe) on PATH or under $env:WIX\bin; the
# windows-2022 runners have it.
param([string]$Out = 'dist', [switch]$NoBuild, [switch]$NoMsi)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$target = 'x86_64-pc-windows-msvc'
$bins = 'mason', 'masond', 'step-host', 'mason-mcp', 'fake-claude'

# Same line scripts/release-version.sh reads.
$ver = $null; $in = $false
foreach ($l in Get-Content (Join-Path $root 'Cargo.toml')) {
  if ($l -match '^\[workspace\.package\]') { $in = $true; continue }
  if ($l -match '^\[') { $in = $false }
  if ($in -and $l -match '^version\s*=\s*"([^"]+)"') { $ver = $Matches[1]; break }
}
if (-not $ver) { throw 'no workspace version in Cargo.toml' }

if (-not $NoBuild) {
  # Static CRT: no VC++ redistributable is needed on the target machine.
  $env:RUSTFLAGS = (($env:RUSTFLAGS, '-C target-feature=+crt-static') -join ' ').Trim()
  Push-Location $root
  try {
    $a = @('build', '--release', '--target', $target)
    foreach ($b in $bins) { $a += '--bin'; $a += $b }
    & cargo @a
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
  } finally { Pop-Location }
}

$tdir = Join-Path $root "target\$target\release"
New-Item -ItemType Directory -Force $Out | Out-Null
$Out = (Resolve-Path $Out).Path
$work = Join-Path ([IO.Path]::GetTempPath()) ("mason-win-" + [guid]::NewGuid().ToString('N'))
$name = "mason-$ver-$target"
$stage = Join-Path $work $name
New-Item -ItemType Directory -Force $stage | Out-Null
try {
  foreach ($b in $bins) {
    $f = Join-Path $tdir "$b.exe"
    if (-not (Test-Path $f)) { throw "missing $f" }
    Copy-Item $f $stage
  }
  Copy-Item (Join-Path $root 'packaging\mason-askpass.cmd') $stage
  foreach ($d in 'LICENSE', 'LICENSE-ADDITIONAL-PERMISSION', 'README.md') {
    Copy-Item (Join-Path $root $d) $stage
  }

  $zip = Join-Path $Out "$name.zip"
  if (Test-Path $zip) { Remove-Item $zip }
  Compress-Archive -Path $stage -DestinationPath $zip   # one top-level folder, like the tarballs
  Write-Output $zip

  if (-not $NoMsi) {
    $wixBin = $null
    if ($env:WIX) { $wixBin = Join-Path $env:WIX 'bin' }
    $candle = Get-Command candle.exe -ErrorAction SilentlyContinue
    if ($candle) { $wixBin = Split-Path $candle.Source }
    if (-not $wixBin -or -not (Test-Path (Join-Path $wixBin 'candle.exe'))) { throw 'WiX v3 (candle.exe) not found' }
    $obj = Join-Path $work 'mason.wixobj'
    $msi = Join-Path $Out "mason-$ver-x86_64-windows-setup.msi"
    & (Join-Path $wixBin 'candle.exe') -nologo -arch x64 "-dVersion=$ver" "-dSourceDir=$stage" -out $obj (Join-Path $root 'packaging\windows\mason.wxs')
    if ($LASTEXITCODE -ne 0) { throw 'candle failed' }
    & (Join-Path $wixBin 'light.exe') -nologo -out $msi $obj
    if ($LASTEXITCODE -ne 0) { throw 'light failed' }
    Write-Output $msi
  }
} finally { Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue }
