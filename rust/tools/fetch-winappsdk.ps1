# Downloads the Windows App SDK component packages that carry the WinRT
# metadata (.winmd) needed by comfy-shell's build script, and the runtime
# DLLs needed to run unpackaged. Everything lands in
# crates/comfy-shell/.winappsdk/ which is gitignored.
#
# Versions match the Microsoft.WindowsAppSDK 2.2.0 meta package.

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$packages = @(
    @{ id = "microsoft.windowsappsdk.base";                   version = "2.0.4"  },
    @{ id = "microsoft.windowsappsdk.foundation";             version = "2.1.0"  },
    @{ id = "microsoft.windowsappsdk.interactiveexperiences"; version = "2.0.15" },
    @{ id = "microsoft.windowsappsdk.winui";                  version = "2.2.1"  },
    @{ id = "microsoft.web.webview2";                         version = "1.0.4078.44" }
)

$root = Join-Path $PSScriptRoot "..\crates\comfy-shell\.winappsdk"
$dl = Join-Path $root "downloads"
New-Item -ItemType Directory -Force $root, $dl | Out-Null

Add-Type -AssemblyName System.IO.Compression.FileSystem

foreach ($pkg in $packages) {
    $name = "$($pkg.id).$($pkg.version)"
    $nupkg = Join-Path $dl "$name.nupkg"
    $extracted = Join-Path $root $name
    if (Test-Path $extracted) {
        Write-Host "Already extracted: $name"
        continue
    }
    if (-not (Test-Path $nupkg)) {
        $url = "https://api.nuget.org/v3-flatcontainer/$($pkg.id)/$($pkg.version)/$name.nupkg"
        Write-Host "Downloading $url"
        Invoke-WebRequest -Uri $url -OutFile $nupkg -UseBasicParsing
    }
    Write-Host "Extracting $name"
    [System.IO.Compression.ZipFile]::ExtractToDirectory($nupkg, $extracted)
}

# Collect all winmds into one folder for windows-bindgen. For packages that
# ship per-contract-version metadata folders, prefer the newest (18362).
$winmd = Join-Path $root "winmd"
New-Item -ItemType Directory -Force $winmd | Out-Null
Get-ChildItem -Path $root -Recurse -Filter *.winmd |
    Where-Object { $_.FullName -notlike "*\winmd\*" -and $_.FullName -notlike "*\10.0.17763.0\*" } |
    ForEach-Object { Copy-Item $_.FullName (Join-Path $winmd $_.Name) -Force }

Write-Host "winmds collected in $winmd :"
Get-ChildItem $winmd | Select-Object -ExpandProperty Name
