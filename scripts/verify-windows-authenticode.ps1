#requires -Version 5.1
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$TargetReleaseDir,
  [string[]]$ExtraFiles = @(),
  [string]$ProofPath,
  [string]$Target,
  [string]$Commit
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:SKIP_WIN_CODESIGN -eq '1') {
  if ($env:DEOX_RELEASE_CONFIRM -ne 'YES') { throw 'SKIP_WIN_CODESIGN=1 requires DEOX_RELEASE_CONFIRM=YES explicitly.' }
  if ($ProofPath) { throw 'Skipped Authenticode verification cannot create native release proof.' }
  Write-Host 'SKIP_WIN_CODESIGN=1; skipping Authenticode verification for local-only inspection.'
  exit 0
}
if ($env:OS -ne 'Windows_NT') { throw 'Authenticode verification must run on Windows.' }
if ([string]::IsNullOrWhiteSpace($env:AZURE_ARTIFACT_SIGNING_PUBLISHER)) { throw 'AZURE_ARTIFACT_SIGNING_PUBLISHER is required for Authenticode verification.' }
if ([string]::IsNullOrWhiteSpace($env:AZURE_ARTIFACT_SIGNING_PUBLISHER_DN)) { throw 'AZURE_ARTIFACT_SIGNING_PUBLISHER_DN is required for full Authenticode identity verification.' }
if ($ProofPath) {
  if ([string]::IsNullOrWhiteSpace($Target) -or $Target -notmatch '^(x86_64|aarch64)-pc-windows-msvc$') {
    throw 'A supported Windows Rust target is required for native release proof.'
  }
  if ([string]::IsNullOrWhiteSpace($Commit) -or $Commit -notmatch '^[0-9a-fA-F]{40}$') {
    throw 'A full candidate commit is required for native release proof.'
  }
  if (Test-Path -LiteralPath $ProofPath) { throw "Refusing to replace native release proof: $ProofPath" }
}
. (Join-Path $PSScriptRoot 'artifact-signing-tools.ps1')
Import-BundledPowerShellSecurityModule
$releaseDir = (Resolve-Path -LiteralPath $TargetReleaseDir).Path
$files = @(Get-ChildItem -LiteralPath $releaseDir -File -Filter '*.exe')
foreach ($extra in $ExtraFiles) {
  if ($extra) { $files += Get-Item -LiteralPath $extra }
}
$files = @($files | Sort-Object FullName -Unique)
if (-not $files.Count) { throw "No Windows runtime or installer artifacts were found under $releaseDir" }
$expected = $env:AZURE_ARTIFACT_SIGNING_PUBLISHER.Trim()
$expectedSubject = $env:AZURE_ARTIFACT_SIGNING_PUBLISHER_DN.Trim()
$records = @{}
  foreach ($file in $files) {
    $signature = Get-AuthenticodeSignature -LiteralPath $file.FullName
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
      throw "Invalid or missing Authenticode signature: $($file.FullName) ($($signature.Status))"
    }
    if (-not $signature.SignerCertificate) { throw "Missing signer certificate: $($file.FullName)" }
    $publisher = $signature.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
    if ($publisher -ne $expected) { throw "Unexpected publisher for $($file.FullName): '$publisher'" }
    $subject = $signature.SignerCertificate.Subject.Trim()
    if ($subject -ne $expectedSubject) { throw "Unexpected certificate Subject for $($file.FullName). Expected '$expectedSubject', got '$subject'." }
    if (-not $signature.TimeStamperCertificate) { throw "Missing RFC3161 timestamp: $($file.FullName)" }
    $name = [IO.Path]::GetFileName($file.FullName)
    if ($records.ContainsKey($name)) {
      $previous = $records[$name]
      $currentHash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
      if ($previous.sha256 -ne $currentHash) { throw "Ambiguous Windows payload name has different bytes: $name" }
      continue
    }
    $records[$name] = @{
      sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
      publisher = $publisher
      subject = $subject
      timestamp_verified = $true
    }
    Write-Host "Verified: $($file.FullName)"
  }

  if ($ProofPath) {
    $arch = if ($Target.StartsWith('x86_64-', [StringComparison]::Ordinal)) { 'x86_64' } else { 'aarch64' }
    $cargoText = Get-Content -LiteralPath (Join-Path $PSScriptRoot '..\Cargo.toml') -Raw
    $packageSection = [regex]::Match($cargoText, '(?ms)^\[package\]\s*(.*?)(?=^\[|\z)')
    $versionMatch = [regex]::Match($packageSection.Groups[1].Value, '(?m)^version\s*=\s*"([^"]+)"')
    if (-not $versionMatch.Success) { throw 'Cargo.toml package version is missing for Windows proof.' }
    $setupName = "deoxidizer-v$($versionMatch.Groups[1].Value)-windows-$arch-setup.exe"
    if (-not $records.ContainsKey('deoxidizer.exe') -or -not $records.ContainsKey('deox.exe')) {
      throw 'Native release proof requires exactly the deoxidizer.exe and deox.exe binaries.'
    }
    if (-not $records.ContainsKey($setupName)) {
      throw "Native release proof requires timestamped installer $setupName."
    }
    $proof = @{
      schema_version = 1
      platform = 'windows'
      target = $Target
      commit = $Commit.ToLowerInvariant()
      artifacts = @{
        'deoxidizer.exe' = $records['deoxidizer.exe'].sha256
        'deox.exe' = $records['deox.exe'].sha256
      }
      installer = @{ name = $setupName; sha256 = $records[$setupName].sha256 }
      native = @{
        mechanism = 'authenticode'
        signature_verified = $true
        timestamp_verified = $true
        publisher = $expected
        subject = $expectedSubject
      }
    }
    $json = $proof | ConvertTo-Json -Depth 8
    $stream = New-Object IO.FileStream($ProofPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try {
      $bytes = (New-Object Text.UTF8Encoding($false)).GetBytes($json + "`n")
      $stream.Write($bytes, 0, $bytes.Length)
      $stream.Flush($true)
    } finally { $stream.Dispose() }
  }
  Write-Host "Verified $($records.Count) timestamped Windows artifact(s) from '$expectedSubject'."
