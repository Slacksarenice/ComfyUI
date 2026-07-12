# Builds comfy-shell and packs it into an AppContainer MSIX.
#
# Produces rust/package-out/ComfyUI-Shell.msix (unsigned). To install
# locally, sign it with a dev certificate first:
#   New-SelfSignedCertificate -Type Custom -Subject "CN=ComfyUI Development" `
#     -KeyUsage DigitalSignature -FriendlyName "ComfyUI Dev" `
#     -CertStoreLocation "Cert:\CurrentUser\My" `
#     -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
#   signtool sign /fd SHA256 /a package-out\ComfyUI-Shell.msix
# then trust the certificate and Add-AppxPackage the msix.

$ErrorActionPreference = "Stop"

$rustRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$shellDir = Join-Path $rustRoot "crates\comfy-shell"
$stage = Join-Path $rustRoot "package-out\stage"
$outDir = Join-Path $rustRoot "package-out"

Push-Location $rustRoot
try {
    cargo build --release -p comfy-shell
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
} finally {
    Pop-Location
}

if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage, (Join-Path $stage "Assets") | Out-Null

Copy-Item (Join-Path $rustRoot "target\release\comfy-shell.exe") $stage
Copy-Item (Join-Path $shellDir "packaging\AppxManifest.xml") $stage

# Placeholder logos (solid color) until real branding assets are added.
Add-Type -AssemblyName System.Drawing
foreach ($asset in @(
    @{ name = "StoreLogo.png";          size = 50  },
    @{ name = "Square150x150Logo.png";  size = 150 },
    @{ name = "Square44x44Logo.png";    size = 44  }
)) {
    $bmp = New-Object System.Drawing.Bitmap($asset.size, $asset.size)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::FromArgb(255, 30, 30, 46))
    $g.Dispose()
    $bmp.Save((Join-Path $stage "Assets\$($asset.name)"), [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
}

$msix = Join-Path $outDir "ComfyUI-Shell.msix"
if (Test-Path $msix) { Remove-Item -Force $msix }

$makeappx = Get-Command makeappx -ErrorAction SilentlyContinue
if (-not $makeappx) {
    $kit = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $kit) { throw "makeappx.exe not found; install the Windows SDK" }
    $makeappx = $kit.FullName
} else {
    $makeappx = $makeappx.Source
}

& $makeappx pack /d $stage /p $msix /nv
if ($LASTEXITCODE -ne 0) { throw "makeappx failed" }

Write-Host "Packed: $msix"
