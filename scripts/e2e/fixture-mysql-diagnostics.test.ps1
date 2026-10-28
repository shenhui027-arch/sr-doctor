$ErrorActionPreference = 'Stop'
. "$PSScriptRoot/../../dev/starrocks-3.5.14/common.ps1"
$count = 0
foreach ($case in @(
    @{ input = 'ERROR 1193 (HY000) at line 2: secret SQL password'; code = 1193; line = 2 },
    @{ input = 'ERROR 1064 (42000): secret SQL'; code = 1064; line = $null },
    @{ input = 'unrecognized secret output'; code = $null; line = $null },
    @{ input = 'prefix ERROR 1193 (HY000) at line 2: secret'; code = $null; line = $null }
)) {
    $d = Get-FixtureMysqlDiagnostic -OutputLines @($case.input) -ExitCode 1
    if ($d.error_code -ne $case.code -or $d.line_number -ne $case.line) { throw 'Diagnostic field mismatch.' }
    if (($d | ConvertTo-Json) -match 'secret|password|prefix|unrecognized') { throw 'Native output leaked.' }
    $count++
}
Write-Output "PASS $count diagnostic cases (pure; no native invocation)"
