#requires -Version 5.1
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$FilePath
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ($env:SKIP_WIN_CODESIGN -eq '1') {
  if ($env:DEOX_RELEASE_CONFIRM -ne 'YES') { throw 'SKIP_WIN_CODESIGN=1 requires DEOX_RELEASE_CONFIRM=YES explicitly.' }
  Write-Host "SKIP_WIN_CODESIGN=1; leaving Windows artifact unsigned: $FilePath"
  exit 0
}
if ($env:OS -ne 'Windows_NT') { throw 'Azure Artifact Signing must run on Windows.' }

$required = @('AZURE_CLIENT_ID','AZURE_TENANT_ID','AZURE_CLIENT_SECRET','AZURE_ARTIFACT_SIGNING_ENDPOINT','AZURE_ARTIFACT_SIGNING_ACCOUNT','AZURE_ARTIFACT_SIGNING_PROFILE','AZURE_ARTIFACT_SIGNING_PUBLISHER','AZURE_ARTIFACT_SIGNING_PUBLISHER_DN')
# NOTE: AZURE_SUBSCRIPTION_ID is intentionally not required here. The signtool
# /dlib metadata flow authenticates via endpoint + account + profile and never
# reads the subscription; the subscription stays plumbed through release.cjs /
# release.yml only for Azure portal scoping on the release VM.
$missing = @($required | Where-Object { [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($_)) })
if ($missing.Count) { throw "Missing Azure Artifact Signing environment variables: $($missing -join ', ')" }

$resolved = (Resolve-Path -LiteralPath $FilePath).Path
. (Join-Path $PSScriptRoot 'artifact-signing-tools.ps1')
Import-BundledPowerShellSecurityModule
$tools = Get-ArtifactSigningTools

$metadataPath = Join-Path ([IO.Path]::GetTempPath()) "artifact-signing-$PID-$([Guid]::NewGuid().ToString('N')).json"
try {
  $metadata = @{
    Endpoint = $env:AZURE_ARTIFACT_SIGNING_ENDPOINT.Trim()
    CodeSigningAccountName = $env:AZURE_ARTIFACT_SIGNING_ACCOUNT.Trim()
    CertificateProfileName = $env:AZURE_ARTIFACT_SIGNING_PROFILE.Trim()
    ExcludeCredentials = @('ManagedIdentityCredential','WorkloadIdentityCredential','SharedTokenCacheCredential','VisualStudioCredential','VisualStudioCodeCredential','AzureCliCredential','AzurePowerShellCredential','AzureDeveloperCliCredential','InteractiveBrowserCredential')
  } | ConvertTo-Json -Depth 4
  [IO.File]::WriteAllText($metadataPath, $metadata, (New-Object Text.UTF8Encoding($false)))

  Write-Host "Artifact Signing: $resolved"
  & $tools.SignToolPath sign /v /debug /fd SHA256 /tr 'https://timestamp.acs.microsoft.com' /td SHA256 /dlib $tools.DlibPath /dmdf $metadataPath $resolved
  if ($LASTEXITCODE -ne 0) { throw "SignTool failed with exit code $LASTEXITCODE for $resolved" }
} finally {
  Remove-Item -LiteralPath $metadataPath -Force -ErrorAction SilentlyContinue
}

$signature = Get-AuthenticodeSignature -LiteralPath $resolved
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) { throw "Authenticode verification failed for ${resolved}: $($signature.Status) $($signature.StatusMessage)" }
if (-not $signature.SignerCertificate) { throw "Missing signer certificate: $resolved" }
$publisher = $signature.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
if ($publisher -ne $env:AZURE_ARTIFACT_SIGNING_PUBLISHER.Trim()) { throw "Unexpected Authenticode publisher for $resolved. Expected '$($env:AZURE_ARTIFACT_SIGNING_PUBLISHER.Trim())', got '$publisher'." }
$subject = $signature.SignerCertificate.Subject.Trim()
$expectedSubject = $env:AZURE_ARTIFACT_SIGNING_PUBLISHER_DN.Trim()
if ($subject -ne $expectedSubject) { throw "Unexpected Authenticode Subject for $resolved. Expected '$expectedSubject', got '$subject'." }
if (-not $signature.TimeStamperCertificate) { throw "Missing RFC3161 timestamp: $resolved" }
Write-Host "Verified Authenticode signature: $subject ($resolved)"
