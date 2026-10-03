#Requires -Version 5.1
<#
.SYNOPSIS
    Quick installer for deoxidizer on Windows.
.DESCRIPTION
    Downloads the latest deoxidizer release from GitHub and installs it
    to %LOCALAPPDATA%\Programs\deoxidizer, then adds it to the user PATH.

    Usage:
      irm https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/install.ps1 | iex

    Authenticity: both binaries must carry a valid Authenticode signature
    from the expected publisher, and the archive must match its entry in
    the release SHA256 manifest. When gpg.exe is available, the manifest's
    detached signature is also verified against the pinned release key.
#>
[CmdletBinding()]
param(
    [switch]$FromSource
)

# Run in a child scope so StrictMode and ErrorActionPreference never leak
# into the caller's session when invoked via `irm | iex`.
& {
    Set-StrictMode -Version Latest
    $ErrorActionPreference = 'Stop'

    $Repo = 'BurntToasters/deoxidizer'
    $ReleaseKeyFingerprint = 'CAEB45D4747E73FA11A9CBF7619A06F3F2FBC20F'
    # Must equal the Authenticode certificate's simple name (the
    # AZURE_ARTIFACT_SIGNING_PUBLISHER value used when signing releases).
    $ExpectedWindowsPublisher = 'BurntToasters'
    $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\deoxidizer'

    Write-Host 'deoxidizer installer for Windows' -ForegroundColor Cyan
    Write-Host ('-' * 40)

    $stagingRoot = Join-Path ([IO.Path]::GetTempPath()) ('deoxidizer-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stagingRoot -Force | Out-Null
    $stagingExtract = Join-Path $stagingRoot 'extracted'
    New-Item -ItemType Directory -Path $stagingExtract -Force | Out-Null

    function Invoke-DownloadWithRetry {
        param([Parameter(Mandatory = $true)][string]$Uri, [Parameter(Mandatory = $true)][string]$OutFile)
        $lastError = $null
        for ($attempt = 1; $attempt -le 3; $attempt++) {
            try {
                Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing -TimeoutSec 300
                return
            } catch {
                $lastError = $_
                if ($attempt -lt 3) { Start-Sleep -Seconds 2 }
            }
        }
        throw $lastError
    }

    function Test-ManifestSignature {
        param([string]$ChecksumPath, [string]$Tag, [string]$ChecksumName)
        $gpg = Get-Command gpg.exe -ErrorAction SilentlyContinue
        if (-not $gpg) {
            Write-Host '  gpg.exe not found; relying on Authenticode + SHA256 verification.'
            return
        }
        $keyPath = Join-Path $stagingRoot 'release-signing-key.asc'
        $keyringPath = Join-Path $stagingRoot 'release-keyring.gpg'
        $gnupgHome = Join-Path $stagingRoot 'gnupg'
        New-Item -ItemType Directory -Path $gnupgHome -Force | Out-Null
        $signaturePath = "$ChecksumPath.asc"
        Invoke-DownloadWithRetry -Uri "https://raw.githubusercontent.com/$Repo/main/release-signing-key.asc" -OutFile $keyPath
        $fingerprint = (& $gpg.Source --homedir $gnupgHome --batch --show-keys --with-colons $keyPath |
            Where-Object { $_ -like 'fpr:*' } |
            Select-Object -First 1).Split(':')[9].ToUpperInvariant()
        if ($fingerprint -ne $ReleaseKeyFingerprint) { throw 'release signing key fingerprint mismatch' }
        & $gpg.Source --homedir $gnupgHome --batch --yes --dearmor --output $keyringPath $keyPath
        if ($LASTEXITCODE -ne 0) { throw 'cannot load release signing key' }
        Invoke-DownloadWithRetry -Uri "https://github.com/$Repo/releases/download/$Tag/$ChecksumName.asc" -OutFile $signaturePath
        $status = & $gpg.Source --homedir $gnupgHome --batch --no-options --no-default-keyring --keyring $keyringPath --status-fd 1 --verify $signaturePath $ChecksumPath 2> $null
        if ($LASTEXITCODE -ne 0 -or -not ($status -match "^\[GNUPG:\] VALIDSIG $ReleaseKeyFingerprint ")) {
            throw 'checksum manifest signature verification failed'
        }
        Write-Host '  Checksum manifest signature verified.'
    }

    function Set-UserPath {
        # Read and write the raw registry value so %VARIABLE% entries and the
        # REG_EXPAND_SZ type survive (Environment.SetEnvironmentVariable
        # would flatten them).
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        try {
            $raw = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $entries = @($raw -split ';' | Where-Object { $_ })
            if ($entries | Where-Object { $_.TrimEnd('\') -ieq $InstallDir.TrimEnd('\') }) {
                Write-Host 'Already in user PATH' -ForegroundColor Green
                return
            }
            $key.SetValue('Path', ((@($InstallDir) + $entries) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
        } finally {
            $key.Close()
        }
        if (-not ([Management.Automation.PSTypeName]'Win32.NativeMethods').Type) {
            Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(
    IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
        }
        $result = [UIntPtr]::Zero
        [Win32.NativeMethods]::SendMessageTimeout([IntPtr]0xFFFF, 0x001A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result) | Out-Null
        Write-Host 'Added to user PATH (new terminals will have deoxidizer available)' -ForegroundColor Green
    }

    try {
        if ($FromSource) {
            $repoRoot = if ($PSScriptRoot) { $PSScriptRoot } else { (Get-Location).Path }
            $targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repoRoot 'target' }
            Push-Location $repoRoot
            try {
                & cargo build --release --locked
                if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
            } finally {
                Pop-Location
            }
            Copy-Item (Join-Path $targetRoot 'release\deoxidizer.exe') (Join-Path $stagingRoot 'deoxidizer.exe') -Force
            Copy-Item (Join-Path $targetRoot 'release\deox.exe') (Join-Path $stagingRoot 'deox.exe') -Force
        } else {
            Write-Host 'Downloading latest release from GitHub...'
            $ProgressPreference = 'SilentlyContinue'
            $release = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases/latest" -TimeoutSec 300
            $version = $release.tag_name -replace '^v', ''
            # Validate before interpolating into asset names/URLs.
            if ($version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-(alpha|beta|rc)\.(0|[1-9][0-9]*))?$') {
                throw "Invalid release version: $version"
            }
            $processArch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
            $arch = switch ($processArch.ToUpperInvariant()) {
                'ARM64' { 'aarch64'; break }
                'AMD64' { 'x86_64'; break }
                default { throw "Unsupported Windows architecture: $processArch" }
            }
            $assetName = "deoxidizer-v$version-windows-$arch.zip"
            $asset = $release.assets | Where-Object { $_.name -eq $assetName }
            if (-not $asset) { throw "Release asset not found: $assetName" }
            if (-not $asset.browser_download_url.StartsWith("https://github.com/$Repo/releases/download/")) {
                throw 'Release asset URL is not an expected GitHub URL'
            }

            Write-Host "  Version: v$version"
            Write-Host "  Asset:   $assetName"

            $tmpZip = Join-Path $stagingRoot $assetName
            $checksumName = "SHA256SUMS-windows-$arch.txt"
            $checksumPath = Join-Path $stagingRoot $checksumName
            Invoke-DownloadWithRetry -Uri $asset.browser_download_url -OutFile $tmpZip
            try {
                Invoke-DownloadWithRetry -Uri "https://github.com/$Repo/releases/download/$($release.tag_name)/$checksumName" -OutFile $checksumPath
            } catch {
                $checksumName = 'SHA256SUMS.txt'
                $checksumPath = Join-Path $stagingRoot $checksumName
                Invoke-DownloadWithRetry -Uri "https://github.com/$Repo/releases/download/$($release.tag_name)/$checksumName" -OutFile $checksumPath
            }
            Test-ManifestSignature -ChecksumPath $checksumPath -Tag $release.tag_name -ChecksumName $checksumName

            $escapedAsset = [Regex]::Escape($assetName)
            $checksumLines = @(Get-Content -LiteralPath $checksumPath |
                Where-Object { $_ -match "^\s*([0-9A-Fa-f]{64})\s+\*?$escapedAsset\s*$" })
            if ($checksumLines.Count -ne 1) {
                throw "$checksumName must contain exactly one entry for $assetName"
            }
            $expectedHash = [Regex]::Match($checksumLines[0], '^\s*([0-9A-Fa-f]{64})').Groups[1].Value.ToUpperInvariant()
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
                    throw "Unexpected Authenticode publisher for ${binary}: $publisher"
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

    Write-Host "Installed to $InstallDir" -ForegroundColor Green
    Set-UserPath
    Write-Host ''
    Write-Host "Run 'deox setup' to configure, or 'deox scan' to get started."
}
