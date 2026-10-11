# Builds the RCWLP host installer: compiles the host in release mode, then runs Inno Setup.
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1
# Needs Rust and Inno Setup 6 (winget install JRSoftware.InnoSetup).
# The installer lands in installer\output\.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

Push-Location (Join-Path $root "host")
try {
    cargo build --release --bins
    if ($LASTEXITCODE) { throw "cargo build failed" }
} finally {
    Pop-Location
}

$version = (Select-String -Path (Join-Path $root "host\Cargo.toml") -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup" }

& $iscc /Q "/DAppVersion=$version" (Join-Path $PSScriptRoot "rcwlp-host.iss")
if ($LASTEXITCODE) { throw "Inno Setup failed" }
Get-Item (Join-Path $PSScriptRoot "output\RCWLP-host-setup-$version.exe")
