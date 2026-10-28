#Requires -Version 7.2
<#
.SYNOPSIS
本仓库的本地 CI 入口。仓库不使用 GitHub Actions，门禁在本机运行。

.DESCRIPTION
按阶段执行仓库已有的校验命令，记录每条命令的 exit code、耗时与测试数，
并把机读回执写到仓库外的 <ReceiptRoot>\<UTC 时间戳>-<HEAD sha>\。
本脚本只编排已有命令，不修改测试、不放宽门禁。需要 Docker 的 E2E 默认跳过，
只有显式传入 -IncludeDockerE2E 才执行。

.EXAMPLE
pwsh -NoProfile -File scripts/ci/local-ci.ps1

.EXAMPLE
pwsh -NoProfile -File scripts/ci/local-ci.ps1 -SkipInstall -FailFast
#>
[CmdletBinding()]
param(
    # 回执根目录；必须位于仓库之外。
    [string]$ReceiptRoot = (Join-Path ($env:SR_DOCTOR_WORK_ROOT ?? (Join-Path ([IO.Path]::GetTempPath()) 'sr-doctor')) 'ci'),
    # 跳过 npm ci，复用已有 node_modules（回执中记录为 SKIPPED）。
    [switch]$SkipInstall,
    # 显式开启 Docker 隔离 E2E（StarRocks 3.5.14 固定 digest + AuditLoader + 浏览器）。
    [switch]$IncludeDockerE2E,
    [string]$DockerExecutable = $env:DOCKER_EXE,
    # 首个失败后跳过其余步骤（回执中记录为 SKIPPED）。
    [switch]$FailFast
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path.TrimEnd('\')
$receiptRootFull = [IO.Path]::GetFullPath($ReceiptRoot).TrimEnd('\')
if ($receiptRootFull -eq $repoRoot -or $receiptRootFull.StartsWith($repoRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw "ReceiptRoot must be outside the repository: $receiptRootFull"
}

$utf8 = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = $utf8
$OutputEncoding = $utf8
$colorEnvNames = @('NO_COLOR', 'FORCE_COLOR', 'CARGO_TERM_COLOR')
$previousColorEnv = @{}
foreach ($name in $colorEnvNames) { $previousColorEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$env:NO_COLOR = '1'
$env:FORCE_COLOR = '0'
$env:CARGO_TERM_COLOR = 'never'

$npm = if ($IsWindows) { 'npm.cmd' } else { 'npm' }
$pwsh = (Get-Process -Id $PID).Path

function Get-GitText([string[]]$GitArgs) {
    $output = & git -C $repoRoot @GitArgs 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    return (@($output) -join "`n").Trim()
}

function Get-ToolVersion([string]$File, [string[]]$VersionArgs) {
    if ($null -eq (Get-Command $File -ErrorAction SilentlyContinue)) { return $null }
    try { return (@(& $File @VersionArgs 2>&1) -join ' ').Trim() } catch { return $null }
}

# ---- 测试数解析：只读取各工具自身的汇总行，解析不到时记为 null，不补 0 ----

function Get-CargoTestCount([string]$Text) {
    $found = [regex]::Matches($Text, 'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out')
    if ($found.Count -eq 0) { return $null }
    $passed = 0; $failed = 0; $ignored = 0
    foreach ($m in $found) { $passed += [int]$m.Groups[1].Value; $failed += [int]$m.Groups[2].Value; $ignored += [int]$m.Groups[3].Value }
    return [ordered]@{ kind = 'cargo_tests'; suites = $found.Count; passed = $passed; failed = $failed; ignored = $ignored; total = $passed + $failed + $ignored }
}

function Get-VitestCount([string]$Text) {
    $line = [regex]::Matches($Text, '(?m)^\s*Tests\s+(.+)$') | Select-Object -Last 1
    if ($null -eq $line) { return $null }
    $summary = $line.Groups[1].Value
    $read = { param($label) $m = [regex]::Match($summary, "(\d+) $label"); if ($m.Success) { [int]$m.Groups[1].Value } else { 0 } }
    $total = [regex]::Match($summary, '\((\d+)\)')
    $files = [regex]::Matches($Text, '(?m)^\s*Test Files\s+.*\((\d+)\)') | Select-Object -Last 1
    return [ordered]@{
        kind = 'vitest_tests'
        files = if ($null -ne $files) { [int]$files.Groups[1].Value } else { $null }
        passed = & $read 'passed'; failed = & $read 'failed'; skipped = & $read 'skipped'; todo = & $read 'todo'
        total = if ($total.Success) { [int]$total.Groups[1].Value } else { $null }
    }
}

function Get-NodeTestCount([string]$Text) {
    $values = @{}
    foreach ($m in [regex]::Matches($Text, '(?m)^(?:\S+\s+)?(tests|suites|pass|fail|cancelled|skipped|todo) (\d+)\s*$')) {
        $values[$m.Groups[1].Value] = [int]$m.Groups[2].Value
    }
    if (-not $values.ContainsKey('tests')) { return $null }
    $read = { param($key) if ($values.ContainsKey($key)) { $values[$key] } else { $null } }
    return [ordered]@{
        kind = 'node_tests'; passed = & $read 'pass'; failed = & $read 'fail'; cancelled = & $read 'cancelled'
        skipped = & $read 'skipped'; todo = & $read 'todo'; total = $values['tests']
    }
}

function Get-ContractCaseCount([string]$Text) {
    $m = [regex]::Match($Text, 'contracts:test passed \(([^)]*)\)')
    if (-not $m.Success) { return $null }
    $breakdown = [ordered]@{}
    $total = 0
    foreach ($part in [regex]::Matches($m.Groups[1].Value, '(\d+) ([^,]+)')) {
        $breakdown[$part.Groups[2].Value.Trim()] = [int]$part.Groups[1].Value
        $total += [int]$part.Groups[1].Value
    }
    return [ordered]@{ kind = 'contract_cases'; passed = $total; failed = 0; total = $total; breakdown = $breakdown }
}

function Get-StaticCheckCount([string]$Text) {
    $m = [regex]::Match($Text, '(?m)(?:(?:checks|assertions)=(\d+)|^PASS (\d+) )')
    if (-not $m.Success) { return $null }
    $count = if ($m.Groups[1].Success) { [int]$m.Groups[1].Value } else { [int]$m.Groups[2].Value }
    return [ordered]@{ kind = 'static_checks'; passed = $count; failed = 0; total = $count }
}

# ---- 步骤定义：命令全部来自仓库已有 package.json / docs / scripts ----

function New-Step([string]$Id, [string]$Phase, [string]$File, [string[]]$Arguments, [string]$Parser = $null, [string[]]$DependsOn = @()) {
    return [ordered]@{ id = $Id; phase = $Phase; file = $File; arguments = $Arguments; parser = $Parser; depends_on = $DependsOn }
}

$nodeTestFiles = @(
    foreach ($dir in @('scripts', 'tests')) {
        Get-ChildItem -LiteralPath (Join-Path $repoRoot $dir) -Recurse -File -Filter '*.test.mjs' |
            Where-Object { $_.FullName -notmatch '[\\/]node_modules[\\/]' } |
            ForEach-Object { [IO.Path]::GetRelativePath($repoRoot, $_.FullName).Replace('\', '/') }
    }
) | Sort-Object
$staticPsTests = @(
    Get-ChildItem -LiteralPath (Join-Path $repoRoot 'scripts\e2e') -File |
        Where-Object { $_.Name -like '*.static-test.ps1' -or $_.Name -like '*.test.ps1' } |
        Sort-Object Name
)

$steps = @(
    New-Step 'install-root' 'install' $npm @('ci')
    New-Step 'install-frontend' 'install' $npm @('--prefix', 'frontend', 'ci')
    New-Step 'rust-fmt' 'rust' 'cargo' @('fmt', '--all', '--', '--check')
    New-Step 'rust-clippy' 'rust' 'cargo' @('clippy', '--locked', '--workspace', '--all-targets', '--', '-D', 'warnings')
    New-Step 'rust-test' 'rust' 'cargo' @('test', '--locked', '--workspace') 'cargo'
    New-Step 'contracts-test' 'contracts' $npm @('run', 'contracts:test') 'contracts' @('install-root')
    New-Step 'frontend-typecheck' 'frontend' $npm @('--prefix', 'frontend', 'run', 'typecheck') $null @('install-frontend')
    New-Step 'frontend-test' 'frontend' $npm @('--prefix', 'frontend', 'test', '--', '--run') 'vitest' @('install-frontend')
    New-Step 'frontend-build' 'frontend' $npm @('--prefix', 'frontend', 'run', 'build') $null @('install-frontend')
    New-Step 'node-tests' 'scripts' 'node' (@('--test', '--test-reporter=spec') + $nodeTestFiles) 'node' @('install-root')
    foreach ($test in $staticPsTests) {
        New-Step ('ps-' + [IO.Path]::GetFileNameWithoutExtension($test.Name)) 'scripts' $pwsh @('-NoProfile', '-NonInteractive', '-File', [IO.Path]::GetRelativePath($repoRoot, $test.FullName)) 'static'
    }
    New-Step 'e2e-build-api' 'e2e-docker' 'cargo' @('build', '--locked', '-p', 'doctor-api')
    New-Step 'e2e-connected-readonly-docker' 'e2e-docker' $pwsh (@('-NoProfile', '-NonInteractive', '-File', 'scripts/e2e/run-browser-e2e.ps1', '-Project', 'connected-readonly-docker', '-RequireOwnedApi') + $(if ($DockerExecutable) { @('-DockerExecutable', $DockerExecutable) } else { @() })) $null @('e2e-build-api', 'frontend-build')
)

$dockerSkipReason = '默认跳过：需要 Docker Desktop、官方 starrocks/allin1-ubuntu:3.5.14 固定 digest 镜像、AuditLoader 与本机浏览器；传入 -IncludeDockerE2E 显式开启。本回执不构成 Docker E2E 证据。'

# ---- 运行 ----

$head = Get-GitText @('rev-parse', 'HEAD')
if ([string]::IsNullOrWhiteSpace($head)) { throw 'Unable to resolve git HEAD.' }
$porcelain = @(& git -C $repoRoot status --porcelain=v1 2>$null | Where-Object { $_ })
$startedUtc = [DateTime]::UtcNow
$runName = '{0}-{1}' -f $startedUtc.ToString('yyyyMMddTHHmmssZ'), $head.Substring(0, 12)
$runDir = Join-Path $receiptRootFull $runName
New-Item -ItemType Directory -Force -Path $runDir | Out-Null

$receipt = [ordered]@{
    schema_version = 'sr-doctor.local-ci/v1'
    status = 'FAIL'
    started_utc = $startedUtc.ToString('o')
    finished_utc = $null
    duration_ms = $null
    runner = [ordered]@{
        script = 'scripts/ci/local-ci.ps1'
        host = [Environment]::MachineName
        os = [Runtime.InteropServices.RuntimeInformation]::OSDescription
        github_actions = 'disabled_by_repository_policy'
        options = [ordered]@{ skip_install = [bool]$SkipInstall; include_docker_e2e = [bool]$IncludeDockerE2E; fail_fast = [bool]$FailFast }
    }
    git = [ordered]@{
        head = $head
        branch = Get-GitText @('rev-parse', '--abbrev-ref', 'HEAD')
        dirty = $porcelain.Count -gt 0
        dirty_entries = $porcelain
    }
    tools = [ordered]@{
        cargo = Get-ToolVersion 'cargo' @('--version')
        rustc = Get-ToolVersion 'rustc' @('--version')
        node = Get-ToolVersion 'node' @('--version')
        npm = Get-ToolVersion $npm @('--version')
        pwsh = $PSVersionTable.PSVersion.ToString()
        docker = if ($IncludeDockerE2E) { Get-ToolVersion $(if ($DockerExecutable) { $DockerExecutable } else { 'docker' }) @('--version') } else { $null }
    }
    totals = $null
    steps = @()
    limitations = @()
}
if ($receipt.git.dirty) { $receipt.limitations += 'Working tree was dirty; results describe HEAD plus uncommitted changes, not HEAD alone.' }
if (-not $IncludeDockerE2E) { $receipt.limitations += 'Docker E2E skipped by default; no Docker/AuditLoader/browser evidence in this receipt.' }

$results = [ordered]@{}
$haltReason = $null
try {
    foreach ($step in $steps) {
        $record = [ordered]@{
            id = $step.id; phase = $step.phase
            command = (@($step.file) + $step.arguments) -join ' '
            status = 'SKIPPED'; exit_code = $null; duration_ms = $null; parser = $step.parser; tests = $null
            skip_reason = $null; log = $null
        }
        $failedDependency = @($step.depends_on | Where-Object { $results.Contains($_) -and $results[$_].status -eq 'FAIL' }) | Select-Object -First 1

        if ($step.phase -eq 'install' -and $SkipInstall) {
            $record.skip_reason = 'SkipInstall 指定：复用已有 node_modules。'
        }
        elseif ($step.phase -eq 'e2e-docker' -and -not $IncludeDockerE2E) {
            $record.skip_reason = $dockerSkipReason
        }
        elseif ($null -ne $haltReason) {
            $record.skip_reason = $haltReason
        }
        elseif ($null -ne $failedDependency) {
            $record.skip_reason = "依赖步骤 $failedDependency 失败。"
        }
        elseif ($null -eq (Get-Command $step.file -ErrorAction SilentlyContinue)) {
            $record.status = 'FAIL'
            $record.skip_reason = "可执行文件不存在：$($step.file)"
        }
        else {
            $logPath = Join-Path $runDir ($step.id + '.log')
            $record.log = [IO.Path]::GetFileName($logPath)
            # 无输出的命令（如 cargo fmt --check 通过时）也留下空日志，保证回执引用的文件存在。
            [IO.File]::WriteAllText($logPath, '', $utf8)
            Write-Host ''
            Write-Host "==> [$($step.phase)] $($step.id): $($record.command)"
            $stopwatch = [Diagnostics.Stopwatch]::StartNew()
            $previousEap = $ErrorActionPreference
            $global:LASTEXITCODE = $null
            Push-Location $repoRoot
            try {
                # 原生命令的 stderr 是正常输出（cargo 进度等），这里不让它触发终止错误。
                $ErrorActionPreference = 'Continue'
                $fileName = $step.file
                $arguments = $step.arguments
                & $fileName @arguments 2>&1 | ForEach-Object { $_.ToString() } | Tee-Object -FilePath $logPath | Out-Host
                $record.exit_code = $LASTEXITCODE
            }
            catch {
                Add-Content -LiteralPath $logPath -Value ("LOCAL_CI_RUNNER_ERROR: " + $_.Exception.Message)
            }
            finally {
                $ErrorActionPreference = $previousEap
                Pop-Location
                $stopwatch.Stop()
            }
            $record.duration_ms = $stopwatch.ElapsedMilliseconds
            $record.status = if ($record.exit_code -eq 0) { 'PASS' } else { 'FAIL' }
            if ($step.parser) {
                $text = if (Test-Path -LiteralPath $logPath) { Get-Content -LiteralPath $logPath -Raw } else { '' }
                $text = [regex]::Replace([string]$text, '\x1b\[[0-9;]*[A-Za-z]', '')
                $record.tests = switch ($step.parser) {
                    'cargo' { Get-CargoTestCount $text }
                    'vitest' { Get-VitestCount $text }
                    'node' { Get-NodeTestCount $text }
                    'contracts' { Get-ContractCaseCount $text }
                    'static' { Get-StaticCheckCount $text }
                }
            }
            Write-Host ("<== {0} {1} exit={2} {3:N1}s" -f $record.status, $step.id, $record.exit_code, ($record.duration_ms / 1000))
        }

        if ($record.status -eq 'FAIL' -and $FailFast -and $null -eq $haltReason) {
            $haltReason = "FailFast：步骤 $($step.id) 失败后停止。"
        }
        $results[$step.id] = $record
        $receipt.steps += $record
    }
}
finally {
    foreach ($name in $colorEnvNames) { [Environment]::SetEnvironmentVariable($name, $previousColorEnv[$name], 'Process') }

    $finishedUtc = [DateTime]::UtcNow
    $executed = @($receipt.steps | Where-Object { $_.status -ne 'SKIPPED' })
    $failed = @($receipt.steps | Where-Object { $_.status -eq 'FAIL' })
    $counted = @($receipt.steps | Where-Object { $null -ne $_.tests })
    $receipt.totals = [ordered]@{
        steps = $receipt.steps.Count
        passed = @($receipt.steps | Where-Object { $_.status -eq 'PASS' }).Count
        failed = $failed.Count
        skipped = @($receipt.steps | Where-Object { $_.status -eq 'SKIPPED' }).Count
        tests_passed = [long](@($counted | ForEach-Object { [int]$_.tests.passed }) | Measure-Object -Sum).Sum
        tests_failed = [long](@($counted | ForEach-Object { if ($null -ne $_.tests.failed) { [int]$_.tests.failed } else { 0 } }) | Measure-Object -Sum).Sum
        # 运行了但汇总行解析失败的测试步骤：测试数未知，不按 0 计。
        unparsed_test_steps = @($receipt.steps | Where-Object { $_.status -ne 'SKIPPED' -and $_.parser -and $null -eq $_.tests } | ForEach-Object { $_.id })
    }
    $receipt.status = if ($receipt.steps.Count -lt $steps.Count) { 'INCOMPLETE' } elseif ($failed.Count -gt 0) { 'FAIL' } elseif ($executed.Count -gt 0) { 'PASS' } else { 'INCONCLUSIVE' }
    $receipt.finished_utc = $finishedUtc.ToString('o')
    $receipt.duration_ms = [long]($finishedUtc - $startedUtc).TotalMilliseconds

    $receiptPath = Join-Path $runDir 'receipt.json'
    [IO.File]::WriteAllText($receiptPath, ($receipt | ConvertTo-Json -Depth 8), $utf8)

    $lines = @(
        "# Local CI $($receipt.status)",
        '',
        "- HEAD: ``$head`` ($($receipt.git.branch)), dirty: $($receipt.git.dirty)",
        "- Started (UTC): $($receipt.started_utc), duration: $([math]::Round($receipt.duration_ms / 1000, 1))s",
        "- Steps: $($receipt.totals.passed) PASS / $($receipt.totals.failed) FAIL / $($receipt.totals.skipped) SKIPPED; tests passed: $($receipt.totals.tests_passed), failed: $($receipt.totals.tests_failed)",
        '',
        '| Step | Status | Exit | Seconds | Tests | Note |',
        '|---|---|---|---|---|---|'
    )
    foreach ($s in $receipt.steps) {
        $tests = if ($null -ne $s.tests) { "$($s.tests.passed)/$($s.tests.total) ($($s.tests.kind))" } else { '' }
        $seconds = if ($null -ne $s.duration_ms) { [math]::Round($s.duration_ms / 1000, 1) } else { '' }
        $lines += "| $($s.id) | $($s.status) | $($s.exit_code) | $seconds | $tests | $($s.skip_reason) |"
    }
    if ($receipt.limitations.Count -gt 0) { $lines += ''; $lines += '## Limitations'; $lines += ($receipt.limitations | ForEach-Object { "- $_" }) }
    [IO.File]::WriteAllText((Join-Path $runDir 'summary.md'), ($lines -join "`n") + "`n", $utf8)

    Write-Host ''
    Write-Host "LOCAL_CI_STATUS=$($receipt.status)"
    Write-Host "LOCAL_CI_RECEIPT=$receiptPath"
}

if ($receipt.status -ne 'PASS') { exit 1 }
exit 0
