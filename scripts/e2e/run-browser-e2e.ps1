[CmdletBinding()]
param(
    [ValidateSet('offline-local', 'connected-readonly-docker')][string]$Project = 'offline-local',
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1, 65535)][int]$ApiPort = 8000,
    [string]$ApiExecutable,
    [string]$FrontendDist,
    [string]$CandidateManifest,
    [switch]$DevelopmentRunner,
    # An API already listening on the requested port has no process/build
    # identity that this runner can verify. Release evidence therefore uses
    # this switch and starts the explicitly hashed binary itself.
    [switch]$RequireOwnedApi,
    # v0.2: compare the actual browser window against an independent AuditLoader read.
    [switch]$AuditReleaseAcceptance,
    [string]$AuditResourceBuilder,
    [switch]$KeepStartedApi,
    # A connected run creates a fixture-only plugin archive and FE logs that
    # contain its derived writer credential. Release runs therefore tear the
    # fixture down by default; retaining it is debugging-only and cannot be
    # used as cold-start evidence.
    [switch]$KeepFixture,
    # Skip the default incremental rebuild of the repository API and frontend.
    # Only for runs that pass explicit, already-verified artifacts.
    [switch]$SkipBuild,
    # Bootstrap the allin1 fixture with -LowResource (2.5 CPUs / 5.25 GiB).
    [switch]$LowResource
)

$ErrorActionPreference = 'Stop'
if ($DevelopmentRunner -and -not [string]::IsNullOrWhiteSpace($CandidateManifest)) { throw 'DevelopmentRunner and CandidateManifest are mutually exclusive.' }
if ($DevelopmentRunner) {
    if (-not $PSBoundParameters.ContainsKey('ApiPort')) { $ApiPort = 18049 }
    $RequireOwnedApi = $true
}
if ($AuditReleaseAcceptance -and ($Project -ne 'connected-readonly-docker' -or -not $RequireOwnedApi)) {
    throw 'Audit release acceptance requires the owned real AuditLoader path.'
}
if (-not [string]::IsNullOrWhiteSpace($AuditResourceBuilder)) {
    if (-not $AuditReleaseAcceptance -or -not $RequireOwnedApi) { throw 'AuditResourceBuilder requires owned AuditReleaseAcceptance.' }
    if (-not [IO.Path]::IsPathFullyQualified($AuditResourceBuilder) -or -not (Test-Path -LiteralPath $AuditResourceBuilder -PathType Leaf)) { throw 'AuditResourceBuilder must be an existing absolute executable path.' }
}
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$startedApi = $null
$previousFrontendDist = $env:DOCTOR_FRONTEND_DIST
$previousDoctorPort = $env:DOCTOR_PORT
$previousDoctorHistory = $env:DOCTOR_HISTORY
$previousUser = $env:STARROCKS_USER
$previousPassword = $env:STARROCKS_PASSWORD
$previousBaseline = $env:E2E_BASELINE_QUERY_ID
$previousCandidate = $env:E2E_CANDIDATE_QUERY_ID
$previousBuildIdentity = $env:E2E_BUILD_IDENTITY_PATH
$previousArtifactRoot = $env:E2E_ARTIFACT_ROOT
$resourceEnvNames = @('E2E_AUDIT_RESOURCE_BUILDER','E2E_AUDIT_RESOURCE_OWNED','E2E_AUDIT_RESOURCE_DOCKER','E2E_AUDIT_RESOURCE_PWSH')
$previousResourceEnv = @{}
foreach ($name in $resourceEnvNames) { $previousResourceEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process'); [Environment]::SetEnvironmentVariable($name, $null, 'Process') }
$previousBaseUrl = $env:E2E_BASE_URL
$previousMissingProfileDigest = $env:E2E_MISSING_PROFILE_DIGEST
$previousMissingProfileQueryId = $env:E2E_MISSING_PROFILE_QUERY_ID
$env:E2E_MISSING_PROFILE_DIGEST = $null
$env:E2E_MISSING_PROFILE_QUERY_ID = $null
$auditArtifactRoot = Join-Path $runtimeRoot ('audit-release-' + [Guid]::NewGuid().ToString('N'))
$releaseReceiptPath = Join-Path $runtimeRoot ("connected-browser-" + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ') + '.json')
$stage = 'initialization'
$summary = [ordered]@{ project = $Project; status = 'FAIL'; started_utc = [DateTime]::UtcNow.ToString('o') }
$candidateIdentity = $null
$fixtureContainerName = 'sr-doctor-v0.1-e2e-3514'
$fixtureDataRoot = [IO.Path]::GetFullPath((Join-Path ($env:SR_DOCTOR_CONTAINER_ROOT ?? (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'containers')) 'sr-doctor-v0.1-e2e-3514'))
$fixtureDocker = $null
$fixtureBootstrapStarted = $false
$primaryFailure = $false
$summary.evidence_scope = if ($DevelopmentRunner) { 'development_only' } else { 'production_runner' }

function Test-LocalApi {
    try { return (Invoke-WebRequest "$apiBase/capabilities" -UseBasicParsing -TimeoutSec 2).StatusCode -eq 200 }
    catch { return $false }
}

function Wait-LocalApi {
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (Test-LocalApi) { return }
        Start-Sleep -Milliseconds 250
    }
    throw 'The local Doctor API did not become ready.'
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

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-Sha256Text([string]$Value) {
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        (($sha256.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value)) | ForEach-Object { $_.ToString('x2') }) -join '')
    }
    finally {
        $sha256.Dispose()
    }
}

function Get-FrontendIdentity([string]$Dist) {
    $prefix = $Dist.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar
    $files = @(Get-ChildItem -LiteralPath $Dist -Recurse -File | ForEach-Object {
        $relative = $_.FullName.Substring($prefix.Length).Replace('\', '/')
        [pscustomobject]@{ path = $relative; byte_length = $_.Length; sha256 = Get-Sha256 $_.FullName }
    } | Sort-Object -Property path)
    if ($files.Count -eq 0) { throw 'FrontendDist contains no files.' }
    $canonical = (($files | ForEach-Object { "$($_.path)`0$($_.byte_length)`0$($_.sha256)" }) -join "`n")
    [ordered]@{
        entrypoint = 'index.html'
        file_count = $files.Count
        tree_sha256 = Get-Sha256Text $canonical
        files = $files
    }
}

try {
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    $summary.evidence_sources = [ordered]@{
        runner_sha256 = Get-Sha256 $PSCommandPath
        browser_sha256 = Get-Sha256 (Join-Path $repoRoot 'tests\e2e\browser-e2e.mjs')
        connected_smoke_sha256 = Get-Sha256 (Join-Path $repoRoot 'scripts\e2e\connected-smoke.ps1')
    }
    if ($RequireOwnedApi -and ($KeepStartedApi -or $KeepFixture)) {
        throw 'Release evidence cannot retain the runner-owned API or dedicated fixture.'
    }
    if ($Project -eq 'connected-readonly-docker' -and $RequireOwnedApi) {
        $stage = 'cold_start_preconditions'
        . (Join-Path $repoRoot 'dev\starrocks-3.5.14\common.ps1')
        $fixtureDocker = Get-FixtureDockerExecutable $DockerExecutable
        $containerAbsent = $null -eq (Get-FixtureContainerIdentity -DockerExecutable $fixtureDocker -ContainerName $fixtureContainerName)
        $dataRootAbsent = -not (Test-Path -LiteralPath $fixtureDataRoot)
        $apiPortFree = -not (Test-TcpPortOccupied $ApiPort)
        $summary.preconditions = [ordered]@{
            mode = 'release_cold_start'
            fixture_container_absent = $containerAbsent
            fixture_data_root_absent = $dataRootAbsent
            api_port_free = $apiPortFree
        }
        if (-not ($containerAbsent -and $dataRootAbsent -and $apiPortFree)) {
            throw 'Release connected E2E requires absent fixture container/data and a free API port before starting.'
        }
    }
    else {
        $summary.preconditions = [ordered]@{ mode = 'debug_or_offline'; mechanically_verified = $false }
    }
    $stage = 'build_identity'
    # The identity below only hashes existing artifacts. When the run uses the
    # repository's own build outputs, rebuild them first (incremental, so cheap
    # when current); otherwise a stale binary silently tests older source.
    $usesRepositoryArtifacts = [string]::IsNullOrWhiteSpace($CandidateManifest) -and [string]::IsNullOrWhiteSpace($ApiExecutable) -and [string]::IsNullOrWhiteSpace($FrontendDist)
    if ($usesRepositoryArtifacts -and -not $SkipBuild) {
        $stage = 'repository_build'
        $cargoArguments = if ($DevelopmentRunner) { @('build', '--locked', '-p', 'doctor-api', '--example', 'loopback-v2-e2e') } else { @('build', '--locked', '-p', 'doctor-api') }
        & cargo @cargoArguments
        if ($LASTEXITCODE -ne 0) { throw 'Repository API build failed before browser E2E.' }
        & npm --prefix (Join-Path $repoRoot 'frontend') run build
        if ($LASTEXITCODE -ne 0) { throw 'Repository frontend build failed before browser E2E.' }
        $summary.repository_build = 'rebuilt_from_current_source'
        $stage = 'build_identity'
    } else {
        $summary.repository_build = if ($SkipBuild) { 'skipped_by_request' } else { 'explicit_artifacts' }
    }
    if ([string]::IsNullOrWhiteSpace($ApiExecutable)) { $ApiExecutable = if ($DevelopmentRunner) { Join-Path $repoRoot 'target\debug\examples\loopback-v2-e2e.exe' } else { Join-Path $repoRoot 'target\debug\doctor-api.exe' } }
    if ([string]::IsNullOrWhiteSpace($FrontendDist)) { $FrontendDist = Join-Path $repoRoot 'frontend\dist' }
    if (-not (Test-Path -LiteralPath $ApiExecutable -PathType Leaf)) { throw 'ApiExecutable is missing. Build doctor-api before browser E2E.' }
    if (-not (Test-Path -LiteralPath (Join-Path $FrontendDist 'index.html') -PathType Leaf)) { throw 'FrontendDist/index.html is missing. Build the frontend before browser E2E.' }
    $ApiExecutable = (Resolve-Path -LiteralPath $ApiExecutable).Path
    $FrontendDist = (Resolve-Path -LiteralPath $FrontendDist).Path
    if (-not [string]::IsNullOrWhiteSpace($CandidateManifest)) {
        if (-not $RequireOwnedApi) { throw 'CandidateManifest verification requires -RequireOwnedApi.' }
        if (-not (Test-Path -LiteralPath $CandidateManifest -PathType Leaf)) { throw 'CandidateManifest is missing.' }
        $CandidateManifest = (Resolve-Path -LiteralPath $CandidateManifest).Path
        $candidate = Get-Content -LiteralPath $CandidateManifest -Raw | ConvertFrom-Json
        if ($candidate.schema_version -ne 'sr-doctor.candidate/v0.1' -or $candidate.build.executed -ne $true) {
            throw 'CandidateManifest does not describe a formally built Candidate.'
        }
        $candidateRoot = Split-Path -Parent $CandidateManifest
        $cyclonedxPath = Join-Path $candidateRoot 'sbom\cyclonedx-1.5.json'
        $cargoSbomPath = Join-Path $candidateRoot 'sbom\cargo-dependencies.json'
        $npmSbomPath = Join-Path $candidateRoot 'sbom\npm-dependencies.json'
        foreach ($sbomPath in @($cyclonedxPath, $cargoSbomPath, $npmSbomPath)) {
            if (-not (Test-Path -LiteralPath $sbomPath -PathType Leaf)) { throw "CandidateManifest product-version evidence is missing: $sbomPath" }
        }
        $cyclonedx = Get-Content -LiteralPath $cyclonedxPath -Raw | ConvertFrom-Json
        $cargoSbom = Get-Content -LiteralPath $cargoSbomPath -Raw | ConvertFrom-Json
        $npmSbom = Get-Content -LiteralPath $npmSbomPath -Raw | ConvertFrom-Json
        $doctorApi = @($cargoSbom.packages | Where-Object { $_.name -eq 'doctor-api' -and $_.source -eq 'workspace' })
        if ($cyclonedx.metadata.component.type -ne 'application' -or $cyclonedx.metadata.component.name -ne 'sr-doctor' `
            -or $doctorApi.Count -ne 1 -or [string]::IsNullOrWhiteSpace([string]$npmSbom.project.version)) {
            throw 'CandidateManifest SBOM product-version evidence is malformed.'
        }
        $sbomProductVersions = @([string]$cyclonedx.metadata.component.version, [string]$doctorApi[0].version, [string]$npmSbom.project.version)
        if (@($sbomProductVersions | Where-Object { [string]::IsNullOrWhiteSpace($_) }).Count -ne 0) { throw 'CandidateManifest SBOM product-version evidence is missing a version.' }
        if ($null -eq $candidate.PSObject.Properties['product_version']) {
            if (@($sbomProductVersions | Where-Object { $_ -ne '0.1.0' }).Count -ne 0) {
                throw 'Legacy CandidateManifest omits product_version without consistent v0.1.0 SBOM evidence.'
            }
            $candidateProductVersion = '0.1.0'
            $candidateProductVersionEvidence = 'legacy_sbom_v0.1.0'
        }
        else {
            if ($candidate.product_version -isnot [string] -or [string]::IsNullOrWhiteSpace($candidate.product_version)) {
                throw 'CandidateManifest product_version must be a non-empty string when present.'
            }
            $candidateProductVersion = [string]$candidate.product_version
            if (@($sbomProductVersions | Where-Object { $_ -ne $candidateProductVersion }).Count -ne 0) {
                throw 'CandidateManifest product_version does not match all embedded SBOM evidence.'
            }
            $candidateProductVersionEvidence = 'manifest_and_sbom'
        }
        $candidateIdentity = $candidate.build.embedded_identity
        if ($null -eq $candidateIdentity `
            -or $candidateIdentity.git_commit -ne $candidate.source.git_head `
            -or $candidateIdentity.candidate_label -ne $candidate.run_id) {
            throw 'CandidateManifest embedded identity does not match its source and run id.'
        }
        $apiDescriptor = @($candidate.package_files | Where-Object { $_.path -eq 'doctor-api.exe' })
        if ($apiDescriptor.Count -ne 1 -or $apiDescriptor[0].sha256 -ne (Get-Sha256 $ApiExecutable)) {
            throw 'ApiExecutable does not match CandidateManifest.'
        }
    }
    $gitHead = (& git -C $repoRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $gitHead -notmatch '^[0-9a-f]{40}$') { throw 'Could not determine the Git HEAD for browser E2E identity.' }
    $buildIdentity = [ordered]@{
        schema_version = 'e2e-build-identity/v1'
        git_head = $gitHead
        expected = [ordered]@{
            api = [ordered]@{ filename = [IO.Path]::GetFileName($ApiExecutable); sha256 = Get-Sha256 $ApiExecutable }
            frontend = Get-FrontendIdentity $FrontendDist
        }
        evidence_scope = $summary.evidence_scope
        runtime = $null
    }
    $apiAlreadyAvailable = Test-LocalApi
    if (-not $apiAlreadyAvailable) {
        $stage = 'api_start'
        $env:DOCTOR_FRONTEND_DIST = $FrontendDist
        if (-not $DevelopmentRunner) { $env:DOCTOR_PORT = [string]$ApiPort }
        # History is off: these checks predate stage B; B-D have their own E2Es.
        $env:DOCTOR_HISTORY = 'off'
        $launchArguments = @{}
        if ($DevelopmentRunner) { $launchArguments.ArgumentList = @([string]$ApiPort) }
        $startedApi = Start-Process @launchArguments -FilePath $ApiExecutable -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
            -RedirectStandardOutput (Join-Path $runtimeRoot 'doctor-api-e2e.stdout.log') `
            -RedirectStandardError (Join-Path $runtimeRoot 'doctor-api-e2e.stderr.log')
        Wait-LocalApi
        $buildIdentity.runtime = [ordered]@{ api_owner = 'this_script'; verification = 'verified'; process_id = $startedApi.Id }
    }
    elseif ($RequireOwnedApi) {
        throw "API port $ApiPort is already in use. Release browser E2E requires a runner-owned API."
    }
    else {
        $buildIdentity.runtime = [ordered]@{ api_owner = 'external'; verification = 'unverified_external_api' }
    }

    if ($null -ne $candidateIdentity) {
        $runtimeCapabilities = Invoke-RestMethod "$apiBase/capabilities" -TimeoutSec 5
        if ($runtimeCapabilities.build.git_commit -ne $candidateIdentity.git_commit `
            -or $runtimeCapabilities.build.candidate_label -ne $candidateIdentity.candidate_label) {
            throw 'Running API build identity does not match CandidateManifest.'
        }
        $summary.candidate_manifest_sha256 = Get-Sha256 $CandidateManifest
        $summary.candidate_product_version = $candidateProductVersion
        $summary.candidate_product_version_evidence = $candidateProductVersionEvidence
        $summary.candidate_label = $candidateIdentity.candidate_label
        $summary.candidate_git_commit = $candidateIdentity.git_commit
        $summary.candidate_identity = 'verified'
    }

    $buildIdentityPath = Join-Path $runtimeRoot ("browser-build-identity-" + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '.json')
    [IO.File]::WriteAllText($buildIdentityPath, ($buildIdentity | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
    $env:E2E_BUILD_IDENTITY_PATH = $buildIdentityPath
    $summary.api_owner = $buildIdentity.runtime.api_owner
    $summary.build_verification = $buildIdentity.runtime.verification
    $summary.git_head = $buildIdentity.git_head
    $summary.api_sha256 = $buildIdentity.expected.api.sha256
    $summary.frontend_tree_sha256 = $buildIdentity.expected.frontend.tree_sha256
    $summary.build_identity_sha256 = Get-Sha256 $buildIdentityPath
    if ($Project -eq 'connected-readonly-docker') {
        $stage = 'fixture_bootstrap'
        . (Join-Path $repoRoot 'dev\starrocks-3.5.14\common.ps1')
        if ($null -eq $fixtureDocker) { $fixtureDocker = Get-FixtureDockerExecutable $DockerExecutable }
        # The fixture token is read only from this process environment. Do not
        # accept it as a script argument: Windows exposes arguments in process
        # command lines.
        $secret = Get-FixtureSecret
        $fixtureBootstrapStarted = $true
        $bootstrap = (& (Join-Path $repoRoot 'dev\starrocks-3.5.14\bootstrap.ps1') -DockerExecutable $DockerExecutable -LowResource:$LowResource) | ConvertFrom-Json
        $stage = 'fixture_workload'
        $workloadOptions = @{}
        if ($AuditReleaseAcceptance) { $workloadOptions.EnableDigestFixture = $true; $workloadOptions.AuditReleaseAcceptance = $true }
        $workload = (& (Join-Path $repoRoot 'dev\starrocks-3.5.14\run-fixture-workload.ps1') -DockerExecutable $DockerExecutable @workloadOptions) | ConvertFrom-Json
        $profilePath = Join-Path $runtimeRoot ("connected-baseline-" + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ') + '.profile')
        $env:STARROCKS_USER = 'doctor_reader'
        $env:STARROCKS_PASSWORD = Get-FixturePassword -Secret $secret -Role doctor_reader
        $stage = 'profile_capture'
        $capture = (& (Join-Path $repoRoot 'scripts\e2e\capture-runtime-profile.ps1') -QueryId $workload.baseline_query_id -OutputPath $profilePath -DockerExecutable $DockerExecutable) | ConvertFrom-Json
        $stage = 'connected_http_smoke'
        $smoke = (& (Join-Path $repoRoot 'scripts\e2e\connected-smoke.ps1') -QueryId $workload.baseline_query_id -CandidateQueryId $workload.candidate_query_id -ProfilePath $capture.output_path -MySqlPort $bootstrap.host_ports.mysql -ApiPort $ApiPort) | ConvertFrom-Json
        $env:E2E_BASELINE_QUERY_ID = $workload.baseline_query_id
        $env:E2E_CANDIDATE_QUERY_ID = $workload.candidate_query_id
        if ($AuditReleaseAcceptance) {
            $env:E2E_MISSING_PROFILE_DIGEST = $workload.missing_profile_digest
            $env:E2E_MISSING_PROFILE_QUERY_ID = $workload.missing_profile_query_id
        }
        $summary.fixture = $bootstrap.fixture
        $summary.image_digest = $bootstrap.image_digest
        $summary.fixture_resources = $bootstrap.resources
        $summary.workload = $workload.fixture
        if ($AuditReleaseAcceptance) {
            $summary.digest_fixture = $workload.digest_fixture
            $summary.audit_visibility = [ordered]@{
                observation_calls = $workload.audit_visibility_observation_calls
                upper_bound_ms = $workload.audit_visibility_upper_bound_ms
                measurement = $workload.audit_visibility_measurement
            }
        }
        $summary.connected_smoke = $smoke.evidence
        $summary.connected_evidence = [ordered]@{
            detected_version = $smoke.detected_version
            effective_time_zone = $smoke.effective_time_zone
            current_utc_offset_seconds = $smoke.current_utc_offset_seconds
            discovered_queries = $smoke.discovered_queries
            unavailable_profile_rows = $smoke.unavailable_profile_rows
            operator_rows = $smoke.operator_rows
            exact_operator_total_time_cells = $smoke.exact_operator_total_time_cells
            graph_state = $smoke.graph_state
            graph_nodes = $smoke.graph_nodes
            summary_state = $smoke.summary_state
            summary_nodes = $smoke.summary_nodes
            summary_hotspot_nodes = $smoke.summary_hotspot_nodes
            official_share_nodes_matched = $smoke.official_share_nodes_matched
            graph_edges = $smoke.graph_edges
            profile_level = $smoke.profile_level
            snapshot_kind = $smoke.snapshot_kind
            merge_state = $smoke.merge_state
            findings = $smoke.findings
            floor_suppressed_evaluations = $smoke.floor_suppressed_evaluations
            finding_rule_ids = $smoke.finding_rule_ids
            analysis_state = $smoke.analysis_state
            candidate_origin = $smoke.candidate_origin
            advisor_capability = $smoke.advisor_capability
            explain_runtime_state = $smoke.explain_runtime_state
            explain_graph_state = $smoke.explain_graph_state
            explain_graph_nodes = $smoke.explain_graph_nodes
            explain_graph_edges = $smoke.explain_graph_edges
            explain_adapter_version = $smoke.explain_adapter_version
            forbidden_statement_status = $smoke.forbidden_statement_status
            runtime_top_query_id = $smoke.runtime_top_query_id
            runtime_top_analysis_state = $smoke.runtime_top_analysis_state
            runtime_top_graph_nodes = $smoke.runtime_top_graph_nodes
            runtime_top_graph_edges = $smoke.runtime_top_graph_edges
            runtime_top_operator_rows = $smoke.runtime_top_operator_rows
            runtime_top_summary_nodes = $smoke.runtime_top_summary_nodes
            runtime_top_findings = $smoke.runtime_top_findings
            runtime_top_forbidden_statement_status = $smoke.runtime_top_forbidden_statement_status
        }
        $summary.profile_hash = $capture.source_hash
    }

    $stage = 'browser_e2e'
    if ($AuditReleaseAcceptance) {
        $env:E2E_ARTIFACT_ROOT = $auditArtifactRoot
        if (-not [string]::IsNullOrWhiteSpace($AuditResourceBuilder)) {
            if (-not $startedApi -or $startedApi.HasExited) { throw 'Resource probe requires live runner-owned API.' }
            $env:E2E_AUDIT_RESOURCE_BUILDER = $AuditResourceBuilder
            $env:E2E_AUDIT_RESOURCE_OWNED = '1'
            $env:E2E_AUDIT_RESOURCE_DOCKER = $DockerExecutable
            $env:E2E_AUDIT_RESOURCE_PWSH = (Get-Process -Id $PID).Path
            $summary.audit_resources = [ordered]@{ status='NOT_RUN'; attribution='resource_observation_only'; request_attribution='NOT_VERIFIED' }
        }
    }
    $env:E2E_BASE_URL = "http://127.0.0.1:$ApiPort"
    & rtk npm run test:e2e -- --project $Project
    $browserExitCode = $LASTEXITCODE
    if (-not [string]::IsNullOrWhiteSpace($AuditResourceBuilder)) {
        $resourceReceipts = @(Get-ChildItem -LiteralPath $auditArtifactRoot -Filter 'browser-e2e-receipt.json' -Recurse -File)
        if ($resourceReceipts.Count -eq 1) {
            $resourceBrowser = Get-Content -LiteralPath $resourceReceipts[0].FullName -Raw | ConvertFrom-Json
            $resourceLink = $null
            if ($null -ne $resourceBrowser.PSObject.Properties['status'] -and
                $null -ne $resourceBrowser.PSObject.Properties['audit_release'] -and
                $null -ne $resourceBrowser.audit_release -and
                $null -ne $resourceBrowser.audit_release.PSObject.Properties['resource_probe']) {
                $resourceLink = $resourceBrowser.audit_release.resource_probe
            }
            if ($null -ne $resourceLink) {
                $summary.audit_resources = $resourceLink
                if ($null -ne $resourceLink.PSObject.Properties['path'] -and $resourceLink.path) {
                    $resourcePath = [IO.Path]::GetFullPath($resourceLink.path)
                    if (-not $resourcePath.StartsWith(([IO.Path]::GetFullPath($auditArtifactRoot) + [IO.Path]::DirectorySeparatorChar), [StringComparison]::OrdinalIgnoreCase)) { throw 'Resource receipt escaped owned artifact directory.' }
                    if ((Get-Sha256 $resourcePath) -ine $resourceLink.sha256) { throw 'Resource receipt hash mismatch.' }
                }
            }
        }
        $resourceWindowNotExercised = $summary.audit_resources.status -eq 'NOT_RUN' -and
            $null -ne $summary.audit_resources.PSObject.Properties['reason'] -and
            $summary.audit_resources.reason -eq 'observed_overview_not_thirty_days'
        if ($browserExitCode -eq 0 -and $summary.audit_resources.status -ne 'OBSERVED' -and -not $resourceWindowNotExercised) { throw 'Independent resource observation missing or incomplete.' }
    }
    if ($browserExitCode -ne 0) { throw 'Browser E2E failed.' }
    if ($AuditReleaseAcceptance) {
        $stage = 'audit_release_oracle'
        $browserReceipts = @(Get-ChildItem -LiteralPath $auditArtifactRoot -Filter 'browser-e2e-receipt.json' -Recurse -File)
        if ($browserReceipts.Count -ne 1) { throw 'Expected exactly one owned audit browser receipt.' }
        $browserReceipt = $browserReceipts[0].FullName
        $browserObservation = Get-Content -LiteralPath $browserReceipt -Raw | ConvertFrom-Json
        if ($browserObservation.audit_release.missing_profile.status -ne 'PASS') { throw 'F05 real missing Profile evidence was not verified.' }
        $oraclePath = Join-Path $auditArtifactRoot 'audit-release-oracle.json'
        $comparisonPath = Join-Path $auditArtifactRoot 'audit-release-comparison.json'
        & (Join-Path $PSScriptRoot 'audit-release-oracle.ps1') -BrowserReceipt $browserReceipt -OutputPath $oraclePath -ContainerName $fixtureContainerName
        if (-not $?) { throw 'Independent audit oracle capture failed.' }
        & node (Join-Path $repoRoot 'tests/e2e/audit-release.mjs') --receipt $browserReceipt --oracle $oraclePath --output $comparisonPath
        if ($LASTEXITCODE -ne 0) { throw 'Independent audit comparison failed.' }
        $summary.audit_release = [ordered]@{
            browser_receipt_sha256 = Get-Sha256 $browserReceipt
            oracle_sha256 = Get-Sha256 $oraclePath
            comparison_sha256 = Get-Sha256 $comparisonPath
            comparison_path = $comparisonPath
        }
    }
    $summary.status = 'PASS'
}
catch {
    # A release receipt names only the bounded stage; it never serializes
    # command output, credentials, Profile bytes, or submitted SQL.
    $primaryFailure = $true
    $summary.failure = [ordered]@{ stage = $stage; message_id = 'e2e_stage_failed' }
    throw
}
finally {
    $teardownFailure = $null
    $postconditionFailure = $null
    if ($fixtureBootstrapStarted -and -not $KeepFixture) {
        try {
            & (Join-Path $repoRoot 'dev\starrocks-3.5.14\teardown.ps1') -DockerExecutable $DockerExecutable
        }
        catch {
            $teardownFailure = $_
        }
    }
    if ($startedApi -and -not $KeepStartedApi -and -not $startedApi.HasExited) {
        try { Stop-Process -Id $startedApi.Id -Force -ErrorAction Stop; $startedApi.WaitForExit(5000) | Out-Null }
        catch { $postconditionFailure = $_ }
    }
    if ($RequireOwnedApi -and ($startedApi -or $fixtureBootstrapStarted)) {
        try {
            if ($startedApi) { Wait-TcpPortFree $ApiPort }
            $containerAbsentAfter = if ($fixtureBootstrapStarted) {
                $null -eq (Get-FixtureContainerIdentity -DockerExecutable $fixtureDocker -ContainerName $fixtureContainerName)
            } else { $true }
            $dataRootAbsentAfter = if ($fixtureBootstrapStarted) { -not (Test-Path -LiteralPath $fixtureDataRoot) } else { $true }
            $apiPortFreeAfter = if ($startedApi) { -not (Test-TcpPortOccupied $ApiPort) } else { $true }
            $summary.postconditions = [ordered]@{
                fixture_container_absent = $containerAbsentAfter
                fixture_data_root_absent = $dataRootAbsentAfter
                api_port_free = $apiPortFreeAfter
            }
            if (-not ($containerAbsentAfter -and $dataRootAbsentAfter -and $apiPortFreeAfter)) {
                throw 'Release connected E2E teardown postconditions were not satisfied.'
            }
        }
        catch { $postconditionFailure = $_ }
    }
    else {
        $summary.postconditions = [ordered]@{ mode = 'not_release_verified' }
    }
    $summary.teardown = if ($Project -ne 'connected-readonly-docker') { 'not_applicable' } elseif (-not $fixtureBootstrapStarted) { 'not_started' } elseif ($teardownFailure -or $postconditionFailure) { 'failed' } else { 'passed' }
    if ($teardownFailure -or $postconditionFailure) {
        $summary.status = 'FAIL'
        $summary.cleanup_failure = [ordered]@{ stage = 'bounded_teardown'; message_id = 'e2e_teardown_failed'; fixture_teardown_failed = [bool]$teardownFailure; postcondition_failed = [bool]$postconditionFailure }
        if (-not $primaryFailure) { $summary.failure = $summary.cleanup_failure }
    }
    if ($AuditReleaseAcceptance) {
        $summary.api_log_scan = [ordered]@{ status = 'NOT_RUN'; reason = 'owned_api_not_stopped'; flush_completed = $false }
        if ($startedApi -and -not $KeepStartedApi -and $startedApi.HasExited) {
            try {
                $scan = & node (Join-Path $repoRoot 'tests/e2e/audit-release.mjs') --scan-owned-logs (Join-Path $runtimeRoot 'doctor-api-e2e.stdout.log') (Join-Path $runtimeRoot 'doctor-api-e2e.stderr.log')
                if ($LASTEXITCODE -ne 0) { throw 'Owned API log scanner failed.' }
                $summary.api_log_scan = $scan | ConvertFrom-Json
                $summary.api_log_scan | Add-Member -NotePropertyName flush_completed -NotePropertyValue $true
                $summary.api_log_scan | Add-Member -NotePropertyName flush_scope -NotePropertyValue 'redirected_files_after_process_exit_not_application_buffer_flush'
                if ($summary.api_log_scan.status -eq 'FAIL') { $postconditionFailure = 'owned_api_log_leak'; $summary.status = 'FAIL' }
            } catch {
                $summary.api_log_scan = [ordered]@{ status = 'NOT_RUN'; reason = 'scanner_failed'; flush_completed = $true }
                $postconditionFailure = 'owned_api_log_scan_failed'; $summary.status = 'FAIL'
            }
        }
    }
    $env:DOCTOR_FRONTEND_DIST = $previousFrontendDist
    $env:DOCTOR_PORT = $previousDoctorPort
    $env:DOCTOR_HISTORY = $previousDoctorHistory
    $env:STARROCKS_USER = $previousUser
    $env:STARROCKS_PASSWORD = $previousPassword
    $env:E2E_BASELINE_QUERY_ID = $previousBaseline
    $env:E2E_CANDIDATE_QUERY_ID = $previousCandidate
    $env:E2E_BUILD_IDENTITY_PATH = $previousBuildIdentity
    $env:E2E_ARTIFACT_ROOT = $previousArtifactRoot
    foreach ($name in $resourceEnvNames) { [Environment]::SetEnvironmentVariable($name, $previousResourceEnv[$name], 'Process') }
    $env:E2E_BASE_URL = $previousBaseUrl
    $env:E2E_MISSING_PROFILE_DIGEST = $previousMissingProfileDigest
    $env:E2E_MISSING_PROFILE_QUERY_ID = $previousMissingProfileQueryId
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
    [IO.File]::WriteAllText($releaseReceiptPath, ($summary | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
    if (-not $primaryFailure -and ($teardownFailure -or $postconditionFailure)) { throw 'Dedicated E2E teardown or postcondition verification failed.' }
}
# Success is emitted only after cleanup and final receipt persistence succeed.
$summary | ConvertTo-Json -Compress
