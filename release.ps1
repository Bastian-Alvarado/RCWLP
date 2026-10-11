# Builds the files for a release into dist\:
#   RCWLP-host-setup-<version>.exe   the PC installer (made by installer\build.ps1)
#   RCWLP-<version>.apk              the phone app, signed with the release key
#   SHA256SUMS.txt                   checksums of both
#
#   powershell -ExecutionPolicy Bypass -File release.ps1
#
# Needs what installer\build.ps1 needs, plus JDK 17 or newer in JAVA_HOME and the signing
# key named in android\keystore.properties (see "Making a release" in the README).
$ErrorActionPreference = "Stop"
$root = $PSScriptRoot
$dist = Join-Path $root "dist"

if (-not (Test-Path (Join-Path $root "android\keystore.properties"))) {
    throw "android\keystore.properties is missing, so the app can't be signed. See ""Making a release"" in the README."
}

& (Join-Path $root "installer\build.ps1") | Out-Null
$hostVersion = (Select-String -Path (Join-Path $root "host\Cargo.toml") -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value

Push-Location (Join-Path $root "android")
try {
    .\gradlew.bat --console=plain assembleRelease
    if ($LASTEXITCODE) { throw "Gradle build failed" }
} finally {
    Pop-Location
}
$appVersion = (Select-String -Path (Join-Path $root "android\app\build.gradle.kts") -Pattern 'versionName = "(.+)"').Matches[0].Groups[1].Value

New-Item -ItemType Directory -Force $dist | Out-Null
$files = @(
    @{ From = "installer\output\RCWLP-host-setup-$hostVersion.exe"; To = "RCWLP-host-setup-$hostVersion.exe" },
    @{ From = "android\app\build\outputs\apk\release\app-release.apk"; To = "RCWLP-$appVersion.apk" }
)
$sums = foreach ($f in $files) {
    $target = Join-Path $dist $f.To
    Copy-Item (Join-Path $root $f.From) $target -Force
    "{0}  {1}" -f (Get-FileHash $target -Algorithm SHA256).Hash.ToLower(), $f.To
}
$sums | Set-Content -Encoding ascii (Join-Path $dist "SHA256SUMS.txt")
Get-ChildItem $dist | Format-Table Name, Length -AutoSize
