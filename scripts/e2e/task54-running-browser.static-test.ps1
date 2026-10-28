# Static-only regression: never launches a native process, SQL, service or browser.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$source = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'task54-running-browser.ps1') -Raw
$launcher = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'task31-development-browser.ps1') -Raw
$checks = 0
function Check([bool]$Condition, [string]$Name) {
    if (-not $Condition) { throw "STATIC_FAIL: $Name" }
    $script:checks++
}
foreach ($name in @('task54-running-browser.ps1', 'task31-development-browser.ps1')) {
    $tokens = $null; $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot $name), [ref]$tokens, [ref]$errors) | Out-Null
    Check ($errors.Count -eq 0) "AST $name"
}
Check ($source.IndexOf('$browserProcess = Start-Process') -lt $source.IndexOf('$readyDeadline =')) 'browser starts before ready wait'
Check ($source.IndexOf("'browser_exited_after_ready'") -lt $source.IndexOf('$queryProcess = Start-Process')) 'ready before only target execution'
Check ($source.Contains('AddSeconds(180)') -and $source.Contains('browserProcess.HasExited')) 'bounded readiness with process liveness'
Check ($source.Contains('SUM(SLEEP(1+event_id-event_id)) FROM runtime_corpus.sales WHERE event_id BETWEEN 1 AND 20')) '20-row fixture'
Check (([regex]::Matches($source, '\$queryExecutions\+\+')).Count -eq 1) 'one target submission'
Check ($source.Contains('query_timeout = 30') -and $source.Contains('WaitForExit(35000)')) 'query timeout and natural wait'
Check ($source.Contains('$attempt -lt 10') -and $source.Contains('SHOW PROFILELIST LIMIT 25')) 'bounded exact observation'
Check ($source.Contains("@{ schema = 'task54-running-handshake/v1'; query_id = `$queryId; observed_at_utc = `$identityFoundUtc }")) 'exact identity protocol fields'
Check ($source.Contains('[IO.File]::WriteAllText($identityTemp,') -and $source.Contains('[IO.File]::Move($identityTemp, $identityPath)')) 'atomic identity publication'
Check ($launcher.Contains('$hasQueryId -eq $hasHandshake')) 'mutually exclusive external identity inputs'
Check ($launcher.Contains('if ($RunningProfile -and $Project -ne ''connected-sql-docker'')')) 'mode guard'
Check ($launcher.Contains('[IO.Path]::IsPathFullyQualified($handshakePath)') -and $launcher.Contains('ReparsePoint')) 'launcher path guard'
Check ($source.Contains('[IO.Path]::GetDirectoryName($resolvedHandshake)') -and $source.Contains("'^task54-running-[0-9a-f]{32}$'")) 'cleanup confined to exact TEMP child'
Check ($source.Contains("'ready.json.tmp'") -and $source.Contains('[IO.Directory]::Delete($resolvedHandshake, $false)')) 'atomic-ready cleanup with nonrecursive empty-directory delete'
Check (-not $source.Contains('Remove-Item -Recurse')) 'no recursive filesystem deletion'
Check ($source.Contains('$liveProcess.StartTime.ToUniversalTime().Ticks -eq $browserStartedTicks') -and $source.Contains("'browser_cleanup_forced'")) 'forced process cleanup identity and FAIL'
Check ($source.Contains('temporary_identity_file_used = $identityFileUsed') -and $source.Contains('temporary_identity_file_removed = $identityFileRemoved')) 'honest temporary identity receipt'
Check ($source.Contains('browser_ready_utc = $browserReadyUtc') -and $source.Contains('identity_found_utc = $identityFoundUtc')) 'handshake timing receipt'
# Execute only the timestamp normalization slice: no wrapper side effects.
$timeStart = $source.IndexOf('    $parsedReady = [DateTimeOffset]::MinValue')
$timeEnd = $source.IndexOf("    Require (-not `$browserProcess.HasExited) 'browser_exited_after_ready'", $timeStart)
Check ($timeStart -ge 0 -and $timeEnd -gt $timeStart) 'timestamp slice found'
$normalizeTime = [scriptblock]::Create($source.Substring($timeStart, $timeEnd - $timeStart))
function Require([bool]$Condition, [string]$Name) { if (-not $Condition) { throw $Name } }
$expected = [DateTimeOffset]::Parse('2026-09-08T08:28:16.123Z', [Globalization.CultureInfo]::InvariantCulture)
$readyReceipt = '{"ready_at_utc":"2026-09-08T08:28:16.123Z"}' | ConvertFrom-Json
. $normalizeTime
Check ([DateTimeOffset]::Parse($browserReadyUtc).UtcTicks -eq $expected.UtcTicks) 'JSON UTC instant preserved'
$readyReceipt = [pscustomobject]@{ ready_at_utc = $expected.UtcDateTime }
. $normalizeTime
Check ([DateTimeOffset]::Parse($browserReadyUtc).UtcTicks -eq $expected.UtcTicks) 'typed UTC DateTime retains Kind'
$readyReceipt = [pscustomobject]@{ ready_at_utc = '2026-09-08T16:28:16.123+08:00' }
. $normalizeTime
Check ([DateTimeOffset]::Parse($browserReadyUtc).UtcTicks -eq $expected.UtcTicks) 'non-UTC offset normalizes to same instant'
# Deterministically reproduce the old cast/reparse under UTC+08, independent of host TZ.
$lostKind = [DateTime]::SpecifyKind($expected.UtcDateTime, [DateTimeKind]::Unspecified)
$utcPlusEight = [TimeZoneInfo]::CreateCustomTimeZone('StaticUTCPlusEight', [TimeSpan]::FromHours(8), 'UTC+08', 'UTC+08')
$legacyUtc = [TimeZoneInfo]::ConvertTimeToUtc($lostKind, $utcPlusEight)
Check (($expected.UtcDateTime - $legacyUtc).TotalHours -eq 8) 'lost Kind causes deterministic eight-hour regression'
Write-Output "TASK54_STATIC_PASS checks=$checks"
