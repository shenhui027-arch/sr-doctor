# 开发指南

本文说明如何在本机构建、运行和测试 SR Doctor。提交规则见 [CONTRIBUTING.md](../../CONTRIBUTING.md)，系统结构见 [架构概览](architecture.md)。

## 环境

| 工具 | 版本 | 用途 |
|---|---|---|
| Rust / Cargo | 1.88 以上（edition 2024） | 后端 |
| Node.js / npm | 22 以上 | 前端、契约检查、脚本测试 |
| PowerShell | 7 以上 | 本地 CI 与 E2E 脚本 |
| Docker | 可选 | Docker E2E、镜像构建 |
| Git Bash 或任意 POSIX bash | 可选 | 运行 `deploy.sh` 的测试 |

## 仓库结构

```text
backend/crates/
  doctor-api/          HTTP 服务（Axum）：信任边界、登录、路由、静态页面
  doctor-core/         内核：规范化 IR、Typed Metric Parser、执行结构、规则、对比、指纹、趋势
  doctor-contracts/    对外的线上契约（DTO），经 ts-rs 导出给前端
  doctor-connector/    StarRocks 只读连接：MySQL 文本协议、TLS、短期会话、有界操作
  doctor-history/      唯一的嵌入式 SQLite：模板、小时聚合、水位、告警与治理
  doctor-export/       确定性的脱敏报告导出
  rule-sets/           诊断规则集
  version-packs/       3.5.14 基线包，加 3.1 及以后各版本的差异（version-deltas.json）
frontend/              React + TypeScript + Vite，只渲染后端 DTO
contracts/             导出的契约、JSON Schema 与契约测试
fixtures/              官方 3.5.14 语料、合成样本与黄金期望
scripts/               本地 CI、E2E、校准与发布脚本
tests/                 跨端测试：浏览器 E2E、视觉比对、部署脚本
compose.yaml           服务器部署（docker compose up -d --build）
deploy.sh              部署与运维脚本，包装同一个 compose.yaml
deploy/local/          Windows 单机容器（回环、无历史）
docs/                  文档，见 docs/README.md
```

## 构建并运行

```bash
npm --prefix frontend ci
npm --prefix frontend run build          # 类型检查 + 构建 + 包体积检查
cargo run -p doctor-api                   # 默认读取 frontend/dist，监听 127.0.0.1:8000
```

只改界面时可以用 Vite 开发服务器：

```bash
npm --prefix frontend run dev
```

开发服务器和后端不同源，后端会拒绝跨源请求。需要真实 API 时，请用上面的构建产物配合 `doctor-api`。

## 本机目录

测试回执、E2E 产物、发布包和下载缓存都写在仓库之外，由两个环境变量决定：

| 变量 | 默认 | 用途 |
|---|---|---|
| `SR_DOCTOR_WORK_ROOT` | 系统临时目录下的 `sr-doctor` | 本地 CI 回执（`ci/`）、E2E 产物（`e2e/`）、发布包（`release/`）、AuditLoader 缓存（`cache/`） |
| `SR_DOCTOR_CONTAINER_ROOT` | `<SR_DOCTOR_WORK_ROOT>/containers` | Docker E2E 夹具的 FE 元数据与 BE 存储 |

临时目录可能被系统清理；长期开发时建议设成固定目录。

## 测试

| 范围 | 命令 |
|---|---|
| 后端 | `cargo test --workspace` |
| 后端静态检查 | `cargo fmt --check`、`cargo clippy --workspace --all-targets -- -D warnings` |
| 前端 | `npm --prefix frontend run typecheck`、`npm --prefix frontend test -- --run` |
| 契约 | `npm run contracts:test` |
| 脚本与部署 | `node --test tests/deploy/*.test.mjs` 等 `*.test.mjs` |

### 本地 CI

仓库不使用 GitHub Actions，合并门禁是本机脚本：

```powershell
pwsh -NoProfile -File scripts/ci/local-ci.ps1               # 全部步骤
pwsh -NoProfile -File scripts/ci/local-ci.ps1 -SkipInstall -FailFast
pwsh -NoProfile -File scripts/ci/local-ci.ps1 -IncludeDockerE2E
```

| 阶段 | 内容 |
|---|---|
| install | `npm ci`、`npm --prefix frontend ci`（`-SkipInstall` 跳过） |
| rust | fmt、clippy、`cargo test --workspace` |
| contracts | 契约生成检查、类型检查与契约测试 |
| frontend | 类型检查、Vitest、生产构建 |
| scripts | `scripts/`、`tests/` 下的 Node 测试与 PowerShell 静态检查 |
| e2e-docker | 默认跳过；`-IncludeDockerE2E` 开启 |

回执写到仓库外（默认 `<工作目录>/ci`，`-ReceiptRoot` 可改，不能指向仓库内）：`receipt.json` 是机读回执，含 HEAD、dirty 状态、每步命令、exit code、测试数与跳过原因；`summary.md` 可以直接贴进 PR。全部通过时退出码为 0。

### Docker E2E

`scripts/e2e/` 下的脚本会在固定 digest 的官方 `starrocks/*:3.5.14` 镜像上建立隔离夹具（含 AuditLoader），走通产品的真实路径：

| 脚本 | 覆盖 |
|---|---|
| `run-browser-e2e.ps1` | 只读连接的浏览器流程 |
| `run-connected-multi-be-e2e.ps1` | 1 FE + 3 BE 集群 |
| `run-a1-fingerprint-e2e.ps1` | 指纹与聚类 |
| `run-b-history-e2e.ps1`、`run-c-trends-e2e.ps1`、`run-d-governance-e2e.ps1` | 采集、趋势、告警与治理 |
| `run-tls-verify-full-e2e.ps1` | TLS verify-full |

资源有限时加 `-LowResource`，并逐个串行运行。夹具口令由脚本在进程内随机生成，不打印、不落盘。

### 视觉比对

`tests/visual/pixel-parity.mjs` 与 `layout-diff.mjs` 在同一视口下比对产品与设计稿：先固定视口与数据，再逐区比较像素与布局，偏差逐项记录；不为凑像素伪造后端不产出的内容。

## 修改契约

后端 DTO 定义在 `doctor-contracts`。改动后重新生成前端类型与契约回执：

```bash
cargo run -p doctor-contracts --bin export_contracts
cargo run -p doctor-core --bin refresh_contract_receipts
```

然后运行 `npm run contracts:test`。生成的文件需要一并提交。

## 新增 StarRocks 版本

规则只有一套：3.5.14 基线。其他版本 = 基线 + 版本差异，差异来自对官方源码的审计，不为每个版本复制规则。

1. **跑审计**：对官方仓库（blobless 克隆即可）运行

   ```bash
   node scripts/version-audit/audit-starrocks-versions.mjs --repo <starrocks.git> --out <目录>
   ```

   脚本按 [`watchlist.json`](../../scripts/version-audit/watchlist.json) 在每个发布标签上计算语义函数指纹、注册指标与能力是否存在、审计字段，并列出触及监控文件的提交。新版本线要先加进 `watchlist.json` 的 `lines`。
2. **读差异**：指纹与基线不同的函数，逐个读 diff，判断是语义变化还是重构；把语义变化写进 `backend/crates/version-packs/starrocks_3_5_14/version-deltas.json`（版本区间、影响、源码路径、官方提交）。没有官方语义的部分按基线契约自研，并标为 `own`。
3. **一致性测试**：`node --test scripts/version-audit/version-deltas.test.mjs` 保证差异数据与审计结果逐项一致；改了差异数据后同步 `deltas.rs` 里的 `DELTAS_HASH`。
4. **官方镜像冒烟**：

   ```powershell
   pwsh -NoProfile -File scripts/version-audit/run-version-smoke.ps1 -Release <x.y.z>
   ```

   在低资源容器里跑几条固定查询、采集 Profile，结束后删除容器、数据卷与镜像；再用 `check-version-smoke.mjs` 让产品分析这些 Profile。

审计范围之外的版本（包括已支持版本线中更新的补丁）不会自动套用最近的版本。契约细节见 [Fixture、VersionPack 与 RuleSet 治理](../10-fixture-versionpack-and-ruleset-contract.md)。

