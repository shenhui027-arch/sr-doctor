[CmdletBinding()]
param(
    [string]$DockerExecutable = $env:DOCKER_EXE,
    [ValidateRange(1024, 65535)][int]$MySqlPort = 39183,
    [ValidateRange(1024, 65535)][int]$RelayPort = 39184,
    # Three owned API processes that differ only in their TLS trust anchor:
    # the fixture CA, an unrelated CA, and the platform store.
    [ValidateRange(1024, 65535)][int]$FixtureCaApiPort = 18071,
    [ValidateRange(1024, 65535)][int]$UnrelatedCaApiPort = 18072,
    [ValidateRange(1024, 65535)][int]$PlatformRootsApiPort = 18073,
    # Skip the incremental `cargo build --locked -p doctor-api`; the hashed
    # binary in the receipt then describes whatever target\debug holds.
    [switch]$SkipBuild,
    # Start the TLS fixture with -LowResource (2.5 CPUs / 5.25 GiB).
    [switch]$LowResource
)

# v0.7-01: product connections with TLS verify-full against a real StarRocks
# 3.5.14 whose MySQL protocol has official SSL enabled (dev/starrocks-3.5.14/
# tls-common.ps1). HTTP only, small data, logic checks; not a load test and
# not Production-safe evidence. The product trusts the run's one-off CA only
# through the process environment SSL_CERT_FILE (rustls-native-certs 0.8.4).
$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$fixtureDir = Join-Path $repoRoot 'dev\starrocks-3.5.14'
. (Join-Path $fixtureDir 'common.ps1')
. (Join-Path $fixtureDir 'tls-common.ps1')
$runtimeRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'e2e')
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$receiptPath = Join-Path $runtimeRoot "tls-verify-full-$stamp.json"
$containerName = $script:TlsFixtureContainerName
$dataRoot = Join-Path ($env:SR_DOCTOR_CONTAINER_ROOT ?? (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'containers')) $containerName
$materialRoot = Join-Path ([IO.Path]::GetTempPath()) ('starrocks-tls-e2e-' + [Guid]::NewGuid().ToString('N'))
$apiPorts = [ordered]@{ fixture_ca = $FixtureCaApiPort; unrelated_ca = $UnrelatedCaApiPort; platform_roots = $PlatformRootsApiPort }
$allPorts = @($MySqlPort, $RelayPort) + @($apiPorts.Values)
$runtimeSql = 'SELECT COUNT(*) AS tls_runtime_count FROM information_schema.columns'
$stage = 'initialization'
$summary = [ordered]@{ project = 'tls-verify-full-docker'; status = 'FAIL'; evidence = 'real_docker_http_not_production'; started_utc = [DateTime]::UtcNow.ToString('o') }
$docker = $null
$secret = $null
$keystorePassword = $null
$fixtureStarted = $false
$primaryFailure = $false
$generatedSecret = $false
$apis = [ordered]@{}
$relay = $null
$responses = [Collections.Generic.List[string]]::new()
$previous = @{}
foreach ($name in @('SSL_CERT_FILE', 'SSL_CERT_DIR', 'DOCTOR_PORT', 'DOCTOR_HISTORY', 'DOCTOR_FRONTEND_DIST')) { $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }

function Get-Sha256([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
# Docker Desktop's published ports are not visible to Get-NetTCPConnection,
# so a port counts as taken when a loopback connect succeeds.
function Test-PortListening([int]$Port) {
    $client = [Net.Sockets.TcpClient]::new()
    try { try { $client.ConnectAsync('127.0.0.1', $Port).Wait(500) -and $client.Connected } catch { $false } }
    finally { $client.Dispose() }
}
function Require([bool]$Condition, [string]$Message) { if (-not $Condition) { throw $Message } }
function Get-FixtureState($Docker) {
    [ordered]@{
        container_present = ($null -ne (Get-FixtureContainerIdentity -DockerExecutable $Docker -ContainerName $containerName))
        data_root_present = (Test-Path -LiteralPath $dataRoot)
        listening_ports = @($allPorts | Where-Object { Test-PortListening $_ })
    }
}

# Start one owned API whose only difference is the TLS trust anchor in its
# process environment: a PEM file, or neither variable for the platform store.
function Start-OwnedApi([string]$Name, [int]$Port, [string]$TrustAnchorPem) {
    [Environment]::SetEnvironmentVariable('SSL_CERT_FILE', $TrustAnchorPem, 'Process')
    [Environment]::SetEnvironmentVariable('SSL_CERT_DIR', $null, 'Process')
    $env:DOCTOR_PORT = [string]$Port
    # History is off: these checks predate stage B; B-D have their own E2Es.
    $env:DOCTOR_HISTORY = 'off'
    $env:DOCTOR_FRONTEND_DIST = $null
    $process = Start-Process -FilePath $script:apiExecutable -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $runtimeRoot "doctor-api-tls-$Name-$stamp.stdout.log") `
        -RedirectStandardError (Join-Path $runtimeRoot "doctor-api-tls-$Name-$stamp.stderr.log")
    $deadline = [DateTime]::UtcNow.AddSeconds(45)
    $capabilities = $null
    while ($null -eq $capabilities -and [DateTime]::UtcNow -lt $deadline) {
        try { $capabilities = Invoke-RestMethod "http://127.0.0.1:$Port/api/v1/capabilities" -TimeoutSec 2 } catch { Start-Sleep -Milliseconds 250 }
    }
    Require ($null -ne $capabilities) "The $Name Doctor API did not become ready."
    [pscustomobject]@{ name = $Name; port = $Port; process = $process; csrf = $capabilities.csrf.nonce; trust_anchor = $(if ($TrustAnchorPem) { 'ssl_cert_file' } else { 'platform_store' }) }
}

# Every response body is kept in memory only for the final credential scan.
function Invoke-Api($Api, [string]$Method, [string]$Path, $Payload) {
    $arguments = @{ Uri = "http://127.0.0.1:$($Api.port)/api/v1$Path"; Method = $Method; Headers = @{ 'X-Doctor-CSRF' = $Api.csrf }; SkipHttpErrorCheck = $true; TimeoutSec = 60 }
    if ($null -ne $Payload) { $arguments.ContentType = 'application/json'; $arguments.Body = ($Payload | ConvertTo-Json -Depth 20 -Compress) }
    $response = Invoke-WebRequest @arguments
    $content = [string]$response.Content
    $script:responses.Add($content)
    $json = $null
    if (-not [string]::IsNullOrWhiteSpace($content)) { try { $json = $content | ConvertFrom-Json } catch { } }
    [pscustomobject]@{ status = [int]$response.StatusCode; json = $json }
}

function Connect-Product($Api, [string]$TargetHost, [int]$Port, [string]$TlsMode) {
    Invoke-Api $Api 'Post' '/connections' @{
        host = $TargetHost; port = $Port; username = 'doctor_reader'; password = (Get-FixturePassword -Secret $script:secret -Role doctor_reader)
        catalog = $null; database = $null; audit_table = $null; tls_mode = $TlsMode
    }
}

# A refused connection returns an error envelope and never a session id.
function Get-Refusal($Response) {
    $code = if ($null -ne $Response.json -and $null -ne $Response.json.PSObject.Properties['code']) { $Response.json.code } else { $null }
    $session = $null -ne $Response.json -and $null -ne $Response.json.PSObject.Properties['connection_id']
    [ordered]@{ http_status = $Response.status; code = $code; session_created = $session }
}

function Join-Map($First, $Second) {
    $merged = [ordered]@{}
    foreach ($map in @($First, $Second)) { foreach ($key in $map.Keys) { $merged[$key] = $map[$key] } }
    $merged
}

function Invoke-RuntimeTop($Api, [string]$ConnectionId) {
    Invoke-Api $Api 'Post' "/connections/$ConnectionId/runtime-profile" @{ connection_id = $ConnectionId; candidate_sql = $runtimeSql }
}

try {
    New-Item -ItemType Directory -Force -Path $runtimeRoot | Out-Null
    $docker = Get-FixtureDockerExecutable $DockerExecutable
    $summary.evidence_sources = [ordered]@{
        runner_sha256 = Get-Sha256 $PSCommandPath
        tls_common_sha256 = Get-Sha256 (Join-Path $fixtureDir 'tls-common.ps1')
        relay_sha256 = Get-Sha256 (Join-Path $PSScriptRoot 'tls-strip-relay.mjs')
        teardown_sha256 = Get-Sha256 (Join-Path $fixtureDir 'teardown.ps1')
    }

    $stage = 'cold_start_preconditions'
    $before = Get-FixtureState $docker
    $summary.preconditions = [ordered]@{
        container_absent = (-not $before.container_present); data_root_absent = (-not $before.data_root_present)
        ports_free = ($before.listening_ports.Count -eq 0)
    }
    Require (@($summary.preconditions.Values | Where-Object { -not $_ }).Count -eq 0) 'TLS E2E requires an absent fixture container/data root and free ports.'

    # Process-only fixture token and keystore password; neither is printed,
    # passed as an argument, or written outside the dedicated container.
    if ([string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))) {
        $bytes = [byte[]]::new(30); [Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
        [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', [Convert]::ToBase64String($bytes).Replace('+', '-').Replace('/', '_').TrimEnd('='), 'Process')
        $generatedSecret = $true
    }
    $secret = Get-FixtureSecret
    $summary.secret_source = if ($generatedSecret) { 'generated_in_process' } else { 'caller_process_environment' }
    $keystoreBytes = [byte[]]::new(24); [Security.Cryptography.RandomNumberGenerator]::Fill($keystoreBytes)
    $keystorePassword = [Convert]::ToHexString($keystoreBytes).ToLowerInvariant()

    if (-not $SkipBuild) {
        $stage = 'repository_build'
        & cargo build --locked -p doctor-api
        if ($LASTEXITCODE -ne 0) { throw 'Repository API build failed.' }
    }
    $script:apiExecutable = Join-Path $repoRoot 'target\debug\doctor-api.exe'
    Require (Test-Path -LiteralPath $script:apiExecutable -PathType Leaf) 'doctor-api.exe is missing.'
    $summary.repository_build = if ($SkipBuild) { 'skipped_by_request' } else { 'rebuilt_from_current_source' }
    $summary.git_head = (& git -C $repoRoot rev-parse HEAD).Trim()
    $summary.api_sha256 = Get-Sha256 $script:apiExecutable

    $stage = 'tls_material'
    $material = New-TlsFixtureMaterial -Directory $materialRoot -KeystorePassword $keystorePassword
    $summary.tls_material = [ordered]@{
        generator = 'dotnet_certificate_request_rsa2048_sha256'; keystore_format = $material.keystore_format
        ca_sha256 = $material.ca_sha256; unrelated_ca_sha256 = $material.unrelated_ca_sha256
        valid_leaf = $material.valid_leaf; expired_leaf = $material.expired_leaf
    }

    $stage = 'fixture_up'
    $fixtureStarted = $true
    $up = Start-TlsFixture -DockerExecutable $docker -KeystoreDirectory $material.keystore_directory -KeystorePassword $keystorePassword -DataRoot $dataRoot -MySqlPort $MySqlPort -LowResource:$LowResource
    # The keystores now exist only inside the dedicated container.
    Remove-Item -LiteralPath $material.keystore_directory -Recurse -Force
    $summary.fixture = [ordered]@{
        container = $up.container; image = $up.image; observed_version = $up.observed_version; host_ports = $up.host_ports; resources = $up.resources
        fe_ssl_settings = @('ssl_keystore_location', 'ssl_keystore_password', 'ssl_key_password', 'ssl_force_secure_transport=true')
        host_private_keys_removed_after_copy = (-not (Test-Path -LiteralPath $material.keystore_directory))
    }
    Require $summary.fixture.host_private_keys_removed_after_copy 'Host copies of the server keystores were not removed.'
    $summary.fixture.secure_transport = Assert-TlsFixtureSecureTransport -DockerExecutable $docker
    Require (-not [string]::IsNullOrWhiteSpace($summary.fixture.secure_transport.in_container_client_cipher)) 'The FE did not negotiate SSL with its own client.'
    New-TlsFixtureReader -DockerExecutable $docker -Secret $secret

    $stage = 'api_start'
    $apis.fixture_ca = Start-OwnedApi 'fixture-ca' $FixtureCaApiPort $material.ca_pem
    $apis.unrelated_ca = Start-OwnedApi 'unrelated-ca' $UnrelatedCaApiPort $material.unrelated_ca_pem
    $apis.platform_roots = Start-OwnedApi 'platform-roots' $PlatformRootsApiPort $null
    $summary.apis = [ordered]@{}
    foreach ($key in $apis.Keys) { $summary.apis[$key] = [ordered]@{ port = $apis[$key].port; trust_anchor = $apis[$key].trust_anchor } }
    $cases = [ordered]@{}

    # T1: matching certificate and host name -> session and a real Profile.
    $stage = 'verify_full_success'
    $connected = Connect-Product $apis.fixture_ca $script:TlsFixtureServerName $MySqlPort 'verify_full'
    Require ($connected.status -eq 200 -and $null -ne $connected.json.connection_id) 'verify_full to the matching host name did not connect.'
    $connectionId = $connected.json.connection_id
    Require ($connected.json.detected_version.value -match '^3\.5\.14(?:-|$)') 'Wrong StarRocks version over TLS.'
    $runtime = Invoke-RuntimeTop $apis.fixture_ca $connectionId
    $report = $runtime.json.outcome.report
    Require ($runtime.status -eq 200 -and $runtime.json.state -eq 'fetched' -and $null -ne $report) 'Runtime Top over TLS did not return an analyzed Profile.'
    Require ($report.query_summary.query_id -eq $runtime.json.query.query_id) 'Runtime Top over TLS lost the exact query identity.'
    Require ($runtime.json.source_refs -contains 'runtime:get_query_profile') 'The Profile was not pulled with get_query_profile.'
    Require (@($report.operator_rows).Count -gt 0 -and @($report.graph_view.nodes).Count -gt 0) 'The TLS-pulled Profile produced no operators or topology.'
    $cases.verify_full_success = [ordered]@{
        host = $script:TlsFixtureServerName; tls_mode = 'verify_full'; http_status = $connected.status
        session_state = $connected.json.state; detected_version = $connected.json.detected_version.value
        profile = [ordered]@{ state = $runtime.json.state; source_refs = @($runtime.json.source_refs); analysis_state = $report.analysis_state
            operator_rows = @($report.operator_rows).Count; graph_nodes = @($report.graph_view.nodes).Count; source_hash = $runtime.json.source_hash }
    }

    # T2: same server, address not in the certificate -> tls_failed.
    $stage = 'hostname_mismatch'
    $mismatch = Get-Refusal (Connect-Product $apis.fixture_ca '127.0.0.1' $MySqlPort 'verify_full')
    Require ($mismatch.code -eq 'tls_failed' -and -not $mismatch.session_created) 'A host name absent from the certificate was not refused with tls_failed.'
    $cases.hostname_mismatch = Join-Map ([ordered]@{ host = '127.0.0.1'; certificate_names = @("DNS:$script:TlsFixtureServerName") }) $mismatch

    # T3: a chain the process does not trust -> tls_failed.
    $stage = 'untrusted_chain'
    $unrelated = Get-Refusal (Connect-Product $apis.unrelated_ca $script:TlsFixtureServerName $MySqlPort 'verify_full')
    $platform = Get-Refusal (Connect-Product $apis.platform_roots $script:TlsFixtureServerName $MySqlPort 'verify_full')
    Require ($unrelated.code -eq 'tls_failed' -and -not $unrelated.session_created) 'A chain outside the configured trust anchor was accepted.'
    Require ($platform.code -eq 'tls_failed' -and -not $platform.session_created) 'The one-off CA was accepted by the platform store.'
    $cases.untrusted_unrelated_ca = $unrelated
    $cases.untrusted_platform_store = $platform

    # T4: plaintext downgrade attempts.
    $stage = 'downgrade'
    # (a) Product request: plaintext to a non-literal-loopback host is refused before any network IO.
    $requested = Get-Refusal (Connect-Product $apis.fixture_ca $script:TlsFixtureServerName $MySqlPort 'docker_loopback_plaintext')
    Require ($requested.http_status -eq 400 -and $requested.code -eq 'tls_required' -and -not $requested.session_created) 'A plaintext request to a non-loopback host name was not refused.'
    $cases.downgrade_requested_plaintext = $requested
    # (b) Wire: the real greeting reaches the product without CLIENT_SSL.
    $relayReport = Join-Path $runtimeRoot "tls-strip-relay-$stamp.json"
    $relay = Start-Process -FilePath 'node' -ArgumentList @((Join-Path $PSScriptRoot 'tls-strip-relay.mjs'), [string]$RelayPort, [string]$MySqlPort, $relayReport) -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
        -RedirectStandardOutput (Join-Path $runtimeRoot "tls-strip-relay-$stamp.stdout.log") -RedirectStandardError (Join-Path $runtimeRoot "tls-strip-relay-$stamp.stderr.log")
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while (-not (Test-Path -LiteralPath "$relayReport.listening") -and -not $relay.HasExited -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 100 }
    Require (Test-Path -LiteralPath "$relayReport.listening") 'The downgrade relay did not listen.'
    Remove-Item -LiteralPath "$relayReport.listening"
    $stripped = Get-Refusal (Connect-Product $apis.fixture_ca $script:TlsFixtureServerName $RelayPort 'verify_full')
    Require ($relay.WaitForExit(15000)) 'The downgrade relay did not finish.'
    $wire = Get-Content -LiteralPath $relayReport -Raw | ConvertFrom-Json
    Require ($stripped.code -eq 'tls_failed' -and -not $stripped.session_created) 'A greeting without CLIENT_SSL was not refused with tls_failed.'
    Require ($wire.status -eq 'completed' -and $wire.upstream_ssl_advertised -eq $true -and $wire.greeting_forwarded_without_ssl -eq $true) 'The relay did not forward a stripped real greeting.'
    Require ([int]$wire.client_bytes_sent -eq 0) 'The product sent bytes (handshake or credentials) after a stripped greeting.'
    $cases.downgrade_stripped_greeting = Join-Map $stripped ([ordered]@{ relay = $wire })
    # (c) Server side: the documented loopback plaintext exception reaches a
    # server that requires secure transport; the server refuses it.
    # The FE exempts clients it sees as 127.0.0.1; Docker's port forwarding
    # presents a non-loopback address, so the FE log must record the refusal.
    $refusalsBefore = Get-TlsFixtureInsecureRefusalCount -DockerExecutable $docker
    $serverRefused = Get-Refusal (Connect-Product $apis.fixture_ca '127.0.0.1' $MySqlPort 'docker_loopback_plaintext')
    $refusalsAfter = Get-TlsFixtureInsecureRefusalCount -DockerExecutable $docker
    Require ($serverRefused.http_status -ge 400 -and -not $serverRefused.session_created) 'ssl_force_secure_transport did not refuse a plaintext client.'
    Require ($refusalsAfter -eq $refusalsBefore + 1) 'The FE did not log exactly one insecure-transport refusal.'
    $cases.server_force_secure_transport = Join-Map $serverRefused ([ordered]@{ fe_insecure_transport_refusals_logged = $refusalsAfter - $refusalsBefore })

    # T5: the established session is discarded once its transport fails, and
    # T6: the restarted FE's expired certificate is refused.
    $stage = 'expired_certificate_and_session_discard'
    Use-TlsFixtureExpiredCertificate -DockerExecutable $docker
    $broken = Invoke-RuntimeTop $apis.fixture_ca $connectionId
    $brokenCode = if ($null -ne $broken.json -and $null -ne $broken.json.PSObject.Properties['code']) { $broken.json.code } else { $null }
    $after = Invoke-RuntimeTop $apis.fixture_ca $connectionId
    $afterCode = if ($null -ne $after.json -and $null -ne $after.json.PSObject.Properties['code']) { $after.json.code } else { $null }
    Require ($broken.status -ge 400 -and $brokenCode -in @('connection_failed', 'tls_failed', 'remote_timeout')) 'An operation on a broken TLS session did not fail with a discard class.'
    Require ($after.status -eq 410 -and $afterCode -eq 'connection_expired') 'The failed TLS session was not discarded.'
    $cases.session_discarded_after_failure = [ordered]@{ first_operation = [ordered]@{ http_status = $broken.status; code = $brokenCode }; next_operation = [ordered]@{ http_status = $after.status; code = $afterCode } }
    $expired = Get-Refusal (Connect-Product $apis.fixture_ca $script:TlsFixtureServerName $MySqlPort 'verify_full')
    Require ($expired.code -eq 'tls_failed' -and -not $expired.session_created) 'An expired server certificate was accepted.'
    $cases.expired_certificate = Join-Map $expired ([ordered]@{ not_after_utc = $material.expired_leaf.not_after_utc })

    # Credentials never appear in any response body.
    $stage = 'response_credential_scan'
    $needles = @($secret, (Get-FixturePassword -Secret $secret -Role doctor_reader), $keystorePassword)
    $leakingResponses = @($responses | Where-Object { $body = $_; @($needles | Where-Object { $body.Contains($_) }).Count -gt 0 }).Count
    $summary.response_scan = [ordered]@{ status = $(if ($leakingResponses -eq 0) { 'PASS' } else { 'FAIL' }); responses = $responses.Count }
    Require ($leakingResponses -eq 0) 'A product response contained a credential.'
    $summary.cases = $cases
    $summary.status = 'PASS'
}
catch {
    # The receipt names only the bounded stage: no command output, SQL,
    # credentials or Profile bytes.
    $primaryFailure = $true
    $summary.failure = [ordered]@{ stage = $stage; message_id = 'e2e_stage_failed' }
    throw
}
finally {
    $cleanupFailure = $null
    if ($null -ne $relay -and -not $relay.HasExited) { try { Stop-Process -Id $relay.Id -Force -ErrorAction Stop; $relay.WaitForExit(5000) | Out-Null } catch { $cleanupFailure = 'relay_stop' } }
    foreach ($api in $apis.Values) {
        if (-not $api.process.HasExited) { try { Stop-Process -Id $api.process.Id -Force -ErrorAction Stop; $api.process.WaitForExit(5000) | Out-Null } catch { $cleanupFailure = 'api_stop' } }
    }
    if ($apis.Count -gt 0 -and $null -ne $secret) {
        # Owned API and relay logs must not contain the token, the reader or
        # keystore password, or the submitted SQL; only counts are recorded.
        $needles = @($secret, (Get-FixturePassword -Secret $secret -Role doctor_reader), 'tls_runtime_count')
        if ($null -ne $keystorePassword) { $needles += $keystorePassword }
        $logs = @(Get-ChildItem -LiteralPath $runtimeRoot -File -Filter "*-$stamp.*.log" | Where-Object { $_.Name -like 'doctor-api-tls-*' -or $_.Name -like 'tls-strip-relay-*' } | ForEach-Object FullName)
        $leaked = @($logs | Where-Object { $text = [IO.File]::ReadAllText($_); @($needles | Where-Object { $text.Contains($_) }).Count -gt 0 }).Count
        $expectedLogs = 2 * $apis.Count + $(if ($null -ne $relay) { 2 } else { 0 })
        $summary.log_scan = [ordered]@{ status = $(if ($logs.Count -ne $expectedLogs) { 'NOT_RUN' } elseif ($leaked -gt 0) { 'FAIL' } else { 'PASS' }); files = $logs.Count; bytes = @($logs | ForEach-Object { (Get-Item -LiteralPath $_).Length }) }
        if ($summary.log_scan.status -ne 'PASS' -and -not $cleanupFailure) { $cleanupFailure = 'log_scan' }
    }
    if ($fixtureStarted) {
        try { & (Join-Path $fixtureDir 'teardown.ps1') -ContainerName $containerName -DockerExecutable $docker }
        catch { $cleanupFailure = 'fixture_teardown' }
    }
    if (Test-Path -LiteralPath $materialRoot) { try { Remove-Item -LiteralPath $materialRoot -Recurse -Force } catch { $cleanupFailure = 'tls_material_removal' } }
    foreach ($name in $previous.Keys) { [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process') }
    if ($generatedSecret) { [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', $null, 'Process') }
    $summary.secret_cleared = (-not $generatedSecret) -or [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable('STARROCKS_E2E_SECRET', 'Process'))
    if ($null -ne $docker) {
        $final = Get-FixtureState $docker
        $summary.postconditions = [ordered]@{
            container_absent = (-not $final.container_present); data_root_absent = (-not $final.data_root_present)
            tls_material_absent = (-not (Test-Path -LiteralPath $materialRoot)); ports_free = ($final.listening_ports.Count -eq 0)
        }
        if (@($summary.postconditions.Values | Where-Object { -not $_ }).Count -gt 0 -and -not $cleanupFailure) { $cleanupFailure = 'postconditions' }
    }
    if ($cleanupFailure) {
        $summary.status = 'FAIL'
        $summary.cleanup_failure = [ordered]@{ stage = $cleanupFailure; message_id = 'e2e_teardown_failed' }
    }
    $summary.completed_utc = [DateTime]::UtcNow.ToString('o')
    [IO.File]::WriteAllText($receiptPath, ($summary | ConvertTo-Json -Depth 10), [Text.UTF8Encoding]::new($false))
    if (-not $primaryFailure -and $cleanupFailure) { throw 'TLS E2E teardown or postcondition verification failed.' }
}
$summary.receipt_path = $receiptPath
$summary | ConvertTo-Json -Compress -Depth 10
