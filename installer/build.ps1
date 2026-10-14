# Builds the app and its installer: dist\UnofficialUniwillControlCenter-<version>-Setup.exe. Needs Inno Setup 6
# (winget install JRSoftware.InnoSetup).
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 isn't installed: winget install JRSoftware.InnoSetup" }

Push-Location $root
try {
    # The real build: not the demo one.
    Remove-Item Env:UUCC_NO_ADMIN, Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    & $iscc "/DAppVersion=$version" "$PSScriptRoot\UnofficialUniwillControlCenter.iss"
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed" }
    Write-Output "Built dist\UnofficialUniwillControlCenter-$version-Setup.exe"
} finally {
    Pop-Location
}
