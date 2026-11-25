#Requires -Version 5.1
<#
.SYNOPSIS
    Quick installer for deoxidizer on Windows.
.DESCRIPTION
    Downloads the latest deoxidizer release from GitHub and installs it
    to %LOCALAPPDATA%\Programs\deoxidizer, then adds it to the user PATH.

    Usage:
      irm https://raw.githubusercontent.com/BurntToasters/deoxidizer/main/install.ps1 | iex

    Authenticity: the raw Minisign signature over the SHA256 manifest is
    verified with the pinned public key, both binaries must carry a valid
    Authenticode signature from the expected publisher, and the archive must
    match its entry in the release manifest.
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
    $ReleaseMinisignPublicKey = "untrusted comment: minisign public key: 083B254A48C428E3`nRWTjKMRISiU7CDbPKID/yjE1xQvWSbhYFjDF3xgaJCAqOiO7IPffxndk`n"
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
        $minisign = Get-Command minisign.exe -ErrorAction SilentlyContinue
        if (-not $minisign) { throw 'minisign.exe is required to authenticate the release' }
        $keyPath = Join-Path $stagingRoot 'release-signing-key.pub'
        $signaturePath = "$ChecksumPath.minisig"
        [IO.File]::WriteAllText($keyPath, $ReleaseMinisignPublicKey, [Text.UTF8Encoding]::new($false))
        Invoke-DownloadWithRetry -Uri "https://github.com/$Repo/releases/download/$Tag/$ChecksumName.minisig" -OutFile $signaturePath
        & $minisign.Source -V -m $ChecksumPath -x $signaturePath -p $keyPath
        if ($LASTEXITCODE -ne 0) { throw 'checksum manifest signature verification failed' }
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

    function Get-RealFileState {
        param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Label)
        try {
            $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
        } catch {
            if ($_.CategoryInfo.Category -eq [System.Management.Automation.ErrorCategory]::ObjectNotFound) {
                return $false
            }
            throw
        }
        if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "$Label is a reparse point: $Path"
        }
        if ($item.PSIsContainer) {
            throw "$Label is a directory: $Path"
        }
        return $true
    }

    function Get-FileDigest {
        param([Parameter(Mandatory = $true)][string]$Path)
        return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant()
    }

    function Write-AtomicRecoveryState {
        param([Parameter(Mandatory = $true)][string]$RecoveryDir, [Parameter(Mandatory = $true)]$State)
        $statePath = Join-Path $RecoveryDir 'state.json'
        $tempPath = Join-Path $RecoveryDir ('state.' + [Guid]::NewGuid().ToString('N') + '.tmp')
        $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($State | ConvertTo-Json -Compress))
        $stream = [IO.File]::Open($tempPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try {
            $stream.Write($bytes, 0, $bytes.Length)
            $stream.Flush($true)
        } finally {
            $stream.Dispose()
        }
        if ([IO.File]::Exists($statePath)) {
            [IO.File]::Replace($tempPath, $statePath, $null)
        } else {
            [IO.File]::Move($tempPath, $statePath)
        }
    }

    function Recover-InstallTransaction {
        $recoveryPath = Join-Path $InstallDir '.deoxidizer-install-recovery'
        try {
            $recovery = Get-Item -LiteralPath $recoveryPath -Force -ErrorAction Stop
        } catch {
            if ($_.CategoryInfo.Category -eq [System.Management.Automation.ErrorCategory]::ObjectNotFound) {
                return
            }
            throw
        }
        if (($recovery.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or -not $recovery.PSIsContainer) {
            throw "Install recovery path is not a real directory: $recoveryPath"
        }
        $statePath = Join-Path $recoveryPath 'state.json'
        if (-not (Get-RealFileState -Path $statePath -Label 'Install recovery state')) {
            throw 'Install recovery state is missing; refusing to alter installed binaries'
        }
        $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
        if ($state.schema_version -ne 1 -or $state.phase -notin @('prepared', 'committed')) {
            throw 'Install recovery state is malformed; refusing to alter installed binaries'
        }
        if ($state.phase -eq 'committed') {
            Remove-Item -LiteralPath $recoveryPath -Recurse -Force
            return
        }

        foreach ($binary in @('deoxidizer.exe', 'deox.exe')) {
            $hadOriginal = if ($binary -eq 'deoxidizer.exe') { [bool]$state.hadDeoxidizer } else { [bool]$state.hadDeox }
            $expectedNewHash = if ($binary -eq 'deoxidizer.exe') { [string]$state.newDeoxidizerHash } else { [string]$state.newDeoxHash }
            $expectedOldHash = if ($binary -eq 'deoxidizer.exe') { [string]$state.oldDeoxidizerHash } else { [string]$state.oldDeoxHash }
            if ($expectedNewHash -notmatch '^[A-Fa-f0-9]{64}$' -or ($hadOriginal -and $expectedOldHash -notmatch '^[A-Fa-f0-9]{64}$')) {
                throw "Install recovery hashes are malformed: $binary"
            }
            $destination = Join-Path $InstallDir $binary
            $destinationExists = Get-RealFileState -Path $destination -Label 'Install destination'
            if ($hadOriginal) {
                $backup = Join-Path $recoveryPath ($binary + '.backup')
                if (-not (Get-RealFileState -Path $backup -Label 'Install recovery backup')) {
                    throw "Install recovery backup is missing: $binary"
                }
                if ((Get-FileDigest -Path $backup) -ne $expectedOldHash) {
                    throw "Install recovery backup hash mismatch: $binary"
                }
                if ($destinationExists) {
                    $currentHash = Get-FileDigest -Path $destination
                    if ($currentHash -eq $expectedOldHash) { continue }
                    if ($currentHash -ne $expectedNewHash) {
                        throw "Install destination changed during recovery: $binary"
                    }
                }
                $restore = Join-Path $InstallDir ('.deoxidizer-restore-' + [Guid]::NewGuid().ToString('N'))
                [IO.File]::Copy($backup, $restore, $false)
                if ($destinationExists) {
                    [IO.File]::Replace($restore, $destination, $null)
                } else {
                    [IO.File]::Move($restore, $destination)
                }
            } elseif ($destinationExists) {
                if ((Get-FileDigest -Path $destination) -ne $expectedNewHash) {
                    throw "Unexpected file appeared at install destination during recovery: $binary"
                }
                [IO.File]::Delete($destination)
            }
        }
        Remove-Item -LiteralPath $recoveryPath -Recurse -Force
    }

    function Install-BinaryPair {
        param([Parameter(Mandatory = $true)][string]$StagingRoot)
        $binaryNames = @('deoxidizer.exe', 'deox.exe')
        $destinations = @{}
        $hadOriginal = @{}
        $oldHashes = @{}
        $newHashes = @{}
        foreach ($binary in $binaryNames) {
            $staged = Join-Path $StagingRoot $binary
            if (-not (Get-RealFileState -Path $staged -Label 'Staged binary')) {
                throw "Staged binary missing: $binary"
            }
            $destination = Join-Path $InstallDir $binary
            $destinations[$binary] = $destination
            $hadOriginal[$binary] = Get-RealFileState -Path $destination -Label 'Install destination'
            if ($hadOriginal[$binary]) {
                $destinationItem = Get-Item -LiteralPath $destination -Force -ErrorAction Stop
                if (($destinationItem.Attributes -band [IO.FileAttributes]::ReadOnly) -ne 0) {
                    throw "Install destination is read-only: $binary"
                }
                $oldHashes[$binary] = Get-FileDigest -Path $destination
            } else {
                $oldHashes[$binary] = ''
            }
        }

        $recoveryPath = Join-Path $InstallDir '.deoxidizer-install-recovery'
        if (Test-Path -LiteralPath $recoveryPath) {
            throw 'An install recovery transaction is already present'
        }
        $workPath = Join-Path $InstallDir ('.deoxidizer-install-stage-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $workPath | Out-Null
        try {
            foreach ($binary in $binaryNames) {
                Copy-Item -LiteralPath (Join-Path $StagingRoot $binary) -Destination (Join-Path $workPath ($binary + '.new'))
                $newHashes[$binary] = Get-FileDigest -Path (Join-Path $workPath ($binary + '.new'))
                if ($hadOriginal[$binary]) {
                    Copy-Item -LiteralPath $destinations[$binary] -Destination (Join-Path $workPath ($binary + '.backup'))
                    if ((Get-FileDigest -Path (Join-Path $workPath ($binary + '.backup'))) -ne $oldHashes[$binary]) {
                        throw "Install destination changed while staging: $binary"
                    }
                }
            }
            $state = [ordered]@{
                schema_version = 1
                phase = 'prepared'
                hadDeoxidizer = [bool]$hadOriginal['deoxidizer.exe']
                hadDeox = [bool]$hadOriginal['deox.exe']
                oldDeoxidizerHash = [string]$oldHashes['deoxidizer.exe']
                oldDeoxHash = [string]$oldHashes['deox.exe']
                newDeoxidizerHash = [string]$newHashes['deoxidizer.exe']
                newDeoxHash = [string]$newHashes['deox.exe']
            }
            Write-AtomicRecoveryState -RecoveryDir $workPath -State $state
            [IO.Directory]::Move($workPath, $recoveryPath)
        } catch {
            if (Test-Path -LiteralPath $workPath) {
                Remove-Item -LiteralPath $workPath -Recurse -Force -ErrorAction SilentlyContinue
            }
            throw
        }

        try {
            foreach ($binary in $binaryNames) {
                $newFile = Join-Path $recoveryPath ($binary + '.new')
                $destination = $destinations[$binary]
                $nowExists = Get-RealFileState -Path $destination -Label 'Install destination'
                if ($nowExists -ne $hadOriginal[$binary]) {
                    throw "Install destination changed during transaction: $binary"
                }
                if ($nowExists -and (Get-FileDigest -Path $destination) -ne $oldHashes[$binary]) {
                    throw "Install destination content changed during transaction: $binary"
                }
                if ($hadOriginal[$binary]) {
                    [IO.File]::Replace($newFile, $destination, $null)
                } else {
                    [IO.File]::Move($newFile, $destination)
                }
            }
            $state['phase'] = 'committed'
            Write-AtomicRecoveryState -RecoveryDir $recoveryPath -State $state
        } catch {
            $installError = $_
            try {
                Recover-InstallTransaction
            } catch {
                throw "Binary pair install failed and rollback is pending: $installError; $($_.Exception.Message)"
            }
            throw $installError
        }
        Remove-Item -LiteralPath $recoveryPath -Recurse -Force
    }

    try {
        try {
            $installDirectory = Get-Item -LiteralPath $InstallDir -Force -ErrorAction Stop
        } catch {
            if ($_.CategoryInfo.Category -ne [System.Management.Automation.ErrorCategory]::ObjectNotFound) { throw }
            $installDirectory = $null
        }
        if (-not $installDirectory) {
            New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
            $installDirectory = Get-Item -LiteralPath $InstallDir -Force -ErrorAction Stop
        }
        if (($installDirectory.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or -not $installDirectory.PSIsContainer) {
            throw "Install directory is not a real directory: $InstallDir"
        }
        Recover-InstallTransaction

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

        foreach ($binary in @('deoxidizer.exe', 'deox.exe')) {
            $staged = Join-Path $stagingRoot $binary
            $stagedItem = Get-Item -LiteralPath $staged
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
        }
        Install-BinaryPair -StagingRoot $stagingRoot
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
