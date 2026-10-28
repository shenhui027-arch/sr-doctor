[CmdletBinding()]
param([Parameter(Mandatory)][string]$InputPath,[Parameter(Mandatory)][string]$OutputPath,[Parameter(Mandatory)][string]$BuilderExecutable,[string]$ContainerName='sr-doctor-v0.1-e2e-3514',[string]$DockerExecutable=$env:DOCKER_EXE)
$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot '../../dev/starrocks-3.5.14/common.ps1')
function Assert-Resource([bool]$Value,[string]$Code) { if(-not $Value){throw $Code} }
function Get-ResourceHash([string]$Value) { $sha=[Security.Cryptography.SHA256]::Create(); try { -join ($sha.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value)) | ForEach-Object {$_.ToString('x2')}) } finally {$sha.Dispose()} }
function Convert-ResourceHex($Value) { if($null -eq $Value){return '-'}; Assert-Resource ($Value -is [string] -and $Value.Length -gt 0 -and [Text.Encoding]::UTF8.GetByteCount($Value) -le 256 -and $Value -notmatch '[\x00-\x1f]') 'scope_filter_invalid'; -join ([Text.Encoding]::UTF8.GetBytes($Value)|ForEach-Object {$_.ToString('x2')}) }
function Assert-ResourceInput($InputObject) {
    Assert-Resource ($InputObject.schema -ceq 'audit-resource-input/v1') 'input_schema_invalid'
    foreach($n in @('start_seconds','end_seconds')) { Assert-Resource ($InputObject.execution_window.$n -is [long] -or $InputObject.execution_window.$n -is [int]) 'execution_window_invalid' }
    $p=$InputObject.execution_window
    Assert-Resource ($p.start_seconds -ge 0 -and $p.end_seconds -gt $p.start_seconds -and $p.end_seconds -le [uint32]::MaxValue -and $p.end_seconds-$p.start_seconds -le 300) 'execution_window_invalid'
    $s=$InputObject.scope
    Assert-Resource (@($s.PSObject.Properties.Name).Count -eq 8) 'scope_shape_invalid'
    foreach($n in @('window','state','user','catalog','database','resource_group','native_digest','statement_filter')) {Assert-Resource ($null -ne $s.PSObject.Properties[$n]) 'scope_shape_invalid'}
    Assert-Resource ($s.statement_filter -is [string] -and $s.statement_filter -cin @('all','exclude_doctor_and_non_query')) 'scope_statement_filter_invalid'
    foreach($n in @('start_seconds','end_seconds')) {Assert-Resource ($s.window.$n -is [long] -or $s.window.$n -is [int]) 'scope_window_invalid'}
    Assert-Resource ($s.window.start_seconds -ge 0 -and $s.window.end_seconds -le [uint32]::MaxValue -and $s.window.end_seconds-$s.window.start_seconds -eq 2592000) 'scope_not_thirty_days'
    $roles=@('coverage','primary','concurrency','family_top')
    Assert-Resource (@($InputObject.statement_hashes.PSObject.Properties).Count -eq 4) 'role_hash_invalid'
    foreach($role in $roles){Assert-Resource ($InputObject.statement_hashes.$role -is [string] -and $InputObject.statement_hashes.$role -cmatch '^[0-9a-f]{64}$') 'role_hash_invalid'}
    Assert-Resource (@($roles|ForEach-Object {$InputObject.statement_hashes.$_}|Select-Object -Unique).Count -eq 4) 'role_hash_duplicate'
    # Caller-provided exclusivity is NOT evidence. No request-level PASS path exists.
    Assert-Resource ($InputObject.attribution -ceq 'resource_observation_only') 'unsupported_attribution'
}
function Get-ResourceBuilderHashes($InputObject,[string]$Executable) {
    $s=$InputObject.scope
    $lines=@([string]$s.window.start_seconds,[string]$s.window.end_seconds,[string]$InputObject.primary_limit,[string]$InputObject.primary_sort,[string]$InputObject.family_top_sort)
    foreach($key in @('state','user','catalog','database','resource_group','native_digest')) {$lines+=Convert-ResourceHex $s.$key}
    $lines+=[string]$s.statement_filter
    $output=@(($lines -join "`n") | & $Executable 2>$null)
    Assert-Resource ($LASTEXITCODE -eq 0 -and $output.Count -eq 4) 'builder_failed'
    $hashes=@{}
    foreach($line in $output) {
        Assert-Resource ($line -cmatch '^(coverage|primary|concurrency|family_top)\t([0-9a-f]+)$') 'builder_shape_invalid'
        $role=$Matches[1];$hex=$Matches[2]
        Assert-Resource (-not $hashes.ContainsKey($role) -and $hex.Length % 2 -eq 0) 'builder_shape_invalid'
        $bytes=[byte[]]::new($hex.Length/2);for($i=0;$i -lt $bytes.Length;$i++){$bytes[$i]=[Convert]::ToByte($hex.Substring($i*2,2),16)}
        $sql=[Text.Encoding]::UTF8.GetString($bytes)
        $hashes[$role]=Get-ResourceHash $sql
        Assert-Resource ($hashes[$role] -ceq $InputObject.statement_hashes.$role) 'builder_hash_mismatch'
    }
    $sql=$null;$bytes=$null;$output=$null
    return $hashes
}
function Assert-ResourceSchema([object[]]$Lines) {
    Assert-Resource ($Lines.Count -gt 0 -and $Lines.Count -le 64) 'schema_row_cap'
    $fields=@{}
    foreach($line in $Lines){$cells=$line -split "`t";Assert-Resource ($cells.Count -ge 2 -and -not $fields.ContainsKey($cells[0])) 'schema_row_invalid';$fields[$cells[0]]=$cells[1].ToUpperInvariant()}
    foreach($name in @('queryId','timestamp','state','queryTime','scanBytes','scanRows','cpuCostNs','memCostBytes','returnRows','catalog','db','resourceGroup','digest','stmt','errorCode','pendingTimeMs','user')) {
        Assert-Resource ($fields.ContainsKey($name)) 'schema_column_missing'
        $expected=if($name -ceq 'timestamp'){'^DATETIME$'}elseif($name -cin @('queryTime','scanBytes','scanRows','cpuCostNs','memCostBytes','returnRows','pendingTimeMs')){'^BIGINT$'}else{'^VARCHAR(?:\([0-9]+\))?$'}
        Assert-Resource ($fields[$name] -cmatch $expected) 'schema_type_mismatch'
    }
}
function New-ResourceSql($InputObject) {
    Assert-ResourceInput $InputObject
    $p=$InputObject.execution_window
    $hashes=@('coverage','primary','concurrency','family_top')|ForEach-Object {"'$($InputObject.statement_hashes.$_)'"}
    'SELECT JSON_ARRAY(LOWER(`queryId`), CAST(UNIX_TIMESTAMP(`timestamp`) AS STRING), `user`, `state`, SHA2(`stmt`, 256), CAST(`queryTime` AS STRING), CAST(`scanRows` AS STRING), CAST(`cpuCostNs` AS STRING), CAST(`memCostBytes` AS STRING)) FROM starrocks_audit_db__.starrocks_audit_tbl__ WHERE `timestamp` >= FROM_UNIXTIME('+$p.start_seconds+') AND `timestamp` < FROM_UNIXTIME('+$p.end_seconds+') AND `user` = ''doctor_reader'' AND SHA2(`stmt`, 256) IN ('+($hashes -join ',')+') LIMIT 9;'
}
function Convert-ResourceRows([object[]]$Lines,$InputObject) {
    Assert-Resource ($Lines.Count -lt 9) 'row_cap_reached'
    $ids=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal);$roles=[Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal);$result=@()
    foreach($line in $Lines) {
        $c=@($line|ConvertFrom-Json)
        Assert-Resource ($c.Count -eq 9) 'row_shape_invalid'
        Assert-Resource ($c[0] -is [string] -and $c[0] -cmatch '^[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$' -and $ids.Add($c[0])) 'query_identity_invalid_or_duplicate'
        Assert-Resource ($c[1] -is [string] -and $c[1] -cmatch '^[0-9]{1,10}$') 'row_time_invalid'
        Assert-Resource ([long]$c[1] -ge $InputObject.execution_window.start_seconds -and [long]$c[1] -lt $InputObject.execution_window.end_seconds) 'row_outside_window'
        Assert-Resource ($c[2] -ceq 'doctor_reader' -and $c[3] -cin @('EOF','ERR')) 'row_identity_or_state_invalid'
        $role=@('coverage','primary','concurrency','family_top')|Where-Object {$InputObject.statement_hashes.$_ -ceq $c[4]}
        Assert-Resource (@($role).Count -eq 1 -and $roles.Add([string]$role)) 'role_missing_or_duplicate'
        $metrics=@();$names=@('queryTime','scanRows','cpuCostNs','memCostBytes');$units=@('ms','count','ns','byte')
        for($i=0;$i -lt 4;$i++) {
            $v=$c[$i+5];$state='exact'
            if($null -eq $v){$state='missing'} elseif($v -isnot [string] -or $v -cnotmatch '^-?(0|[1-9][0-9]*)$'){$state='invalid';$v=$null} elseif($v.StartsWith('-')){$state='invalid_negative'} elseif([Numerics.BigInteger]::Parse($v) -gt [long]::MaxValue){$state='overflow'}
            $metrics += [ordered]@{source_field=$names[$i];value=$v;unit=$units[$i];state=$state}
        }
        $result += [ordered]@{role=[string]$role;query_id_sha256=(Get-ResourceHash $c[0]);statement_sha256=$c[4];terminal_state=$c[3];metrics=$metrics}
    }
    [ordered]@{complete=($roles.Count -eq 4);rows=$result}
}
Assert-Resource (-not (Test-Path -LiteralPath $OutputPath)) 'output_exists'
$result=[ordered]@{schema='audit-resource-observation/v1';status='INCONCLUSIVE';attribution='resource_observation_only';request_attribution='NOT_VERIFIED';rows=@()}
$password=$null;$attempts=0;$clock=[Diagnostics.Stopwatch]::StartNew()
try {
    Assert-Resource ((Get-Item -LiteralPath $InputPath).Length -le 65536) 'input_too_large'
    $inputObject=Get-Content -LiteralPath $InputPath -Raw|ConvertFrom-Json
    Assert-ResourceInput $inputObject
    $null=Get-ResourceBuilderHashes $inputObject $BuilderExecutable
    $result.input_sha256=(Get-FileHash -LiteralPath $InputPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $result.builder_executable_sha256=(Get-FileHash -LiteralPath $BuilderExecutable -Algorithm SHA256).Hash.ToLowerInvariant()
    $result.execution_window=$inputObject.execution_window
    $result.expected_statement_hashes=$inputObject.statement_hashes
    Assert-FixtureContainerName $ContainerName
    $docker=Get-FixtureDockerExecutable $DockerExecutable
    $identity=Get-FixtureContainerIdentity $docker $ContainerName
    Assert-Resource ($null -ne $identity -and $identity.Image -ceq $script:FixtureImage -and $identity.Label -ceq $script:FixtureLabelValue) 'fixture_identity_invalid'
    $password=Get-FixturePassword -Secret (Get-FixtureSecret) -Role doctor_reader
    $settings='SET query_timeout=5; SET exec_mem_limit=67108864; '
    $zone='SELECT JSON_ARRAY(current_version(), @@time_zone, @@system_time_zone, CAST(TIME_TO_SEC(TIMEDIFF(NOW(), UTC_TIMESTAMP())) AS STRING), CAST(UNIX_TIMESTAMP(NOW()) AS STRING)); '
    $schemaRows=@(Invoke-FixtureMysql $docker $ContainerName doctor_reader $password ($settings+'SHOW COLUMNS FROM starrocks_audit_db__.starrocks_audit_tbl__;'))
    Assert-ResourceSchema $schemaRows
    $schemaRows=$null
    $sql=New-ResourceSql $inputObject
    $poll=[Diagnostics.Stopwatch]::StartNew()
    do {
        $attempts++
        $raw=@(Invoke-FixtureMysql $docker $ContainerName doctor_reader $password ($settings+$zone+$sql))
        Assert-Resource ($raw.Count -ge 1) 'source_empty'
        $z=@($raw[0]|ConvertFrom-Json)
        Assert-Resource ($z.Count -eq 5 -and $z[0] -ceq '3.5.14-23a56ec') 'fixture_version_invalid'
        $tz=$inputObject.audit_timestamp_time_zone
        Assert-Resource ($z[1] -ceq $tz.session_time_zone -and $z[2] -ceq $tz.system_time_zone -and [int]$z[3] -eq $tz.current_utc_offset_seconds) 'source_timezone_mismatch'
        Assert-Resource ([long]$z[4] -ge $inputObject.execution_window.end_seconds -and [long]$z[4]-$inputObject.execution_window.end_seconds -le 3600) 'execution_window_not_recent_closed'
        $facts=Convert-ResourceRows @($raw|Select-Object -Skip 1) $inputObject
        if($facts.complete){$result.rows=$facts.rows;$result.status='OBSERVED';break}
        if($attempts -ge 7 -or $poll.ElapsedMilliseconds -ge 25000){break}
        Start-Sleep -Milliseconds ([int][Math]::Min(5000,25000-$poll.ElapsedMilliseconds))
    } while($poll.ElapsedMilliseconds -lt 30000)
    if($result.status -ne 'OBSERVED'){$result.reason='four_roles_not_observed_before_poll_budget'}
} catch {$result.reason=if($_.Exception.Message -cmatch '^[a-z][a-z0-9_]+$'){$_.Exception.Message}else{'resource_capture_failed'}} finally {
    $password=$null;$sql=$null;$raw=$null;$clock.Stop()
    $result.resources=[ordered]@{elapsed_ms=$clock.ElapsedMilliseconds;poll_attempts=$attempts;poll_start_budget_ms=30000;query_timeout_seconds=5;exec_mem_limit_bytes=67108864;row_cap=9}
    $result.limitations=@('same_role_concurrent_identical_sql_attribution_unverified','caller_exclusivity_claims_not_accepted','audit_resources_not_server_rss','row_limit_not_scan_budget','no_fe_cancellation_proof','current_builder_binary_freshness_requires_external_build_receipt','poll_budget_excludes_native_process_hang_without_process_watchdog')
    $result|ConvertTo-Json -Depth 12|Set-Content -LiteralPath $OutputPath -Encoding utf8
}
if($result.status -ne 'OBSERVED'){exit 2}
