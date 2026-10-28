$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$path = "$PSScriptRoot/../../dev/starrocks-3.5.14/run-fixture-workload.ps1"
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'Fixture parse failed.' }
$fn = $ast.Find({ param($n) $n -is [Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq 'Invoke-DigestFixtureWorkload' }, $true)
Invoke-Expression $fn.Extent.Text
$source = Get-Content $path -Raw
$start = $source.IndexOf("    `$f05State.stage = 'session_batch'")
$end = $source.IndexOf("    `$f05State.stage = 'session_batch_completed'", $start) + "    `$f05State.stage = 'session_batch_completed'".Length
$action = [scriptblock]::Create($source.Substring($start, $end - $start))
function Invoke-FixtureMysql { '12345678-1234-1234-1234-123456789abc' }
$docker = 'unused'; $ContainerName = 'unused'; $workloadPassword = 'unused'
$f05State = [ordered]@{ query_id = $null; stage = 'not_started' }
Invoke-DigestFixtureWorkload -Action $action -Evidence @{}
if ($f05State.query_id -ne '12345678-1234-1234-1234-123456789abc') { throw 'Identity did not cross Action scope.' }
if ($f05State.stage -ne 'session_batch_completed') { throw 'Stage did not cross Action scope.' }
if ($source.IndexOf('F05 identity missing before Audit observation.') -gt $source.IndexOf('while ([DateTime]')) { throw 'Identity guard is too late.' }
Write-Output 'PASS 3 scope/guard checks (pure stub; no SQL/native execution)'
