# Builds the RCWLP host installer: compiles the host in release mode, then runs Inno Setup.
#   powershell -ExecutionPolicy Bypass -File installer\build.ps1
# Needs Rust and Inno Setup 6 (winget install JRSoftware.InnoSetup).
# The installer lands in installer\output\.
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

# Runs a program and shows its output. Only its exit code decides failure: when output is
# redirected, Windows PowerShell would otherwise stop at the first line a program writes to
# stderr, and cargo writes its progress there.
function Invoke-Program([scriptblock]$Command, [string]$Failure) {
    $ErrorActionPreference = "Continue"
    & $Command 2>&1 | ForEach-Object { "$_" } | Out-Host
    if ($LASTEXITCODE) { throw $Failure }
}

# Rust records library source paths for error messages, and those sit in the user's folder.
# Rewrite them so the programs don't carry this PC's user name or folders.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$savedFlags = $env:CARGO_ENCODED_RUSTFLAGS
$env:CARGO_ENCODED_RUSTFLAGS = @("--remap-path-prefix=$cargoHome=cargo", "--remap-path-prefix=$root=rcwlp") -join [char]0x1f
Push-Location (Join-Path $root "host")
try {
    Invoke-Program { cargo build --release --bins } "cargo build failed"
} finally {
    Pop-Location
    $env:CARGO_ENCODED_RUSTFLAGS = $savedFlags
}

$version = (Select-String -Path (Join-Path $root "host\Cargo.toml") -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found. Install it with: winget install JRSoftware.InnoSetup" }

Invoke-Program { & $iscc /Q "/DAppVersion=$version" (Join-Path $PSScriptRoot "rcwlp-host.iss") } "Inno Setup failed"
Get-Item (Join-Path $PSScriptRoot "output\RCWLP-host-setup-$version.exe")
