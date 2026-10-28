param(
    [string]$SourcePath = (Join-Path $PSScriptRoot 'audit-workload-smoke.ps1')
)

# Do not dot-source the smoke harness: its top level creates artifacts and may
# call Docker/API. Parse it, import only the explicit pure SQL builders, then
# execute those builders against fixed in-memory inputs.
$ErrorActionPreference = 'Stop'
$script:assertionCount = 0

function Require([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:assertionCount++
}

$resolvedSourcePath = (Resolve-Path -LiteralPath $SourcePath -ErrorAction Stop).Path
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($resolvedSourcePath, [ref]$tokens, [ref]$errors)
if ($errors.Count -gt 0) { throw ($errors | Out-String) }

$pureFunctionNames = @('Sql-String', 'Sql-Decimal', 'New-OracleRecord', 'Format-OracleRecord', 'New-HalfOpenWindowCountSql', 'New-FixtureSql')
$functionSources = @{}
foreach ($functionAst in @($ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $true))) {
    if ($pureFunctionNames -contains $functionAst.Name) {
        if ($functionSources.ContainsKey($functionAst.Name)) { throw "Duplicate pure function $($functionAst.Name)." }
        $functionSources[$functionAst.Name] = $functionAst.Extent.Text
    }
}
foreach ($name in $pureFunctionNames) {
    if (-not $functionSources.ContainsKey($name)) { throw "Pure function $name was not found." }
    . ([scriptblock]::Create($functionSources[$name]))
}

$windowStart = [Int64]1788048000
$windowEnd = $windowStart + 3600
$fixtureQualifiedTable = 'doctor_v2_oracle.audit_workload_events'
$sourceSql = New-FixtureSql
$halfOpenSql = New-HalfOpenWindowCountSql -QualifiedTable $fixtureQualifiedTable -WindowStart $windowStart -WindowEnd $windowEnd

Require ((Sql-String $null) -eq 'NULL') 'NULL text input was coerced to an empty SQL string.'
Require ((Sql-Decimal $null) -eq 'NULL') 'NULL decimal input was coerced to an empty SQL value.'
Require ((Sql-String '') -eq "''") 'An empty text value was confused with SQL NULL.'
Require ((Sql-String "a'b") -eq "'a''b'") 'Text SQL escaping drifted.'
Require ($sourceSql.IndexOf([char]9) -eq -1 -and $halfOpenSql.IndexOf([char]9) -eq -1) 'Generated SQL contains an unexpected tab character.'
Require ($halfOpenSql -eq 'SELECT COUNT(*) AS expected_raw_window_count FROM doctor_v2_oracle.audit_workload_events WHERE `timestamp` >= FROM_UNIXTIME(1788048000) AND `timestamp` < FROM_UNIXTIME(1788051600);') 'Half-open count SQL has incorrect timestamp quoting or bounds.'
Require ($sourceSql.Contains($halfOpenSql)) 'Fixture artifact omits the shared half-open count SQL.'

$columnMatches = @([regex]::Matches($sourceSql, '(?m)^  `[^`]+` (?:VARCHAR|DATETIME|BIGINT)'))
Require ($columnMatches.Count -eq 15) 'Fixture DDL no longer has 15 audit columns including optional user.'
Require ($sourceSql.Contains('  `user` VARCHAR(256) NULL')) 'Fixture DDL omitted the explicit user column.'
foreach ($columnName in @('queryTime', 'scanBytes', 'scanRows', 'cpuCostNs', 'memCostBytes', 'returnRows')) {
    Require ($sourceSql.Contains("  ``$columnName`` BIGINT NULL")) "Fixture DDL column $columnName is not the official signed BIGINT type."
}
Require (-not $sourceSql.Contains('DECIMAL(38,0)')) 'Fixture DDL still uses the previous DECIMAL metric type.'
Require (-not $sourceSql.Contains('BIGINT UNSIGNED')) 'Fixture DDL must use signed BIGINT, not BIGINT UNSIGNED.'
$rowPattern = [regex]::new("\('(?<id>[^']+)', FROM_UNIXTIME\((?<event_seconds>\d+)\), '(?<state>EOF|ERR)', (?<query_time>NULL|-?\d+), (?<scan_bytes>NULL|-?\d+), (?<scan_rows>NULL|-?\d+), (?<cpu_cost>NULL|-?\d+), (?<memory_cost>NULL|-?\d+),[^\r\n]+, '(?<user>fixture_user_[ab])'\)")
$rows = @($rowPattern.Matches($sourceSql))
Require ($rows.Count -eq 608) 'Fixture no longer emits exactly 608 source events.'
$halfOpenRows = @($rows | Where-Object {
    $eventSeconds = [Int64]$_.Groups['event_seconds'].Value
    $eventSeconds -ge $windowStart -and $eventSeconds -lt $windowEnd
})
Require ($halfOpenRows.Count -eq 607) 'Fixture half-open window no longer has exactly 607 source rows.'
$userARows = @($halfOpenRows | Where-Object { $_.Groups['user'].Value -eq 'fixture_user_a' })
$userBRows = @($halfOpenRows | Where-Object { $_.Groups['user'].Value -eq 'fixture_user_b' })
Require ($userARows.Count -eq 305 -and $userBRows.Count -eq 302) 'User-specific raw window counts drifted.'
$userBErrorRows = @($userBRows | Where-Object { $_.Groups['state'].Value -eq 'ERR' })
Require ($userBErrorRows.Count -eq 2) 'Combined user/state fixture must contain exactly two errors.'
Require ((@($userBErrorRows | ForEach-Object { $_.Groups['id'].Value } | Sort-Object) -join ',') -eq '00000000-0000-0000-0000-000000000020,00000000-0000-0000-0000-000000000022') 'Combined user/state fixture identities drifted.'
Require (@($userBRows | Where-Object { $_.Groups['state'].Value -eq 'EOF' }).Count -eq 300) 'Finished-state raw fixture count drifted.'
Require (@($userBRows | ForEach-Object { $_.Groups['id'].Value } | Sort-Object -Unique).Count -eq 300) 'User B canonical identity count drifted.'

$idCounts = @{}
foreach ($row in $rows) {
    $id = $row.Groups['id'].Value
    $priorCount = if ($idCounts.ContainsKey($id)) { [int]$idCounts[$id] } else { 0 }
    $idCounts[$id] = 1 + $priorCount
}
foreach ($number in 11..14) {
    $id = '00000000-0000-0000-0000-{0:D12}' -f $number
    Require ($idCounts[$id] -eq 2) "Exact duplicate fixture count drifted for $id."
}
$conflictId = '00000000-0000-0000-0000-000000000601'
Require ($idCounts[$conflictId] -eq 2) 'Conflict duplicate fixture count drifted.'
$conflictQueryTimes = @($rows | Where-Object { $_.Groups['id'].Value -eq $conflictId } | ForEach-Object { $_.Groups['query_time'].Value } | Sort-Object)
Require (($conflictQueryTimes -join ',') -eq '77,78') 'Conflict duplicate fixture values drifted.'
$endBoundaryId = '00000000-0000-0000-0000-000000000700'
$endBoundary = @($rows | Where-Object { $_.Groups['id'].Value -eq $endBoundaryId })
Require ($endBoundary.Count -eq 1 -and [Int64]$endBoundary[0].Groups['event_seconds'].Value -eq $windowEnd) 'End-boundary source event drifted.'

function Get-GeneratedRow([string]$QueryId) {
    $matches = @($rows | Where-Object { $_.Groups['id'].Value -eq $QueryId })
    Require ($matches.Count -eq 1) "Expected one generated row for $QueryId."
    $matches[0]
}

$nullMetricRow = Get-GeneratedRow '00000000-0000-0000-0000-000000000002'
$zeroMetricRow = Get-GeneratedRow '00000000-0000-0000-0000-000000000003'
$precisionUnsafeRow = Get-GeneratedRow '00000000-0000-0000-0000-000000000004'
$negativeMetricRow = Get-GeneratedRow '00000000-0000-0000-0000-000000000005'
$maximumMetricRow = Get-GeneratedRow '00000000-0000-0000-0000-000000000006'
Require ($nullMetricRow.Groups['scan_bytes'].Value -eq 'NULL') 'Fixture SQL NULL scanBytes was not emitted as SQL NULL.'
Require ($zeroMetricRow.Groups['scan_bytes'].Value -eq '0') 'Fixture numeric zero scanBytes drifted.'
Require ($precisionUnsafeRow.Groups['scan_rows'].Value -eq '9007199254740992') 'Fixture precision-unsafe scanRows drifted.'
Require ($negativeMetricRow.Groups['cpu_cost'].Value -eq '-1') 'Fixture negative sentinel drifted.'
Require ($maximumMetricRow.Groups['memory_cost'].Value -eq '9223372036854775807') 'Fixture signed BIGINT maximum input drifted.'
Require (-not $sourceSql.Contains('9223372036854775808')) 'Fixture SQL contains a value outside signed BIGINT; keep overflow coverage in offline parser tests.'

Write-Output "PASS pure_fixture_assertions=$script:assertionCount source_events=$($rows.Count) half_open_events=$($halfOpenRows.Count)"
