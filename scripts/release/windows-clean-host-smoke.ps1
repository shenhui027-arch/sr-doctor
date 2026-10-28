[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ZipPath,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9A-Fa-f]{64}$')][string]$ExpectedZipSha256,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9A-Fa-f]{40}$')][string]$ExpectedCommit,
    [Parameter(Mandatory = $true)][string]$WorkRoot,
    [Parameter(Mandatory = $true)][string]$ReceiptPath,
    [ValidateRange(1, 65535)][int]$Port = 8000,
    [switch]$VerifyOnly
)

$ErrorActionPreference = 'Stop'
$expectedSchema = 'sr-doctor.candidate/v0.1'
$apiBase = "http://127.0.0.1:$Port"
$startedApi = $null
$previousFrontendDist = $env:DOCTOR_FRONTEND_DIST
$previousDoctorPort = $env:DOCTOR_PORT
$caught = $null
$stage = 'initialization'
$summary = [ordered]@{
    schema_version = 'sr-doctor.windows-clean-host-smoke/v1'
    status = 'FAIL'
    mode = if ($VerifyOnly) { 'verify_only' } else { 'verify_start_stop' }
    started_utc = [DateTime]::UtcNow.ToString('o')
    host = [ordered]@{
        computer_name = [Environment]::MachineName
        os_version = [Environment]::OSVersion.VersionString
        powershell_version = $PSVersionTable.PSVersion.ToString()
    }
}

function Get-Sha256([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        (($sha256.ComputeHash($stream) | ForEach-Object { $_.ToString('x2') }) -join '')
    }
    finally {
        $sha256.Dispose()
        $stream.Dispose()
    }
}

function Test-TcpPortOccupied([int]$Port) {
    $client = [Net.Sockets.TcpClient]::new()
    try {
        $pending = $client.BeginConnect('127.0.0.1', $Port, $null, $null)
        if (-not $pending.AsyncWaitHandle.WaitOne(500)) { return $false }
        try { $client.EndConnect($pending); return $client.Connected }
        catch { return $false }
        finally { $pending.AsyncWaitHandle.Dispose() }
    }
    finally {
        $client.Dispose()
    }
}

function Wait-TcpPortFree([int]$Port) {
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not (Test-TcpPortOccupied $Port)) { return }
        Start-Sleep -Milliseconds 200
    }
    throw "TCP port $Port remained occupied after bounded teardown."
}

function Wait-Api {
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $response = Invoke-WebRequest "$apiBase/api/v1/capabilities" -UseBasicParsing -TimeoutSec 2
            if ($response.StatusCode -eq 200) { return }
        }
        catch {
            Start-Sleep -Milliseconds 250
        }
    }
    throw 'The verified Candidate API did not become ready.'
}

function Assert-SafeRelativePath([string]$Path, [string]$Label) {
    if ([string]::IsNullOrWhiteSpace($Path) -or $Path.Contains('\') -or $Path.Contains(':') -or $Path.StartsWith('/')) {
        throw "$Label is not a safe relative path: $Path"
    }
    $segments = @($Path.Split('/'))
    if ($segments.Count -eq 0 -or @($segments | Where-Object { $_ -eq '' -or $_ -eq '.' -or $_ -eq '..' }).Count -ne 0) {
        throw "$Label is not a safe relative path: $Path"
    }
}

function Stop-OwnedApi {
    if ($null -eq $script:startedApi) { return }
    $process = Get-Process -Id $script:startedApi.Id -ErrorAction SilentlyContinue
    if ($null -ne $process) {
        Stop-Process -Id $script:startedApi.Id -Force -ErrorAction Stop
        $deadline = [DateTime]::UtcNow.AddSeconds(15)
        while ([DateTime]::UtcNow -lt $deadline -and $null -ne (Get-Process -Id $script:startedApi.Id -ErrorAction SilentlyContinue)) {
            Start-Sleep -Milliseconds 100
        }
        if ($null -ne (Get-Process -Id $script:startedApi.Id -ErrorAction SilentlyContinue)) {
            throw 'Candidate API process remained alive after bounded teardown.'
        }
    }
    $script:startedApi = $null
}

try {
    $stage = 'input_preconditions'
    $summary.evidence_sources = [ordered]@{ script_sha256 = Get-Sha256 $PSCommandPath }
    if (-not (Test-Path -LiteralPath $ZipPath -PathType Leaf)) { throw "Release ZIP is missing: $ZipPath" }
    $ZipPath = (Resolve-Path -LiteralPath $ZipPath).Path
    $WorkRoot = [IO.Path]::GetFullPath($WorkRoot)
    $ReceiptPath = [IO.Path]::GetFullPath($ReceiptPath)
    if (Test-Path -LiteralPath $WorkRoot) { throw "WorkRoot must not already exist: $WorkRoot" }
    $zipHash = Get-Sha256 $ZipPath
    if ($zipHash -ne $ExpectedZipSha256.ToLowerInvariant()) { throw 'Release ZIP SHA-256 does not match the frozen expected value.' }
    $zipLength = (Get-Item -LiteralPath $ZipPath).Length
    $summary.release_zip = [ordered]@{
        filename = [IO.Path]::GetFileName($ZipPath)
        size_bytes = $zipLength
        sha256 = $zipHash
    }

    $stage = 'archive_preflight'
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($ZipPath)
    try {
        $seenEntries = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
        $entryCount = 0
        [int64]$uncompressedBytes = 0
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName.Replace('\', '/')
            if ($name.EndsWith('/')) { continue }
            Assert-SafeRelativePath $name 'ZIP entry'
            if (-not $name.StartsWith('candidate/', [StringComparison]::Ordinal)) { throw "ZIP entry is outside candidate/: $name" }
            if (-not $seenEntries.Add($name)) { throw "ZIP contains a duplicate path: $name" }
            $unixType = (($entry.ExternalAttributes -shr 16) -band 0xF000)
            if ($unixType -eq 0xA000) { throw "ZIP contains a symbolic link: $name" }
            $entryCount += 1
            $uncompressedBytes += $entry.Length
            if ($entryCount -gt 1000 -or $uncompressedBytes -gt 2147483648) { throw 'ZIP exceeds the bounded entry or expanded-size budget.' }
        }
        if ($entryCount -eq 0) { throw 'Release ZIP has no Candidate payload files.' }
        if (-not $seenEntries.Contains('candidate/candidate-manifest.json')) { throw 'Release ZIP is missing candidate/candidate-manifest.json.' }
    }
    finally {
        $zip.Dispose()
    }
    $summary.archive_preflight = [ordered]@{ file_entries = $entryCount; uncompressed_bytes = $uncompressedBytes }

    $stage = 'fresh_extract'
    New-Item -ItemType Directory -Path $WorkRoot | Out-Null
    Expand-Archive -LiteralPath $ZipPath -DestinationPath $WorkRoot
    $candidateRoot = Join-Path $WorkRoot 'candidate'
    $manifestPath = Join-Path $candidateRoot 'candidate-manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw 'Extracted Candidate manifest is missing.' }
    $candidate = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json

    $stage = 'manifest_identity'
    if ($candidate.schema_version -ne $expectedSchema) { throw "Unexpected Candidate schema: $($candidate.schema_version)" }
    if ($candidate.source.dirty -ne $false -or $candidate.source.dirty_entry_count -ne 0) { throw 'Candidate source is not recorded as clean.' }
    if ($candidate.source.git_head -ne $ExpectedCommit.ToLowerInvariant()) { throw 'Candidate source commit does not match ExpectedCommit.' }
    if ($candidate.build.executed -ne $true) { throw 'Candidate does not record a formal build.' }
    if ($candidate.build.embedded_identity.git_commit -ne $candidate.source.git_head -or $candidate.build.embedded_identity.candidate_label -ne $candidate.run_id) {
        throw 'Candidate embedded build identity does not match its source and run id.'
    }
    if ($candidate.license_review.status -ne 'complete' `
        -or $candidate.license_review.redistribution_status -ne 'ready_with_included_license_and_notices' `
        -or $candidate.license_review.missing_license_expression_count -ne 0 `
        -or $candidate.license_review.missing_local_license_text_count -ne 0) {
        throw 'Candidate license gate is incomplete.'
    }
    $cyclonedxPath = Join-Path $candidateRoot 'sbom\cyclonedx-1.5.json'
    $cargoSbomPath = Join-Path $candidateRoot 'sbom\cargo-dependencies.json'
    $npmSbomPath = Join-Path $candidateRoot 'sbom\npm-dependencies.json'
    foreach ($sbomPath in @($cyclonedxPath, $cargoSbomPath, $npmSbomPath)) {
        if (-not (Test-Path -LiteralPath $sbomPath -PathType Leaf)) { throw "Candidate product-version evidence is missing: $sbomPath" }
    }
    $cyclonedx = Get-Content -LiteralPath $cyclonedxPath -Raw | ConvertFrom-Json
    $cargoSbom = Get-Content -LiteralPath $cargoSbomPath -Raw | ConvertFrom-Json
    $npmSbom = Get-Content -LiteralPath $npmSbomPath -Raw | ConvertFrom-Json
    $doctorApi = @($cargoSbom.packages | Where-Object { $_.name -eq 'doctor-api' -and $_.source -eq 'workspace' })
    if ($cyclonedx.metadata.component.type -ne 'application' -or $cyclonedx.metadata.component.name -ne 'sr-doctor' `
        -or $doctorApi.Count -ne 1 -or [string]::IsNullOrWhiteSpace([string]$npmSbom.project.version)) {
        throw 'Candidate SBOM product-version evidence is malformed.'
    }
    $sbomProductVersions = @([string]$cyclonedx.metadata.component.version, [string]$doctorApi[0].version, [string]$npmSbom.project.version)
    if (@($sbomProductVersions | Where-Object { [string]::IsNullOrWhiteSpace($_) }).Count -ne 0) { throw 'Candidate SBOM product-version evidence is missing a version.' }
    if ($null -eq $candidate.PSObject.Properties['product_version']) {
        if (@($sbomProductVersions | Where-Object { $_ -ne '0.1.0' }).Count -ne 0) {
            throw 'Legacy Candidate omits product_version without consistent v0.1.0 SBOM evidence.'
        }
        $candidateProductVersion = '0.1.0'
        $candidateProductVersionEvidence = 'legacy_sbom_v0.1.0'
    }
    else {
        if ($candidate.product_version -isnot [string] -or [string]::IsNullOrWhiteSpace($candidate.product_version)) {
            throw 'Candidate product_version must be a non-empty string when present.'
        }
        $candidateProductVersion = [string]$candidate.product_version
        if (@($sbomProductVersions | Where-Object { $_ -ne $candidateProductVersion }).Count -ne 0) {
            throw 'Candidate product_version does not match all embedded SBOM evidence.'
        }
        $candidateProductVersionEvidence = 'manifest_and_sbom'
    }
    $expectedEntrypoints = [ordered]@{
        api_binary = 'doctor-api.exe'
        frontend_dist = 'frontend'
        start_script = 'start.ps1'
        bind_address = '127.0.0.1:8000'
    }
    foreach ($key in $expectedEntrypoints.Keys) {
        if ($candidate.entrypoints.$key -ne $expectedEntrypoints[$key]) { throw "Candidate entrypoint mismatch: $key" }
    }

    $stage = 'payload_hashes'
    $expectedFiles = [Collections.Generic.Dictionary[string, object]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($descriptor in @($candidate.package_files)) {
        Assert-SafeRelativePath $descriptor.path 'Manifest payload path'
        if ($expectedFiles.ContainsKey($descriptor.path)) { throw "Candidate manifest contains a duplicate path: $($descriptor.path)" }
        $expectedFiles.Add($descriptor.path, $descriptor)
        $payloadPath = [IO.Path]::GetFullPath((Join-Path $candidateRoot $descriptor.path.Replace('/', [IO.Path]::DirectorySeparatorChar)))
        $candidatePrefix = [IO.Path]::GetFullPath($candidateRoot).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
        if (-not $payloadPath.StartsWith($candidatePrefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Manifest path escapes Candidate root: $($descriptor.path)" }
        if (-not (Test-Path -LiteralPath $payloadPath -PathType Leaf)) { throw "Manifest payload is missing: $($descriptor.path)" }
        $item = Get-Item -LiteralPath $payloadPath
        if ($item.Length -ne [int64]$descriptor.size_bytes -or (Get-Sha256 $payloadPath) -ne $descriptor.sha256) {
            throw "Candidate payload hash or size mismatch: $($descriptor.path)"
        }
    }
    $prefix = [IO.Path]::GetFullPath($candidateRoot).TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $actualFiles = @(Get-ChildItem -LiteralPath $candidateRoot -Recurse -File | ForEach-Object {
        $_.FullName.Substring($prefix.Length).Replace('\', '/')
    } | Where-Object { $_ -ne 'candidate-manifest.json' })
    if ($actualFiles.Count -ne $expectedFiles.Count -or @($actualFiles | Where-Object { -not $expectedFiles.ContainsKey($_) }).Count -ne 0) {
        throw 'Extracted Candidate files do not exactly match the manifest allowlist.'
    }
    $summary.candidate = [ordered]@{
        run_id = $candidate.run_id
        product_version = $candidateProductVersion
        product_version_evidence = $candidateProductVersionEvidence
        git_commit = $candidate.source.git_head
        manifest_sha256 = Get-Sha256 $manifestPath
        package_file_count = $expectedFiles.Count
        extraction_directory = $WorkRoot
        immutable_package_intact = $true
    }

    if (-not $VerifyOnly) {
        $stage = 'runtime_precondition'
        if (Test-TcpPortOccupied $Port) { throw "TCP port 127.0.0.1:$Port is already occupied." }
        $apiExecutable = Join-Path $candidateRoot 'doctor-api.exe'
        $frontendDist = Join-Path $candidateRoot 'frontend'
        $stdoutPath = Join-Path $WorkRoot 'doctor-api.stdout.log'
        $stderrPath = Join-Path $WorkRoot 'doctor-api.stderr.log'
        $env:DOCTOR_FRONTEND_DIST = $frontendDist
        $env:DOCTOR_PORT = [string]$Port
        $stage = 'api_start'
        $startedApi = Start-Process -FilePath $apiExecutable -WorkingDirectory $candidateRoot -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath
        $env:DOCTOR_FRONTEND_DIST = $previousFrontendDist
        $env:DOCTOR_PORT = $previousDoctorPort
        Wait-Api

        $stage = 'runtime_identity'
        $capabilities = Invoke-RestMethod "$apiBase/api/v1/capabilities" -TimeoutSec 5
        if ($capabilities.build.git_commit -ne $candidate.source.git_head `
            -or $capabilities.build.candidate_label -ne $candidate.run_id) {
            throw 'Running API build identity does not match Candidate manifest.'
        }
        $homeResponse = Invoke-WebRequest "$apiBase/" -UseBasicParsing -TimeoutSec 5
        if ($homeResponse.StatusCode -ne 200) { throw 'Candidate frontend entrypoint did not return HTTP 200.' }
        $summary.runtime = [ordered]@{
            bind_address = "127.0.0.1:$Port"
            api_owner = 'this_script'
            candidate_label = $capabilities.build.candidate_label
            git_commit = $capabilities.build.git_commit
            capabilities_http_status = 200
            frontend_http_status = $homeResponse.StatusCode
        }

        $stage = 'bounded_teardown'
        Stop-OwnedApi
        Wait-TcpPortFree $Port
        $summary.teardown = [ordered]@{
            process_stopped = $true
            port_released = $true
            candidate_directory_preserved = $true
            prior_candidate_restart = 'not_executed_no_prior_candidate_supplied'
        }
    }

    $summary.status = 'PASS'
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
}
catch {
    $caught = $_
    $summary.status = 'FAIL'
    $summary.failed_stage = $stage
    $summary.error = $_.Exception.Message
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
}
finally {
    $env:DOCTOR_FRONTEND_DIST = $previousFrontendDist
    $env:DOCTOR_PORT = $previousDoctorPort
    if ($null -ne $startedApi) {
        try { Stop-OwnedApi } catch { }
        try { Wait-TcpPortFree $Port } catch { }
    }
    $receiptParent = Split-Path -Parent $ReceiptPath
    if (-not [string]::IsNullOrWhiteSpace($receiptParent)) { New-Item -ItemType Directory -Force -Path $receiptParent | Out-Null }
    [IO.File]::WriteAllText($ReceiptPath, ($summary | ConvertTo-Json -Depth 12), [Text.UTF8Encoding]::new($false))
}

$summary | ConvertTo-Json -Depth 12
if ($null -ne $caught) { throw $caught }
