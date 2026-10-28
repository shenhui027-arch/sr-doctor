# v0.1 研发路线与 Git 验收门禁

状态：v0.1 研发路线，已实现；之后的变化见 [更新日志](../CHANGELOG.md)
原则：每个可独立验收的任务或子任务只在自身 AC 通过后提交；一个独立验收单元对应一次提交，不以大阶段强行合并。

## 0. v0.1 闭环

```text
Offline JSON/Text Profile 导入或粘贴，或连接 StarRocks 3.5.14
  -> Queries 浏览 / 已有 Profile 拉取 / 用户显式受限 `EXPLAIN COSTS`
  -> 3.5.14 版本上下文与兼容报告
  -> Typed Metric + Partial IR
  -> 真实/可证推导/不可用执行图
  -> Doctor Findings + Evidence + Confidence + Limitations
  -> Recommendations（风险、预期方向、人工 validation steps）
  -> Operators / Raw / Provenance
  -> 用户外部复跑后导入 candidate Profile Compare
```

v0.1 不执行原始业务 SQL 或 `EXPLAIN ANALYZE`，不自动安装 AuditLoader、不修改 Advisor、不自动复跑或生成 candidate Profile。实时监控、自动聚类、自动优化、用户系统、数据库持久化、HA 和生产部署均不在 v0.1。

## 1. 阶段与提交单元

| Phase | 产物 | 通过后提交 |
|---|---|---|
| 0A 源码基线 | 固定 commit、代码索引、三角校验、证据账本 | 验收后一次提交 |
| 0B 产品/架构 | PRD、架构、ADR、阶段任务目录 | 验收后一次提交 |
| 0C 契约设计 | API/IR/Metric/Graph/Rule/Compare wire、grammar、fixture/pack/ruleset manifest 与 expected matrix | 验收后一次提交 |
| 1 设计稿 | Queries/Doctor/Recommendations/Compare 四个亮色关键画板、tokens/components | 验收后一次提交 |
| 2 Frontend | Queries/Doctor/Recommendations/Compare 高密度亮色 UI 与视觉/E2E 证据 | 按任务目录细分提交 |
| 3 Backend | Axum + core + 3.5.14 pack + rules + connected_readonly + Compare | 按任务目录细分提交 |
| 4 Candidate | Docker 3.5.14 + 已配置 AuditLoader 本地 E2E、构建、SBOM/许可证、发布回执 | 验收后一次提交 |

## 2. 阶段门禁

### Phase 0A

- 基线 commit 可复核；官方 tag 确认为 3.5.14。
- 关键源码建立代码索引，已知缺陷有调用链和影响面。
- 问题先查官方、再查独立实现；同源 fork 不计票。
- 外部都无可靠解法时，有明确 `own` 契约和 claim boundary。

### Phase 0B

- PRD 中的每个 v0.1 功能都有 FR/AC ID；原型不得展示未入 PRD 的可用能力。
- React/TypeScript/Vite、Rust/Axum、无数据库和本机边界无冲突。
- VersionPack 支持未来版本，但未预建插件平台/微服务。
- 任务目录每项有输入、输出、验收、命令、不做事项和提交说明。

### Phase 0C

- 文档冻结 schema/wire、失败语义、grammar、manifest 与 expected matrix；实际 schema/types/fixtures/tests 由 Task 01 独立提交，当前不得标记已通过。
- Task 01 可执行 schema/契约测试必须覆盖 missing/0/invalid/partial/overflow/unavailable。
- `500ms/us/ns`、组合时长、μ/µ strict reject、前后缀垃圾和数量 `M` 不发生单位混淆。
- 显式/唯一推导/不可用拓扑有 golden fixtures；不存在线性假树。
- 规则的 applicability/evidence/confidence/limitations/not_applicable 可序列化并可回放。

### Phase 1

- 在统一的亮色系统中设计四个 1440x1024 状态：Queries（连接状态、浏览与 Profile 拉取）、Doctor（版本/兼容、Findings、Execution、Operators、Source/Provenance）、Recommendations（证据化建议和人工 validation steps）、Compare（两份外部复跑后 Profile 与四类 delta）；不得以 tab 标签代替页面。
- 内置样例显式为 Example/Mock analysis；连接状态只在实际 `connected_readonly` 成功时显示，无未标注 mock/伪 live 数据、无总分。
- 设计稿与前端在同一视口做像素比对，偏差逐项留档；不为凑像素伪造后端不产出的内容。

### Phase 2/3/4

- 前端不解析 metric/拓扑/规则；后端不引入无必要数据库/服务。
- 合理的核心测试：Parser/IR/Graph/Rule/API trust boundary、connected_readonly Query/Profile/受限 `EXPLAIN COSTS` 边界、analysis report、关键 UI 流程和 Docker 3.5.14 + 已配置 AuditLoader E2E；不追求无意义的覆盖率数字。
- Candidate 回执包含 Git、锁文件、命令/exit code、测试数、fixture hash、支持版本、SBOM/许可证、已知问题和回滚。
- 没有真实脱敏 3.5.14 Profile 校准时，真实集群正确性保持 `INCONCLUSIVE`。

## 3. 统一证据回执

```text
task id / acceptance ids
branch / commit / author / message / dirty state
toolchain and dependency locks
commands / exit codes / test counts
fixture source / redaction / hashes
schema / registry / rule / version-pack versions
visual viewport / reference / diff score
known failures / NOT_READY / INCONCLUSIVE boundaries
```

