$ErrorActionPreference='Stop'
Set-StrictMode -Version Latest
$path=Join-Path $PSScriptRoot 'audit-resource-probe.ps1'
$tokens=$null;$errors=$null
$ast=[System.Management.Automation.Language.Parser]::ParseFile($path,[ref]$tokens,[ref]$errors)
if($errors.Count){throw ($errors|Out-String)}
$ast.FindAll({param($n)$n -is [System.Management.Automation.Language.FunctionDefinitionAst]},$false)|ForEach-Object {. ([scriptblock]::Create($_.Extent.Text))}
$count=0
function Check([bool]$Ok,[string]$Name){if(-not $Ok){throw $Name};$script:count++}
function Reject([scriptblock]$Action,[string]$Code){$caught=$false;try{& $Action}catch{$caught=$_.Exception.Message -ceq $Code};Check $caught $Code}
$x=[pscustomobject]@{schema='audit-resource-input/v1';execution_window=[pscustomobject]@{start_seconds=3000000;end_seconds=3000030};scope=[pscustomobject]@{window=[pscustomobject]@{start_seconds=408000;end_seconds=3000000};state=$null;user='workload_user';catalog=$null;database=$null;resource_group=$null;native_digest=$null;statement_filter='exclude_doctor_and_non_query'};statement_hashes=[pscustomobject]@{coverage=('a'*64);primary=('b'*64);concurrency=('c'*64);family_top=('d'*64)};attribution='resource_observation_only'}
$sql=New-ResourceSql $x
Check ($sql.Contains('>= FROM_UNIXTIME(3000000)') -and $sql.Contains('< FROM_UNIXTIME(3000030)')) 'frozen short half open window'
Check ($sql.EndsWith('LIMIT 9;') -and $sql.Contains('SHA2(`stmt`, 256) IN')) 'hash and cap'
Check ($sql.Contains("``user`` = 'doctor_reader'")) 'fixed read account'
Check (-not ($sql -match ',\s*`stmt`\s*[,)]')) 'no raw stmt projection'
$x.attribution='exclusive';Reject {New-ResourceSql $x} 'unsupported_attribution';$x.attribution='resource_observation_only'
$x.execution_window.end_seconds=3000301;Reject {New-ResourceSql $x} 'execution_window_invalid';$x.execution_window.end_seconds=3000030
$x.statement_hashes.primary=$x.statement_hashes.coverage;Reject {New-ResourceSql $x} 'role_hash_duplicate';$x.statement_hashes.primary='b'*64
function Row([int]$Id,[string]$Hash){ '["00000000-0000-0000-0000-'+$Id.ToString('000000000000')+'","3000001","doctor_reader","EOF","'+$Hash+'","2","3","4","5"]' }
$rows=@((Row 1 ('a'*64)),(Row 2 ('b'*64)),(Row 3 ('c'*64)),(Row 4 ('d'*64)))
$f=Convert-ResourceRows $rows $x
Check ($f.complete -and $f.rows.Count -eq 4) 'four roles'
Check ($f.rows[0].metrics[0].unit -ceq 'ms' -and $f.rows[0].metrics[0].value -ceq '2') 'raw query ms'
Check (($f|ConvertTo-Json -Depth 12) -notmatch '00000000-0000') 'hashed ids only'
Check (-not (Convert-ResourceRows @($rows[0]) $x).complete) 'partial is incomplete'
Reject {Convert-ResourceRows @($rows[0],$rows[0]) $x} 'query_identity_invalid_or_duplicate'
Reject {Convert-ResourceRows @($rows[0],(Row 2 ('a'*64))) $x} 'role_missing_or_duplicate'
Reject {Convert-ResourceRows @($rows[0].Replace('3000001','3000030')) $x} 'row_outside_window'
Reject {Convert-ResourceRows @($rows[0].Replace('doctor_reader','workload_user')) $x} 'row_identity_or_state_invalid'
Reject {Convert-ResourceRows (@($rows[0])*9) $x} 'row_cap_reached'
foreach($case in @(@('null','missing'),@('"-1"','invalid_negative'),@('"9223372036854775808"','overflow'),@('"oops"','invalid'),@('"0"','exact'))){$f=Convert-ResourceRows @($rows[0].Replace('"2"',$case[0])) $x;Check ($f.rows[0].metrics[0].state -ceq $case[1]) ('metric '+$case[1])}
$source=Get-Content $path -Raw
Check ($source.Contains("status='OBSERVED'") -and -not $source.Contains("status='PASS'")) 'no request PASS'
Check ($source.Contains('Get-ResourceBuilderHashes $inputObject $BuilderExecutable') -and $source.Contains('builder_hash_mismatch')) 'current builder verification'
Check ($source.Contains('source_timezone_mismatch') -and $source.Contains('fixture_identity_invalid') -and $source.Contains('3.5.14-23a56ec')) 'identity timezone version'
Check ($source.Contains('SET query_timeout=5; SET exec_mem_limit=67108864;') -and $source.Contains('$attempts -ge 7')) 'bounded retry and sessions'
Check (-not ($source -match '\b(INSERT INTO|CREATE TABLE|KILL QUERY|EXPLAIN ANALYZE)\b')) 'read only'
Check ($source.Contains('no_fe_cancellation_proof') -and $source.Contains('without_process_watchdog')) 'honest resource boundaries'
Check ((Convert-ResourceHex "x';--") -ceq '78273b2d2d') 'hex input to exporter'
Reject {Convert-ResourceHex "x`ny"} 'scope_filter_invalid'
$schema=@('queryId','timestamp','state','queryTime','scanBytes','scanRows','cpuCostNs','memCostBytes','returnRows','catalog','db','resourceGroup','digest','stmt','errorCode','pendingTimeMs','user')|ForEach-Object {$t=if($_ -ceq 'timestamp'){'DATETIME'}elseif($_ -cin @('queryTime','scanBytes','scanRows','cpuCostNs','memCostBytes','returnRows','pendingTimeMs')){'BIGINT'}else{'VARCHAR(64)'};"$_`t$t"}
Assert-ResourceSchema $schema;Check $true 'fixture schema'
Reject {Assert-ResourceSchema @($schema|Where-Object {$_ -notmatch '^user\t'})} 'schema_column_missing'
Reject {Assert-ResourceSchema @($schema|ForEach-Object {$_ -replace 'BIGINT','DOUBLE'})} 'schema_type_mismatch'
Write-Output "PASS audit-resource-probe static/pure checks=$count; SQL/Docker not executed"
