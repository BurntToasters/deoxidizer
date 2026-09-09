#Requires -Version 5.1
<#
.SYNOPSIS
    Quick installer for deoxidizer on Windows.
.DESCRIPTION
    Downloads the latest deoxidizer release from GitHub and installs it
    to %LOCALAPPDATA%\Programs\deoxidizer, then adds it to the user PATH.

    Usage:
      irm https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/install.ps1 | iex
#>
[CmdletBinding()]
param(
    [switch]$FromSource
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Repo = 'BurntToasters/deoxidizer'
$ReleaseKeyFingerprint = 'CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F'
$ExpectedWindowsPublisher = 'BurntToasters'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\deoxidizer'

Write-Host '🔧 deoxidizer installer for Windows' -ForegroundColor Cyan
Write-Host ('─' * 40)

$stagingRoot = Join-Path ([IO.Path]::GetTempPath()) ("deoxidizer-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stagingRoot -Force | Out-Null
$stagingExtract = Join-Path $stagingRoot 'extracted'
New-Item -ItemType Directory -Path $stagingExtract -Force | Out-Null

try {
    if ($FromSource) {
        $targetRoot = if ($env:CARGO_TARGET_DIR) {
            $env:CARGO_TARGET_DIR
        } else {
            Join-Path $PSScriptRoot 'target'
        }
        $localBuild = Join-Path $targetRoot 'release\deoxidizer.exe'
        $localDeox = Join-Path $targetRoot 'release\deox.exe'
        if (-not (Test-Path $localBuild) -or -not (Test-Path $localDeox)) {
            Push-Location $PSScriptRoot
            try {
                & cargo build --release --locked
            } finally {
                Pop-Location
            }
        }
        Copy-Item $localBuild (Join-Path $stagingRoot 'deoxidizer.exe') -Force
        Copy-Item $localDeox (Join-Path $stagingRoot 'deox.exe') -Force
    } else {
        Write-Host 'Downloading latest release from GitHub...'
        $ProgressPreference = 'SilentlyContinue'
        $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest"
        $version = $release.tag_name -replace '^v',''
        $processArch = if ($env:PROCESSOR_ARCHITEW6432) {
            $env:PROCESSOR_ARCHITEW6432
        } else {
            $env:PROCESSOR_ARCHITECTURE
        }
        $arch = switch ($processArch.ToUpperInvariant()) {
            'ARM64' { 'aarch64'; break }
            'AMD64' { 'x86_64'; break }
            default { throw "Unsupported Windows architecture: $processArch" }
        }
        $assetName = "deoxidizer-v$version-windows-$arch.zip"
        $asset = $release.assets | Where-Object { $_.name -eq $assetName }
        if (-not $asset) { throw "Release asset not found: $assetName" }
        if (-not $asset.browser_download_url.StartsWith("https://github.com/$Repo/releases/download/")) {
            throw "Release asset URL is not an expected GitHub URL"
        }

        Write-Host "  Version: v$version"
        Write-Host "  Asset:   $assetName"

        $tmpZip = Join-Path $stagingRoot $assetName
        $checksumName = "SHA256SUMS-windows-$arch.txt"
        $checksumPath = Join-Path $stagingRoot $checksumName
        Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $tmpZip
        try {
            Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$($release.tag_name)/$checksumName" -OutFile $checksumPath
        } catch {
            $checksumName = 'SHA256SUMS.txt'
            $checksumPath = Join-Path $stagingRoot $checksumName
            Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$($release.tag_name)/$checksumName" -OutFile $checksumPath
        }
        $gpg = Get-Command gpg.exe -ErrorAction SilentlyContinue
        if (-not $gpg) { throw 'gpg.exe is required to authenticate release manifests' }
        $keyPath = Join-Path $stagingRoot 'release-signing-key.asc'
        $keyringPath = Join-Path $stagingRoot 'release-keyring.gpg'
        $signaturePath = "$checksumPath.asc"
        Invoke-WebRequest -Uri "https://raw.githubusercontent.com/$Repo/main/release-signing-key.asc" -OutFile $keyPath
        $fingerprint = (& $gpg.Source --batch --show-keys --with-colons $keyPath |
            Where-Object { $_ -like 'fpr:*' } |
            Select-Object -First 1).Split(':')[9].ToUpperInvariant()
        if ($fingerprint -ne $ReleaseKeyFingerprint) { throw 'release signing key fingerprint mismatch' }
        & $gpg.Source --batch --yes --dearmor --output $keyringPath $keyPath
        if ($LASTEXITCODE -ne 0) { throw 'cannot load release signing key' }
        Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$($release.tag_name)/$checksumName.asc" -OutFile $signaturePath
        & $gpg.Source --batch --no-options --no-default-keyring --keyring $keyringPath --verify $signaturePath $checksumPath *> $null
        if ($LASTEXITCODE -ne 0) { throw 'checksum manifest signature verification failed' }
        $escapedAsset = [Regex]::Escape($assetName)
        $checksumLines = @(Get-Content -LiteralPath $checksumPath |
            Where-Object { $_ -match "^\s*([0-9A-Fa-f]{64})\s+\*?$escapedAsset\s*$" } |
            Select-Object)
        if ($checksumLines.Count -ne 1) {
            throw "$checksumName must contain exactly one entry for $assetName"
        }
        $checksumLine = $checksumLines[0]
        $expectedHash = [Regex]::Match($checksumLine, '^\s*([0-9A-Fa-f]{64})').Groups[1].Value.ToUpperInvariant()
        $actualHash = (Get-FileHash -LiteralPath $tmpZip -Algorithm SHA256).Hash.ToUpperInvariant()
        if ($actualHash -ne $expectedHash) {
            throw "SHA256 checksum verification failed for $assetName"
        }

        Expand-Archive -Path $tmpZip -DestinationPath $stagingExtract -Force
        Copy-Item (Join-Path $stagingExtract 'deoxidizer.exe') (Join-Path $stagingRoot 'deoxidizer.exe') -Force
        Copy-Item (Join-Path $stagingExtract 'deox.exe') (Join-Path $stagingRoot 'deox.exe') -Force
    }

    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }
    foreach ($binary in @('deoxidizer.exe', 'deox.exe')) {
        $staged = Join-Path $stagingRoot $binary
        if (-not (Test-Path $staged)) { throw "Staged binary missing: $binary" }
        $stagedItem = Get-Item -LiteralPath $staged
        if (($stagedItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Staged binary is a reparse point: $binary"
        }
        if (-not $FromSource) {
            $signature = Get-AuthenticodeSignature -LiteralPath $staged
            if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
                throw "Invalid or missing Authenticode signature: $binary"
            }
            if (-not $signature.SignerCertificate) { throw "Missing Authenticode signer certificate: $binary" }
            $publisher = $signature.SignerCertificate.GetNameInfo(
                [System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName,
                $false
            )
            if ($publisher -ne $ExpectedWindowsPublisher) {
                throw "Unexpected Authenticode publisher for $binary: $publisher"
            }
        }
        $destination = Join-Path $InstallDir $binary
        if (Test-Path -LiteralPath $destination) {
            $destinationItem = Get-Item -LiteralPath $destination -Force
            if (($destinationItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Install destination is a reparse point: $binary"
            }
        }
        Move-Item -LiteralPath $staged -Destination $destination -Force
    }
} finally {
    if (Test-Path $stagingRoot) {
        Remove-Item -LiteralPath $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "✓ Installed to $InstallDir" -ForegroundColor Green

# Add to PATH if not already there
$currentPath = [Environment]::GetEnvironmentVariable('PATH', 'User')
$pathEntries = @($currentPath -split ';' | Where-Object { $_ })
if (-not ($pathEntries | Where-Object { $_.TrimEnd('\\') -ieq $InstallDir.TrimEnd('\\') })) {
    $newPath = (@($InstallDir) + $pathEntries) -join ';'
    [Environment]::SetEnvironmentVariable('PATH', $newPath, 'User')
    # Broadcast WM_SETTINGCHANGE so active shells pick up the change
    if (-not ([Management.Automation.PSTypeName]'Win32.NativeMethods').Type) {
        Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @'
using System;
using System.Runtime.InteropServices;
public static class NativeMethods {
    [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
    public static extern IntPtr SendMessageTimeout(
        IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
        uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
}
'@
    }
    $HWND_BROADCAST = [IntPtr]0xFFFF
    $WM_SETTINGCHANGE = 0x001A
    $result = [UIntPtr]::Zero
    [Win32.NativeMethods]::SendMessageTimeout($HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result) | Out-Null
    Write-Host '✓ Added to user PATH (new terminals will have deoxidizer available)' -ForegroundColor Green
} else {
    Write-Host '✓ Already in user PATH' -ForegroundColor Green
}

Write-Host ''
Write-Host "Run 'deox setup' to configure, or 'deox scan' to get started."
