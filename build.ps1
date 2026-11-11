# Builds Voxa Server (+ Voxa Server Manager) and its MSI installer into .\dist
#   .\build.ps1             full build
#   .\build.ps1 -SkipCargo  only re-package the MSI from the last build
param([switch]$SkipCargo)

# ---- product configuration ----------------------------------------------------------
$Product = @{
    Packages = @('voxa-server', 'voxa-server-manager')
    Wxs      = 'server.wxs'
    Exts     = @('WixUIExtension', 'WixUtilExtension', 'WixFirewallExtension')
    Msi      = 'Voxa-Server-{0}-x64.msi'
    CopyTo   = '..\Server'      # optional local copy (ignored if the folder does not exist)
}
# -------------------------------------------------------------------------------------

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$Root = $PSScriptRoot
$Tools = Join-Path $env:LOCALAPPDATA 'voxa-build-tools'
$Toolchain = 'stable-x86_64-pc-windows-gnullvm'
$Target = 'x86_64-pc-windows-gnullvm'
New-Item -ItemType Directory -Force $Tools | Out-Null

function Get-GitHubAsset([string]$repo, [string]$pattern, [string]$dest) {
    $headers = @{}
    if ($env:GITHUB_TOKEN) { $headers['Authorization'] = "Bearer $env:GITHUB_TOKEN" }
    $rel = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -Headers $headers
    $asset = $rel.assets | Where-Object { $_.name -match $pattern } | Select-Object -First 1
    if (-not $asset) { throw "No release asset matching $pattern in $repo" }
    $zip = Join-Path $Tools $asset.name
    Write-Host "Downloading $($asset.name)..."
    Invoke-WebRequest $asset.browser_download_url -OutFile $zip
    Expand-Archive $zip -DestinationPath $dest -Force
    Remove-Item $zip
}

# --- LLVM-MinGW (linker, resource compiler) ---------------------------------------
if (-not (Get-Command x86_64-w64-mingw32-clang -ErrorAction SilentlyContinue)) {
    $llvm = Join-Path $Tools 'llvm-mingw'
    if (-not (Test-Path $llvm)) { Get-GitHubAsset 'mstorsjo/llvm-mingw' 'ucrt-x86_64\.zip$' $llvm }
    $bin = Get-ChildItem $llvm -Directory | Select-Object -First 1 | ForEach-Object { Join-Path $_.FullName 'bin' }
    $env:PATH = "$bin;$env:PATH"
}

# --- WiX Toolset 3.14 ---------------------------------------------------------------
$wix = if ($env:WIX -and (Test-Path (Join-Path $env:WIX 'bin\candle.exe'))) { Join-Path $env:WIX 'bin' } else { Join-Path $Tools 'wix' }
if (-not (Test-Path (Join-Path $wix 'candle.exe'))) { Get-GitHubAsset 'wixtoolset/wix3' 'binaries\.zip$' $wix }

# --- Rust ---------------------------------------------------------------------------
if (-not (rustup toolchain list | Select-String -SimpleMatch $Toolchain)) {
    rustup toolchain install $Toolchain --profile minimal
}
if (-not $env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR = Join-Path $env:LOCALAPPDATA "voxa-build-target\$(Split-Path $Root -Leaf)" }
Push-Location $Root
try {
    if (-not $SkipCargo) {
        $pkgArgs = $Product.Packages | ForEach-Object { '-p'; $_ }
        & cargo "+$Toolchain" build --release --locked @pkgArgs
        if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
    }
    $bin = Join-Path $env:CARGO_TARGET_DIR 'release'
    $version = (Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value

    # --- Documentation & third-party notices ------------------------------------------
    $stage = Join-Path $env:TEMP "voxa-msi-stage-$(Split-Path $Root -Leaf)"
    Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stage | Out-Null
    Copy-Item (Join-Path $Root 'README.md') (Join-Path $stage 'README.txt')
    Copy-Item (Join-Path $Root 'LICENSE-MIT') (Join-Path $stage 'LICENSE-MIT.txt')
    Copy-Item (Join-Path $Root 'LICENSE-APACHE') (Join-Path $stage 'LICENSE-APACHE.txt')

    Write-Host 'Collecting third-party licenses...'
    $meta = (& cargo "+$Toolchain" metadata --format-version 1 --filter-platform $Target) | Out-String | ConvertFrom-Json
    $used = @{}
    foreach ($n in $meta.resolve.nodes) { $used[$n.id] = $true }
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.AppendLine("Voxa $version - third-party software notices")
    [void]$sb.AppendLine("Voxa includes the following open source components. Their license texts follow.")
    [void]$sb.AppendLine()
    $seen = @{}
    $sha = [Security.Cryptography.SHA256]::Create()
    foreach ($p in ($meta.packages | Where-Object { $_.source -and $used[$_.id] } | Sort-Object name, version)) {
        [void]$sb.AppendLine('=' * 78)
        [void]$sb.AppendLine("$($p.name) $($p.version)  -  License: $($p.license)")
        if ($p.repository) { [void]$sb.AppendLine($p.repository) }
        [void]$sb.AppendLine('=' * 78)
        $dir = Split-Path $p.manifest_path
        $files = Get-ChildItem $dir -File | Where-Object { $_.Name -match '^(LICEN[CS]E|COPYING|NOTICE|COPYRIGHT)' }
        foreach ($f in $files) {
            $text = Get-Content $f.FullName -Raw
            $hash = [BitConverter]::ToString($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($text)))
            if ($seen.ContainsKey($hash)) {
                [void]$sb.AppendLine("[$($f.Name): identical to the text shown for $($seen[$hash])]")
            } else {
                $seen[$hash] = "$($p.name) ($($f.Name))"
                [void]$sb.AppendLine("--- $($f.Name) ---")
                [void]$sb.AppendLine($text.Trim())
            }
            [void]$sb.AppendLine()
        }
        [void]$sb.AppendLine()
    }
    [IO.File]::WriteAllText((Join-Path $stage 'THIRD-PARTY-NOTICES.txt'), $sb.ToString())

    # --- MSI package -------------------------------------------------------------------
    $dist = Join-Path $Root 'dist'
    New-Item -ItemType Directory -Force $dist | Out-Null
    $obj = Join-Path $env:TEMP "voxa-msi-obj-$(Split-Path $Root -Leaf)"
    New-Item -ItemType Directory -Force $obj | Out-Null
    $defs = @("-dVersion=$version", "-dBin=$bin", "-dAssets=$(Join-Path $Root 'assets')", "-dDocs=$stage")
    $exts = $Product.Exts | ForEach-Object { '-ext'; $_ }
    $wixobj = Join-Path $obj ($Product.Wxs -replace '\.wxs$', '.wixobj')
    $msi = Join-Path $dist ($Product.Msi -f $version)
    & (Join-Path $wix 'candle.exe') -nologo -arch x64 @defs @exts -out $wixobj (Join-Path $Root "installer\$($Product.Wxs)")
    if ($LASTEXITCODE -ne 0) { throw 'candle failed' }
    # ICE38/43/57 flag per-machine shortcuts with HKLM key paths (fine for ALLUSERS=1);
    # ICE61 warns about AllowSameVersionUpgrades, which is intended.
    & (Join-Path $wix 'light.exe') -nologo @exts -cultures:en-us -spdb -sice:ICE38 -sice:ICE43 -sice:ICE57 -sice:ICE61 -out $msi $wixobj
    if ($LASTEXITCODE -ne 0) { throw 'light failed' }
    Write-Host "Built $msi" -ForegroundColor Green
    $copy = Join-Path $Root $Product.CopyTo
    if ($Product.CopyTo -and (Test-Path $copy)) {
        Copy-Item $msi $copy -Force
        Write-Host "Copied to $((Resolve-Path $copy).Path)" -ForegroundColor Green
    }
} finally {
    Pop-Location
}
