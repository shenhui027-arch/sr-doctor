$ErrorActionPreference = 'Stop'
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $PSScriptRoot 'connected-smoke.ps1'), [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw 'Harness syntax errors.' }
foreach ($name in @('Require', 'Assert-RuntimeSqlDisclosure')) {
    $fn = $ast.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $name }.GetNewClosure(), $true)
    . ([scriptblock]::Create($fn.Extent.Text))
}
$sql = 'SELECT COUNT(*) AS p11_runtime_count FROM information_schema.columns'
$secret = 'fixture-secret-only'
$passed = 0
function Check([string]$Name, [scriptblock]$Mutate, [bool]$Accept) {
    $response = '{"outcome":{"report":{"graph_view":{"nodes":[{"sql_location":{"state":"located","kind":"table","before":"","highlight":"information_schema.columns","after":"","source_refs":[]}}]}}}}' | ConvertFrom-Json
    & $Mutate $response
    $accepted = $true
    try { Assert-RuntimeSqlDisclosure $response $sql $secret } catch { $accepted = $false }
    if ($accepted -ne $Accept) { throw "Unexpected disclosure result: $Name" }
    $script:passed++
}
Check 'exact excerpt in graph view' {param($r)} $true
Check 'excerpt copied into a retired projection' {param($r) $r.outcome.report | Add-Member graph_reading ([pscustomobject]@{nodes=@([pscustomobject]@{sql_location=$r.outcome.report.graph_view.nodes[0].sql_location})})} $false
Check 'no location' {param($r) $r.outcome.report.graph_view.nodes[0].sql_location=$null} $true
Check 'ambiguous' {param($r) $r.outcome.report.graph_view.nodes[0].sql_location=[pscustomobject]@{state='ambiguous'}} $true
Check 'complete SQL' {param($r) $r | Add-Member leaked $sql} $false
Check 'alias' {param($r) $r | Add-Member leaked 'p11_runtime_count'} $false
Check 'credential' {param($r) $r | Add-Member leaked $secret} $false
Check 'table outside approved slot' {param($r) $r | Add-Member leaked 'information_schema.columns'} $false
Check 'context disclosure' {param($r) $r.outcome.report.graph_view.nodes[0].sql_location.before='SELECT COUNT(*)'} $false
Check 'oversized excerpt' {param($r) $r.outcome.report.graph_view.nodes[0].sql_location.highlight=('x'*513)} $false
Check 'wrong kind' {param($r) $r.outcome.report.graph_view.nodes[0].sql_location.kind='predicate'} $false
Check 'unknown state' {param($r) $r.outcome.report.graph_view.nodes[0].sql_location.state='guessed'} $false
Check 'hidden source in provenance' {param($r) $r.outcome.report.graph_view.nodes[0].sql_location.source_refs=@('information_schema.columns')} $false
Write-Output "PASS: $passed disclosure contract cases; harness AST syntax valid."
