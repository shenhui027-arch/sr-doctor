[CmdletBinding()]
param(
    [Parameter(Mandatory)][int]$ApiPort,
    [Parameter(Mandatory)][string]$ConnectionId,
    [Parameter(Mandatory)][string]$CsrfNonce,
    [Parameter(Mandatory)][string]$Session,
    [Parameter(Mandatory)][string]$OutDir,
    # The driver runs only while "$OutDir\phase" holds this phase.
    [Parameter(Mandatory)][string]$RunPhase,
    # Seconds between two actions: one engineer clicking through the product.
    [ValidateRange(0, 60)][double]$PauseSeconds = 1,
    # Every Nth cycle also runs one Runtime Top (a real, user-chosen SELECT).
    [ValidateRange(0, 100)][int]$RuntimeTopEvery = 3
)

# Product activity for the multi-BE product-footprint check (v0.6-10). Drives only the
# product's own HTTP API, as the browser would; it never talks to StarRocks.
# Runs while "$OutDir\phase" equals -RunPhase. Each action appends:
# phase,session,action,latency_ms,status
$ErrorActionPreference = 'Stop'
$apiBase = "http://127.0.0.1:$ApiPort/api/v1"
$headers = @{ 'X-Doctor-CSRF' = $CsrfNonce }
$csv = Join-Path $OutDir "product-$Session.csv"
$explainSql = 'SELECT bucket, COUNT(*) FROM e2e_fixture.profile_rows WHERE id >= 0 GROUP BY bucket ORDER BY bucket'
$runtimeSql = 'SELECT payload, COUNT(*) AS footprint_runtime FROM e2e_fixture.profile_rows GROUP BY payload'

function Get-Phase { $path = Join-Path $OutDir 'phase'; if (Test-Path -LiteralPath $path) { (Get-Content -LiteralPath $path -Raw).Trim() } else { 'warmup' } }
function Scope([int]$Seconds) {
    $end = [int][DateTimeOffset]::UtcNow.ToUnixTimeSeconds() + 1
    @{ window = @{ start_seconds = $end - $Seconds; end_seconds = $end }; state = $null; user = $null; native_digest = $null
       catalog = $null; database = $null; resource_group = $null; statement_filter = 'exclude_doctor_and_non_query' }
}
function Invoke-Action([string]$Name, [string]$Path, $Body) {
    $phase = Get-Phase
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $status = 200; $result = $null
    try {
        $result = Invoke-RestMethod "$apiBase/connections/$ConnectionId/$Path" -Method Post -Headers $headers -ContentType 'application/json' `
            -Body ($Body | ConvertTo-Json -Depth 20 -Compress) -TimeoutSec 60
    }
    catch {
        $response = $_.Exception.Response
        $status = if ($null -ne $response) { [int]$response.StatusCode } else { 0 }
    }
    $watch.Stop()
    Add-Content -LiteralPath $csv -Value "$phase,$Session,$Name,$($watch.ElapsedMilliseconds),$status"
    if ($PauseSeconds -gt 0) { Start-Sleep -Milliseconds ([int]($PauseSeconds * 1000)) }
    $result
}

$cycle = 0
while ((Get-Phase) -ceq $RunPhase) {
    $cycle += 1
    $null = Invoke-Action 'audit_overview_1h' 'audit/overview' @{ connection_id = $ConnectionId; request = @{
        scope = (Scope 3600); primary = @{ state = 'families'; sort = 'cumulative_query_time_desc'; limit = 50 }; family_top_sort = 'cumulative_query_time_desc' } }
    $null = Invoke-Action 'audit_overview_30d' 'audit/overview' @{ connection_id = $ConnectionId; request = @{
        scope = (Scope (30 * 86400 - 60)); primary = @{ state = 'occurrences'; sort = 'query_time_desc'; limit = 50 }; family_top_sort = 'cumulative_cpu_cost_desc' } }
    $null = Invoke-Action 'audit_occurrences_1h' 'audit/occurrences' @{ connection_id = $ConnectionId; request = @{
        scope = (Scope 3600); sort = 'newest_first'; limit = 100 } }
    $null = Invoke-Action 'audit_families_24h' 'audit/families' @{ connection_id = $ConnectionId; request = @{
        scope = (Scope 86400); sort = 'cumulative_cpu_cost_desc'; limit = 50 } }
    $queries = Invoke-Action 'query_search' 'queries/search' @{ connection_id = $ConnectionId; limit = 100; status = $null; digest = $null }
    $candidate = if ($null -ne $queries) { @($queries.queries | Where-Object { $_.profile_fetch.state -eq 'available' }) | Select-Object -First 1 } else { $null }
    if ($null -ne $candidate) {
        $generation = [int]$queries.discovery_generation
        $null = Invoke-Action 'profile_fetch' 'profiles/fetch' @{ connection_id = $ConnectionId; query_id = $candidate.query_id; discovery_generation = $generation; kind = 'baseline' }
        $null = Invoke-Action 'official_analysis' 'profiles/official-analysis' @{ connection_id = $ConnectionId; query_id = $candidate.query_id; node_ids = @()
            identity = @{ kind = 'discovery'; discovery_generation = $generation } }
    }
    $null = Invoke-Action 'plan_check' 'explain' @{ input_kind = 'user_sql'; connection_id = $ConnectionId; candidate_sql = $explainSql }
    if ($RuntimeTopEvery -gt 0 -and ($cycle % $RuntimeTopEvery) -eq 0) {
        $null = Invoke-Action 'runtime_top' 'runtime-profile' @{ connection_id = $ConnectionId; candidate_sql = $runtimeSql }
    }
}
