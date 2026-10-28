# Static coordinator checks only; no services, browser, Docker or SQL are run.
$ErrorActionPreference = 'Stop'
$path = Join-Path $PSScriptRoot 'run-browser-e2e.ps1'
$source = Get-Content -LiteralPath $path -Raw
$tokens = $null; $errors = $null
[void][Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
$checks = 0
function Check([bool]$Condition, [string]$Name) {
    if (-not $Condition) { throw "STATIC_FAIL: $Name" }
    $script:checks++
}
Check ($errors.Count -eq 0) 'PowerShell syntax'
Check ($source.IndexOf('if ($AuditReleaseAcceptance -and') -lt $source.IndexOf('try {')) 'invalid mode rejected before fixture work'
Check ($source.Contains('-not $RequireOwnedApi')) 'requires owned API'
Check ($source.Contains("'audit-release-' + [Guid]::NewGuid().ToString('N')")) 'unique artifact directory'
Check ($source.Contains('$browserReceipts.Count -ne 1')) 'exactly one owned browser receipt'
$browser = $source.IndexOf('& rtk npm run test:e2e')
$oracle = $source.IndexOf("'audit-release-oracle.ps1'")
$compare = $source.IndexOf("'tests/e2e/audit-release.mjs'")
$teardown = $source.IndexOf("'dev\starrocks-3.5.14\teardown.ps1'")
Check ($browser -gt 0 -and $browser -lt $oracle -and $oracle -lt $compare -and $compare -lt $teardown) 'independent oracle before fixture teardown'
Check ($source.Contains('if ($LASTEXITCODE -ne 0) { throw ''Independent audit comparison failed.'' }')) 'comparison exit blocks PASS'
Check ($source.Contains('$env:E2E_ARTIFACT_ROOT = $previousArtifactRoot')) 'artifact environment restored'
Check ($source.Contains('comparison_sha256 = Get-Sha256 $comparisonPath')) 'comparison receipt hash retained'
Check ($source.Contains('[switch]$DevelopmentRunner')) 'development runner is explicit'
Check ($source.Contains('if ($DevelopmentRunner -and -not [string]::IsNullOrWhiteSpace($CandidateManifest))')) 'development runner excludes Candidate'
Check ($source.Contains("if (-not `$PSBoundParameters.ContainsKey('ApiPort')) { `$ApiPort = 18049 }")) 'development default port is isolated'
Check ($source.Contains("'target\debug\examples\loopback-v2-e2e.exe'")) 'development uses existing example binary'
Check ($source.Contains('$launchArguments.ArgumentList = @([string]$ApiPort)')) 'development port passed explicitly'
Check ($source.Contains("{ 'development_only' } else { 'production_runner' }")) 'development evidence distinguished'
Check ($source.Contains('if ($fixtureBootstrapStarted -and -not $KeepFixture)')) 'fixture cleanup requires entered bootstrap'
Check ($source.IndexOf('$fixtureBootstrapStarted = $true') -lt $source.IndexOf('$bootstrap = (&')) 'partial bootstrap remains cleanup eligible'
Check ($source.Contains('if ($startedApi) { Wait-TcpPortFree $ApiPort }')) 'external API port not awaited during cleanup'
Check ($source.Contains('if ($RequireOwnedApi -and ($startedApi -or $fixtureBootstrapStarted))')) 'preflight failure skips resource postconditions'
Check ($source.Contains('$summary.cleanup_failure = [ordered]@{')) 'cleanup failure recorded separately'
Check ($source.Contains('if (-not $primaryFailure) { $summary.failure = $summary.cleanup_failure }')) 'primary receipt failure preserved'
Check ($source.Contains('if (-not $primaryFailure -and ($teardownFailure -or $postconditionFailure))')) 'cleanup throw does not replace primary exception'
Check ($source.Contains('$env:E2E_BASE_URL = "http://127.0.0.1:$ApiPort"') -and -not $source.Contains('if ($DevelopmentRunner) { $env:E2E_BASE_URL')) 'browser targets runner-owned API port'
Check ($source.Contains('$env:E2E_BASE_URL = $previousBaseUrl')) 'browser base URL restored'
Check ($source.Contains('[string]$AuditResourceBuilder') -and $source.Contains('AuditResourceBuilder requires owned AuditReleaseAcceptance')) 'resource explicit owned opt in'
Check ($source.Contains('$resourceEnvNames') -and $source.Contains('$previousResourceEnv[$name]')) 'resource env isolated restored'
Check ($source.Contains('Resource receipt hash mismatch') -and $source.Contains('Resource receipt escaped owned artifact directory')) 'resource receipt identity and path fence'
Check ($source.Contains("status='NOT_RUN'; attribution='resource_observation_only'") -and $source.Contains('Independent resource observation missing or incomplete')) 'resource no missing PASS'
# Regression: early browser FAIL has no audit_release; retain NOT_RUN and primary error.
$start = $source.IndexOf('    if (-not [string]::IsNullOrWhiteSpace($AuditResourceBuilder)) {', $source.IndexOf('$browserExitCode = $LASTEXITCODE'))
$end = $source.IndexOf('    if ($AuditReleaseAcceptance) {', $start)
$failureBlock = $source.Substring($start, $end - $start)
$failureBlock = $failureBlock.Replace('$resourceReceipts = @(Get-ChildItem -LiteralPath $auditArtifactRoot -Filter ''browser-e2e-receipt.json'' -Recurse -File)', '$resourceReceipts = @([pscustomobject]@{ FullName = ''unused'' })')
$failureBlock = $failureBlock.Replace('$resourceBrowser = Get-Content -LiteralPath $resourceReceipts[0].FullName -Raw | ConvertFrom-Json', '$resourceBrowser = [pscustomobject]@{ status = ''FAIL'' }')
$AuditResourceBuilder='enabled';$browserExitCode=1;$summary=[ordered]@{audit_resources=[ordered]@{status='NOT_RUN'}}
$primary=$null
try { & ([scriptblock]::Create($failureBlock)) } catch { $primary=$_.Exception.Message }
Check ($primary -ceq 'Browser E2E failed.' -and $summary.audit_resources.status -ceq 'NOT_RUN') 'early browser failure is not masked by missing audit resource property'
$rebuildIndex = $source.IndexOf("& npm --prefix (Join-Path `$repoRoot 'frontend') run build")
$identityIndex = $source.IndexOf('$buildIdentity = [ordered]@{')
Check ($source.Contains("@('build', '--locked', '-p', 'doctor-api')") -and $rebuildIndex -gt 0 -and $rebuildIndex -lt $identityIndex) 'repository API and frontend are rebuilt before their identity is hashed'
Check ($source.Contains('$usesRepositoryArtifacts = [string]::IsNullOrWhiteSpace($CandidateManifest)') -and $source.Contains('[switch]$SkipBuild')) 'only default repository artifacts are rebuilt, with an explicit opt-out'
Write-Output "AUDIT_RELEASE_COORDINATOR_STATIC_PASS checks=$checks"
