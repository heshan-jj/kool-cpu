# Build dist\kool-cpu-setup.exe (bundles kool-cpu-gui.exe, kool-cpu.exe, and PawnIO_setup.exe).
$ErrorActionPreference = "Stop"

$Root = (Resolve-Path "$PSScriptRoot\..\..").Path
$Stage = Join-Path $Root "target\installer"
$PawnIoUrl = "https://github.com/namazso/PawnIO.Setup/releases/download/2.2.0/PawnIO_setup.exe"
$PawnIoExe = Join-Path $Stage "PawnIO_setup.exe"

# Locate Inno Setup compiler (ISCC.exe)
$Iscc = @(
    (Get-Command iscc -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source),
    "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    "C:\Program Files\Inno Setup 6\ISCC.exe",
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1

if (-not $Iscc) {
    throw "Inno Setup 6 (ISCC.exe) not found. Install it with: winget install JRSoftware.InnoSetup"
}

# Build release binaries if missing
Push-Location $Root
try {
    if (-not (Test-Path "target\release\kool-cpu-gui.exe") -or -not (Test-Path "target\release\kool-cpu.exe")) {
        cargo build --release --workspace
        if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }
    }

    New-Item -ItemType Directory -Force $Stage | Out-Null
    if (-not (Test-Path $PawnIoExe)) {
        Write-Host "Downloading PawnIO_setup.exe (v2.2.0)..."
        Invoke-WebRequest -Uri $PawnIoUrl -OutFile $PawnIoExe -Headers @{"User-Agent"="kool-cpu"}
    }

    $Version = (Select-String -Path "Cargo.toml" -Pattern 'version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
    Write-Host "Compiling kool-cpu-setup.exe (v$Version) with $Iscc..."
    & $Iscc "/DAppVersion=$Version" "packaging\windows\kool-cpu.iss"
    if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

    Write-Host "Built: $Root\dist\kool-cpu-setup.exe"
} finally {
    Pop-Location
}
