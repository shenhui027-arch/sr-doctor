# v0.1 本机 HTTP API 与失败语义契约

状态：Approved Phase 0C contract baseline / 实现：已实现，随 v0.7.0 冻结

## 1. 边界

- 监听：原生进程默认且唯一内置 bind 为 `127.0.0.1`。显式 `loopback-published` 容器模式在容器内监听 wildcard，但受控 Compose 只发布到 Windows `127.0.0.1:<public-port>`；不提供 LAN/public bind 快捷开关。
- API：同步、无数据库、无分析 ID 查询、无后台任务/取消端点；只为 StarRocks credential pool 保存有 TTL 的短期 connection session。
- Offline 路径：`health`、`capabilities`、`analyses`、`comparisons`、`exports`。
- Connected 路径：`connections`、有界 `queries/search`、`profiles/fetch`、两份服务端拉取 Profile 的 `profiles/compare`、`advisor-guides`、普通 `explain`。
- 不提供任意 SQL 控制台、DDL/DML/INSERT、自动 SQL/参数修改、Plan Advisor mutation、Monitor、分享 URL 或遥测。

响应中的 `request_id` 用于本进程错误关联，绝不进入 canonical AnalysisSnapshotReport/Comparison hash，也不表示服务端保存结果。

## 2. 信任边界处理顺序

所有请求按以下顺序拒绝，不能等解析 Profile 后才检查：

1. 原生模式 TCP peer 必须是 loopback；显式容器模式只接受 loopback、private 或 link-local peer；
2. `Host` 必须精确命中 `127.0.0.1:<public-port>`、`localhost:<public-port>` 或显式开发代理 allowlist；
3. 浏览器 `Origin` 必须精确命中 same-origin 或显式 Vite dev origin，`null`/未知 Origin 拒绝；
4. 浏览器 POST 必须携带 `X-Doctor-CSRF`，值等于本进程随机 nonce；
5. Content-Type 和总 bytes 预算；
6. per-input bytes/line/depth/node/metric/time 预算；
7. adapter/normalization/analysis；
8. Connected 请求再检查 connection ID/TTL、只读 action allowlist、statement class 和远端 timeout/budget。

`GET /capabilities` 返回 nonce，并设置 `Cache-Control: no-store`；Candidate 不返回宽 CORS。恶意网站即使能发 simple request，也不能读取 nonce；multipart POST 因自定义 header 触发 preflight，未知 Origin 不获允许。

开发期 Vite 跨 origin 只对显式 allowlist origin 返回精确 `Access-Control-Allow-Origin` 与 `Vary: Origin`，不允许 credentials。`OPTIONS` 只为已登记写端点提供协议级 preflight；仅允许端点所需 method、`Content-Type`、`X-Doctor-CSRF`，未知 Origin/Path/Header 拒绝。Candidate 同 origin 静态资源模式不需要 CORS/preflight。

无 Origin 的 CLI 请求仍须 peer、Host、Content-Type 和预算。原生模式为兼容本机 CLI，可不带 CSRF nonce；显式容器模式的所有写请求均须 nonce。nonce 不等于用户认证；容器部署只支持同一 Windows 主机的 loopback URL，未来网络部署必须另开 ADR。

## 3. 初始资源预算

这些是 v0.1 本机 Candidate 的可测试默认值，不是生产容量承诺：

| budget | value |
|---|---:|
| single input bytes | 16 MiB |
| comparison aggregate bytes（离线 request 或两份 connected fetched Profile） | 34 MiB |
| text lines per input | 250,000 |
| bytes per text line | 256 KiB |
| JSON nesting depth | 128 |
| graph/operator nodes | 20,000 |
| normalized metrics | 250,000 |
| retained diagnostics | 10,000 |
| single analysis wall time | 10 s |
| comparison wall time | 20 s |
| concurrent analysis/comparison requests | 2；不排队，超出 429 |
| connection sessions | 3；idle TTL 15 min |
| DNS/connect/TLS handshake | 各 5 s |
| query browser | default 100 / max 500 rows；max 30-day window；5 s |
| profile fetch / advisor / explain | 10 s |

Task 01 用边界/边界+1 generated fixtures 验证；如实测代表性 3.5.14 Profile 需要调整，必须记录机器、fixture hash、前后值和理由，不静默放宽。

## 4. GET /api/v1/health

200：

```json
{"status":"ok"}
```

不返回 Profile、运行中请求、环境变量、文件路径或完整 build metadata。

## 5. GET /api/v1/capabilities

200：

```text
CapabilitiesResponse
├─ api_version
├─ schema_version
├─ supported_inputs       json_profile | text_profile
├─ connected_mode         connected_readonly
├─ connected_actions      browse | fetch_profile | compare_profiles | read_advisor | explain_plan
├─ version_packs[]        only starrocks-3.5.14 in v0.1
├─ rule_sets[]
├─ limits                 section 3 values
├─ csrf
│  ├─ nonce
│  └─ required_when_origin_present = true
└─ build
   ├─ git_commit
   └─ candidate_label
```

`version_packs` 表示可用 pack，不表示任何尚未导入的 Profile 已 compatible/supported。

## 6. POST /api/v1/analyses

允许两种 Content-Type：

### application/json

```text
AnalysisRequest
├─ source_kind           auto | json_profile | text_profile
├─ content               string
├─ filename              optional sanitized display name
└─ version_hint          auto | 3.5.14
```

### multipart/form-data

字段：`profile`（单一 file part）、`source_kind`、可选 `filename`、`version_hint`。拒绝重复 file part、嵌套 multipart 和额外未知大字段。直接 `text/plain` POST 不接受。

200：

```text
AnalysisResponse
├─ meta.request_id
└─ outcome               AnalysisOutcome from IR contract
```

只要能构造 Partial IR，即使 version unknown/incompatible、graph unavailable、部分 metric invalid，也返回 200；唯一公开状态位于 `outcome.report.analysis_state=partial`，诊断位于 `outcome.report.diagnostics`。只有无法构造任何可信输入事实时返回 ErrorEnvelope。

## 7. POST /api/v1/comparisons

JSON 请求包含 `baseline` 与 `candidate` 两个 AnalysisRequest；multipart 使用 `baseline_profile` / `candidate_profile` 及各自 source_kind/version_hint。两侧互不继承版本或 adapter 状态。

```text
ComparisonResponse
├─ meta.request_id
└─ comparison            ComparisonSnapshot（内含 baseline/candidate AnalysisAttempt）
```

外层请求合法时，即使一侧 adapter/parse 失败，也返回 200：失败侧有脱敏 ErrorSummary，comparison.state=`not_comparable` 并给 reasons；成功侧状态只从 `outcome.report.analysis_state` 推导，失败侧只从 ErrorSummary 推导，不另存可矛盾的 attempt state。总 HTTP body 超限、安全校验失败或外层 JSON/multipart 无法解析仍为请求级 4xx。

两侧 complete/partial 时 Comparison 包含 compatibility、structure、metrics、findings 四类 delta。Comparison public schema 固定为 `1.1`，独立于 Capabilities 中的 Analysis schema `1.0`；其 receipt 由服务端本地 comparator 组装为唯一 `doctor-comparator-v0.1` semantic receipt，JSON/multipart 请求不得传入 receipt 或 hash。baseline exact 0 不是错误，percentage state=`undefined_baseline_zero`；baseline bounded interval 含 0 时为 `undefined_baseline_interval_contains_zero`，不得用 representative 计算百分比。MetricDelta `bounds` 只表示 absolute delta interval，percentage interval 只放 `percentage_bounds`。

### 7.1 POST /api/v1/exports

请求必须且只能携带一个已经由本进程 API 返回的 public `AnalysisSnapshotReport` 或 `Comparison`。请求是严格的相邻 tagged union：

```json
{"kind":"analysis","snapshot":{}}
{"kind":"comparison","snapshot":{}}
```

两个 arm 互斥，除 `kind` 与 `snapshot` 外拒绝未知字段。服务端重新验证 schema、receipt、canonical hash 与预算，生成确定性的 `ExportArtifact`；不得接受客户端补写 raw Profile、SQL、SourceView 正文、credential 或自定义 receipt。递归脱敏和 assembler 属于 BE-07，contract crate 只冻结 wire shape，不在请求契约中伪造运行时装配。

响应顶层固定为 `schema_version="0.1"` 的：

```text
ExportArtifact
├─ schema_version = "0.1"
├─ subject                 # 严格 kind + snapshot；snapshot 是后端脱敏后的完整 public DTO
└─ manifest
   ├─ schema_version = "0.1"
   ├─ canonicalization = "doctor-canonical-json-v1"
   ├─ integrity = "self_consistent_not_signed"
   ├─ kind = analysis | comparison
   ├─ evidence
   ├─ files[]               # 恰一个 report.json
   │  └─ path/media_type/byte_length/sha256
   ├─ schemas[]             # schema_id/version/sha256
   ├─ receipt_bindings[]    # scope + ReceiptComponent；exporter 恰为 doctor-export-v0.1
   ├─ subject_hashes        # analysis 或 comparison tagged union
   └─ manifest_hash
```

`SubjectHashes` 的 analysis arm 为 `analysis_key + facts_hash`；comparison arm 为 `comparison_key` 与 baseline/candidate attempt hash，其中 success 只含 `analysis_key + facts_hash`，failed 只含 `error_state`。v0.1 固定 media type 为 `application/vnd.sr-doctor.report+json;version=0.1`。文件名只允许安全的相对逻辑名，默认不输出绝对路径、request ID、nonce、时间戳或原始 SourceView；重复输入的 canonical package bytes/hash 必须一致。

## 8. Connected endpoints

### 8.1 POST /api/v1/connections

请求包含 host、port、username、write-only password、可选 catalog/database/audit table 和 TLS mode。host 只能是 DNS/IP，不接受 scheme、path、socket 或 URL userinfo。password 只传入本请求，不进入 canonical request receipt。

成功返回 `ConnectionSessionView`：opaque ID、detected version、TLS state、capabilities/limitations、expiry，以及同一 MySQL session 探测得到的 `session_time_zone/system_time_zone/effective_time_zone/current_utc_offset_seconds`；永不回显 password。`SYSTEM` 只用 StarRocks 返回的 `system_time_zone` 解析，不读取应用主机时区；`CST` 等名称不在客户端猜测。默认最大三条、idle 15 分钟。仅 exact 3.5.14 可继续版本敏感的 Audit/Profile/Advisor/Explain 操作；其他版本返回 `unsupported_version` 能力状态并允许断开，不借用 3.5.14 pack。

错误稳定区分 DNS/connect/TLS/auth/version/probe stage。服务端原始错误只映射 code/message_id/allowlisted numeric code，不回显 DSN 或 server message 原文。

### 8.2 DELETE /api/v1/connections/{id}

立即关闭 pool、清除 secret 并使 ID 失效。重复关闭返回幂等 204；未知/过期 ID 不泄露是否曾存在。

### 8.3 POST /api/v1/connections/{id}/queries/search

请求只允许 path/body 一致的 `connection_id`、`status`、`digest` 与 `limit`。时间窗由服务端固定为最近30天，source 固定为 `auto`，客户端不能扩大窗口或指定任意表/列/排序。服务端固定 Audit column allowlist、identifier validation、参数化 filter、排序和 limit；v0.1 默认100条、最大500条。返回 `ConnectedQueryRef[]`、实际 discovery source 与 limitations。

AuditLoader unavailable/forbidden 时，`auto` 可降级 ProfileList，并明确 degraded；显式 `audit_loader` 不 fallback。固定 Audit/ProfileList 投影可将 bounded statement 暂存到后端 session-local vault，但公开列表完全不返回 SQL preview 或 stmt；只返回 typed action 和短期 opaque reference。ProfileList 固定读取最新最多 500 个 cache rows，再本地筛选/截断，响应显式标记 `filter_scope=latest_profile_cache_rows`，不冒充完整历史。

Audit 表的原始 `timestamp` 列及存储格式不修改，固定查询同时选择原始值和同一 StarRocks reader session 计算的 `UNIX_TIMESTAMP(timestamp)` 辅助列。应用只把合法正数 epoch 格式化为 RFC3339，并标记 `timestamp_state=reader_session_interpreted`；AuditLoader DATETIME 没有 producer offset，所以该值不是插件原始绝对时刻的证明。禁止使用“当前 offset”回算历史时间。0、非数字或越界值保持 `observed_at_utc=null` 并附 limitation；ProfileList 只保留 bounded raw wall time，状态为 `raw_wall_time_only`。

### 8.4 POST /api/v1/connections/{id}/profiles/fetch

请求只含 path/body 一致的 `connection_id`、UUID query ID、`discovery_generation` 与 `kind=baseline|candidate_user_external_rerun`。服务端先校验当前 session/generation 的 typed eligibility，再调用 `get_query_profile(?)`，把返回 Text Profile 交给唯一 Analysis 领域管线；不在 connector 内解析 metric/graph/rule，也不让浏览器二次上传原文。3.5.14 返回空字符串时统一为 `profile_unavailable`，仅携带 `not_found_or_expired_or_not_profiled` limitation；除非有独立权限错误证据，否则不得猜成唯一原因。

`ProfileFetchResponse` 是仅成功 DTO：`state` 只能为 `fetched`，并且 `outcome=AnalysisOutcome`、`source_hash`、`analysis_key`、`analysis_receipt` 均为必填非空字段。四项摘要与 `source_kind` 只能从同一个 `outcome.report` 投影；complete/partial Report 均可进入 Doctor。不可用、无权限、解析或分析失败统一使用非2xx `ErrorEnvelope`，协议层不能表达 `state=fetched + outcome=null`，也不返回 raw connector Profile。前端仅在 outcome 属于当前所选 query 时允许打开 Doctor；切换 query 必须使旧 outcome 失效。

`kind=baseline` 投影为 `input_origin=connected_profile_fetch`；`kind=candidate_user_external_rerun` 投影为 `candidate_profile_user_external_rerun`，不表示产品执行过候选 SQL。成功响应从 connector 保存的同代发现行传播 discovery source、raw/reader-session timestamp、status、digest、database/resource group 和 source refs，不构造 metadata-empty synthetic query，也不提升其证据等级。

### 8.5 POST /api/v1/connections/{id}/profiles/compare

请求 `ConnectedProfileComparisonRequest` 严格且只含：

```text
connection_id
baseline_query_id       # UUID；与 candidate 不同
candidate_query_id      # UUID；必须由用户在 StarRocks 侧完成受控复跑
discovery_generation    # 两侧必须来自同一当前列表代际
```

path/body `connection_id` 不一致、任一非 UUID、两 ID 相同或未知字段均在远端调用前以 400 拒绝。服务端使用同一 opaque connection session 顺序调用两次 `get_query_profile(?)`，每侧进入唯一 `analyze_input` 管线并受单 input/adapter 预算约束，再对两份后端构造的 `AnalysisAttempt` 调用 Comparator；两份 fetched Profile 的合计还受 comparison aggregate bytes/wall/concurrency 预算约束。客户端不得提交 `AnalysisSnapshotReport`、`Comparison` 或 raw Profile。

成功响应 `ConnectedProfileComparisonResponse`：

```text
baseline.query / baseline.input_origin = connected_profile_fetch
candidate.query / candidate.input_origin = candidate_profile_user_external_rerun
comparison                               # 已含两侧 typed attempts/reports，不重复摘要/hash
validation_context                        # observed_only / same session+generation / fetch order / attestation
limitations
```

candidate origin 仅记录用户外部受控复跑 attestation，产品既不执行候选 SQL，也不以 `EXPLAIN` 代替候选运行时。服务端在一次 session lease 内按 baseline→candidate 顺序拉取，并固定 `interpretation_state=observed_only`；同会话/同代际仍不足以证明因果改善。连接、fetch 或每侧 analysis 任一失败返回现有非 2xx `ErrorEnvelope`（可标识 `affected_side`）；不返回 raw connector bytes、不持久化报告，也不为成功的一侧制造 partial comparison。

### 8.6 GET /api/v1/connections/{id}/advisor-guides

只读 `SHOW PLAN ADVISOR` 的安全投影；返回 source=`starrocks_plan_advisor`、版本/guide ID、bounded reason/advice、关联 query ref 和 limitations，并固定标记 `official=true`、`experimental=true`、`scope=fe_local`、`persistence=volatile`。无权限/无 feature/无 guide 分开。不得通过此端点 ADD/DROP/TRUNCATE guide。

### 8.7 POST /api/v1/connections/{id}/explain

请求是严格 tagged union：`user_sql` 只包含单条 write-only SELECT/WITH；`plan_sql_reference` 只包含 query ID、当前 discovery generation 和 session-local opaque ref。两者都固定 COSTS。客户端不能提交 session context/variables；服务端不执行 `SET`，连接创建时已确定的 catalog/database 仅作为 provenance，session fingerprint 在 v0.1 为 unavailable + limitation。MySQL multi-statements 关闭；目标 FE 负责真实 parse/plan。输出 `ExplainSnapshot`，不混入 AnalysisSnapshotReport；估计字段不能进入 runtime metric delta。

产品 API 不提供 `/validate` 或任意 execute 端点。`EXPLAIN ANALYZE`、INSERT、UPDATE、DELETE、DDL、LOAD、ADMIN、SET、Plan Advisor mutation 和 multi-statement 必须在远端调用前拒绝为 `statement_not_allowed`。候选查询由用户在 StarRocks 侧自行受控复跑；产品刷新 Query Browser 并拉取其 candidate Profile。`ExplainSnapshot.graph_view`/runtime 在 v0.1 仍为 unavailable；未有稳定 Explain graph normalization 前不提供 PlanComparison/Explain-Explain structure delta。隔离 Docker E2E harness 可用独立 workload user 执行 fixture SELECT，但该能力不进入产品路由或前端。

## 9. ErrorEnvelope

```text
ErrorEnvelope
├─ request_id
├─ code
├─ stage                 request | security | limits | connection | query_source | detect | parse | analysis | explain | compare | export | internal
├─ message_id            stable, localizable
├─ recoverability        retry_same | change_input | reduce_input | restart | report_bug
├─ affected_side         baseline | candidate | null
├─ source_refs[]         bounded, optional
└─ details               allowlisted scalar map
```

ErrorSummary 使用同一字段子集，不含 request_id/details。任何错误都不含完整原文、SQL、凭据、文件绝对路径或 stack。

| HTTP | code examples | 语义 |
|---:|---|---|
| 400 | `invalid_request`, `invalid_multipart` | 外层请求形状错误 |
| 403 | `non_loopback_peer`, `host_not_allowed`, `origin_not_allowed`, `csrf_invalid` | 本机/跨站信任边界 |
| 413 | `input_too_large`, `line_too_long`, `too_many_lines`, `too_deep`, `too_many_nodes`, `too_many_metrics` | 资源预算 |
| 415 | `unsupported_media_type` | Content-Type 不允许 |
| 422 | `unsupported_input`, `parse_failed` | 单分析无法构造可信 Report |
| 408 | `analysis_timeout` | 同步预算耗尽；`details.reason=wall_clock_budget_exceeded`。分析在 blocking 线程池运行，到截止时刻即返回此错误，不返回部分报告；后台计算在下一个取消点停止：阶段边界（adapt / resolve / rules / assemble / 响应序列化之间），以及 adapt、resolve、assemble 内部按固定单元数（文本行、算子、指标、观测、source ref，每 256 个）和响应序列化每 256 KiB 的轮询。后台在观察到停止之前继续占用并发槽位；观察到之后只释放已分配的内存，不再计算，槽位随即释放（v0.6-08）。无 public cancelled 状态 |
| 429 | `analysis_busy` | 并发上限且不排队；`details.reason=concurrent_request_limit_reached`，上限见 capabilities `limits.concurrent_requests` |
| 500 | `internal_error` | 稳定 message/request ID，无 stack |

Connected 额外映射：

| HTTP | code examples | 语义 |
|---:|---|---|
| 401/403 | `authentication_failed`, `permission_denied` | 远端认证/最小权限不足；不回显 credential |
| 410 | `connection_expired` | 短期 session 不存在或已过期；前端必须清空该会话持有的 Profile/Compare/Explain 结果并进入重连状态 |
| 409 | `capability_unavailable`, `profile_unavailable`, `unsupported_version` | 远端能力/证据不可用；`profile_unavailable` 不猜唯一根因 |
| 422 | `explain_rejected`, `statement_not_allowed` | 计划输入不允许，或命中执行/变更 deny gate |
| 502/504 | `connection_failed`, `tls_failed`, `remote_timeout` | 远端连接/只读调用失败，server message 脱敏 |

unknown/incompatible version、metric invalid、graph unavailable、Finding not_applicable 是 200 Partial Outcome 的领域状态，不是请求级 ErrorEnvelope。

## 10. 确定性与日志

任务 21：`POST /api/v1/history/status` 的 `store.state=unavailable` 附带必填 `reason` 和 `code`；当前后端响应始终输出这两个字段，Rust DTO／JSON Schema 也要求必填。生成的 TypeScript 读取类型把它们标为可选，以兼容只有 `state` 的旧服务响应；前端缺字段时使用通用不可用文案，不推断具体原因。`available`、`disabled` 保持原结构。`reason=migration_requires_manual_resolution` 对应 `code=history_store_migration_requires_manual_resolution`，表示旧库存在重复活动治理项，需要人工处理，迁移不擅自关闭用户记录；`reason=io_or_permission` 对应 `code=history_store_io_or_permission`，涵盖目录创建、打开、权限、只读、磁盘 I/O／空间错误；其余（包括损坏／非 SQLite 文件、不支持的新库版本）为 `reason=other`、`code=history_store_open_failed`。响应不含路径、SQL、内部错误原文或已存业务值。

每次进程尝试打开历史库失败，只向 stderr 写一行 `history_store_unavailable code=<上述稳定码>`；查询状态不重复记录。历史库未配置时为 `disabled`，不写失败日志。分类在存储层读取 typed I/O／SQLite 错误码，禁止解析或输出内部错误原文。

- JSON key/canonical number/null/empty list 策略由 schema 固定；request_id、nonce、wall-clock metadata 不进入 AnalysisSnapshotReport/Comparison。
- 日志只记录 request_id、route、HTTP status、error code、connection ID、remote error class、bytes/rows/elapsed bucket 和 build commit；不记录 filename、source hash、host username、password、DSN、SQL/Profile、advisor 原文或 finding evidence text。
- 不设置持久 cookie；CSRF nonce 与 credential pool 仅驻留进程内存，重启即变化/删除。connection session 是唯一短期 server state。
- API contract tests 必须覆盖 malicious Host/Origin、Origin null、缺/错/旧 nonce、simple multipart POST、坏 Content-Type、明确 dev proxy、credential redaction、TLS downgrade、session expiry、multi-statement、`EXPLAIN ANALYZE`、任意 execute 与 Advisor mutation；所有拒绝路径必须证明远端调用次数为零。
