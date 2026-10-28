# StarRocks 连接器、建议与优化验证契约

状态：Approved v0.1 connected contract / Implementation `COMPLETE`（固定 3.5.14 下 Audit/Profile、Advisor、Plan Check、Compare、SQL Runtime Top 与 Docker/浏览器 E2E 已闭环；生产集群安全仍为 `INCONCLUSIVE`）
目标版本：StarRocks 3.5.14

## 0. 官方证据锚点

| 证据 | v0.1 固定值/状态 | 用途 |
|---|---|---|
| StarRocks semantic source | `3.5.14@23a56ec517db763ea9bc7ee50b19357acdee0303` | VersionPack、Profile/Explain/Audit 语义 oracle |
| all-in-one image | `starrocks/allin1-ubuntu:3.5.14`；multi-arch index `sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f`；linux/amd64 manifest `sha256:be9418c36e3dc29b7dd299293de43ab720a84630ccc6e35931ff8b5c4a83e4e3` | 开发 E2E；运行时仍须记录实际 platform digest 与 `current_version()`，不得用镜像 digest 冒充源码 commit |
| AuditLoader | 官方文档所示插件版本 `5.0.0`，适用 StarRocks 3.3.11+；artifact `https://releases.starrocks.io/resources/auditloader.zip` | 仅 Docker bootstrap；下载后的 SHA-256 尚为 `NOT_READY`，未固定前不得安装或签 E2E GO |
| Profile retrieval | `SHOW PROFILELIST`、`get_query_profile(query_id)` | 当前 FE 的有限 Profile cache；空结果只能 `profile_unavailable` |
| Plan evidence | 普通 `EXPLAIN COSTS`、只读 `SHOW PLAN ADVISOR` | 计划/官方 guide 证据；不等于 runtime improvement |

官方文档：[AuditLoader](https://docs.starrocks.io/docs/administration/management/audit_loader/)、[Query Profile](https://docs.starrocks.io/docs/best_practices/query_tuning/query_profile_overview/)、[EXPLAIN](https://docs.starrocks.io/docs/sql-reference/sql-statements/cluster-management/plan_profile/EXPLAIN/)、[Query Feedback / Plan Advisor](https://docs.starrocks.io/docs/using_starrocks/query_feedback/)。

## 1. 能力边界

连接器不是通用 SQL 客户端。它只完成 Profile 分析所需的八件事：

1. 建立短期 MySQL 连接并验证 StarRocks 版本；
2. 探测 AuditLoader、Profile、Plan Advisor 与 Explain 能力；
3. 有界查询最近 Audit 记录或 `SHOW PROFILELIST`；
4. 按 query ID 拉取真实 Query Profile；
5. 读取官方 Plan Advisor tuning guides；
6. 对单条 SELECT/WITH 获取真实 Explain；
7. 拉取用户在 StarRocks 侧受控重跑后生成的 candidate Profile。
8. 用户显式选择 Runtime Top 时，执行一条受限 `SELECT`/`WITH ... SELECT`，固定 10 秒超时并丢弃结果行，再按同次 Query ID 拉取 Profile。

Offline import 与 Connected fetch 最终进入同一个 Adapter/NormalizedIR/RuleSet/Comparator。连接器不得出现第二套 Profile parser、graph builder、规则或 diff。

## 2. ConnectionSession

```text
CreateConnectionRequest
├─ host                 DNS name or IP, no scheme/path
├─ port                 default 9030
├─ username
├─ password             write-only secret
├─ default_catalog      optional identifier
├─ default_database     optional identifier
├─ audit_table          optional catalog.db.table, default starrocks_audit_db__.starrocks_audit_tbl__
└─ tls_mode             verify_full | disabled_loopback_docker

ConnectionSessionView
├─ connection_id        random opaque ID
├─ state                connected | degraded | expired
├─ endpoint_label       sanitized host:port
├─ detected_version
├─ pack_match / compatibility
├─ capabilities
│  ├─ audit_loader      available | unavailable | forbidden | unknown
│  ├─ profile_list
│  ├─ get_query_profile
│  ├─ explain
│  └─ plan_advisor
├─ tls_state
├─ limitations[]
└─ idle_timeout_seconds
```

Password never appears in `ConnectionSessionView`, error details, logs, trace, report or frontend storage. The API stores a bounded maximum of three sessions in process memory; default idle expiry is 15 minutes. Close and process shutdown drop the pool and secret. Session IDs are not authentication and remain protected by loopback/Host/Origin/CSRF controls.

The connector must disable MySQL multi-statements. Host is never interpreted as URL and no HTTP redirect/file/socket path is accepted. DNS resolution, TCP connect, TLS handshake and initial probe each have independent timeout. `tls_mode=disabled_loopback_docker` 只允许 `127.0.0.1`/`::1` 的一次性 Docker，并显示明确警告；所有远端连接必须 `verify_full`，校验证书链和主机名，不允许静默降级。Rust/TypeScript schema 与负向测试只使用这两个 wire enum。

**TLS 信任锚（v0.7-01 产品决策）**：`verify_full` 的根证书由 `rustls-native-certs` 0.8.4 `load_native_certs()` 提供，每次握手读取一次。默认是操作系统证书库（Windows 为系统根证书库）。集群使用内部/自签 CA 时，操作者在**启动 API 进程前**设置进程环境变量 `SSL_CERT_FILE=<PEM CA 文件>`（或 `SSL_CERT_DIR`）；设置后只信任该文件中的证书，不再合并系统根（依赖源码 `rustls-native-certs-0.8.4/src/lib.rs` 的 `load_native_certs` 文档与 `CertPaths::from_env`）。文件缺失、不可读或无证书时 `load_native_certs` 返回错误，产品 fail-closed 为 `tls_failed`。CA 路径只属于进程启动配置：连接请求、HTTP API 和前端都不接受证书路径或 URL，也不提供跳过校验的开关。客户端证书（mTLS）不在范围内。真实验证见 `docs/v0.7/tasks/01-tls-verify-full-e2e/`。

## 3. Capability probe

Probe order is deterministic and read-only:

1. `SELECT current_version()`;
2. a bounded Audit table metadata/read probe when `audit_table` is configured;
3. `SHOW PROFILELIST LIMIT 1`;
4. `SHOW PLAN ADVISOR`;
5. no-op/synthetic Explain only in the isolated Docker E2E, not against a user cluster during connection test.

Permission denial, feature absence and format incompatibility are distinct capability states. A 3.5.14 pack match does not turn a denied Audit table into an available capability. 只有 exact detected 3.5.14 才继续版本敏感的 Audit/Profile/Advisor/Explain probe；unknown/known-other 返回 unsupported 并允许断开，不查询假定 schema，也不借用 3.5.14 pack。

## 4. Query browser

```text
ConnectedQuerySummary
├─ query_id
├─ source               audit_loader | profile_list
├─ started_at
├─ state
├─ query_time_ms
├─ scan_bytes / scan_rows
├─ cpu_cost_ns / mem_cost_bytes
├─ database / resource_group
├─ digest               optional
├─ profile_fetch        available | unavailable + typed reason
├─ plan_check           available + opaque ref | unavailable + typed reason
├─ discovery_generation bounded server generation
└─ source_receipt
```

Audit 查询只选择固定 allowlisted columns，固定排序，默认 100 行、最大 500 行、最大 30 天窗口。筛选值参数化；audit catalog/database/table 必须作为逐段 identifier 校验和引用，不能直接拼接任意 SQL。固定投影可读取 bounded `stmt`，但原文只进入 connector 的 session-local secret vault 并立即从公开行剥离；浏览器、DTO、日志、fixture 均没有 preview。用户也可显式粘贴一条 SELECT/WITH 到当前内存 Explain 表单。

连接建立后用固定只读查询探测当前 session `time_zone`、FE `system_time_zone` 和当前 UTC offset。Audit 查询保留原始 `timestamp` 投影，并附加同一 reader session 的 `UNIX_TIMESTAMP(timestamp)`；应用可将其格式化为 RFC3339，但必须标记 `reader_session_interpreted`，因为 AuditLoader 的 DATETIME 没有 producer offset，不能据此证明原始绝对时刻。该设计不修改 AuditLoader 表/格式、不依赖应用主机时区，也不拿当前 offset 推算历史 DST。无法探测时区、epoch 为歧义 0/越界或格式非法时，UTC 保持 unavailable 并显式降级。

`digest` 在 v0.1 只作筛选/分组键，不宣称自动聚类。AuditLoader 与 ProfileList 是独立来源：前者是持久 Audit 表且有加载延迟，后者是当前 FE 可淘汰的有限 Profile 缓存（默认只按最新 500 cache rows 取数，再本地筛选和截断）。`auto` 降级必须返回 actual source、filter scope 和 degraded reason；显式 Audit 不 fallback，也不得伪造 audit CPU/scan/memory 字段。公开列表 DTO 永不获取完整 `stmt`。

## 5. Profile 与计划证据

### 5.1 Runtime Profile

`get_query_profile(query_id)` 返回的 Text Profile 进入既有 Text Adapter。query ID 必须符合 UUID 格式并作为 SQL 参数传递。响应保留 connection/version/query source receipt，但不把凭据或完整 SQL写入 canonical facts。

### 5.2 Explain

`EXPLAIN COSTS <single SELECT/WITH>` 由目标 StarRocks FE 生成。产品保存 Explain source hash、detected version、连接创建时已确定的 catalog/database 和 source ranges。v0.1 不接受客户端 session variable/context，也不执行任何 `SET`；`session_fingerprint_state=unavailable` 并携 limitation。Explain Adapter 与 Profile Adapter 分离；v0.1 只提取官方 3.5.14 golden 已证明的 plan node、edge、operator kind、join distribution、rollup/preaggregation/predicate/cardinality 等登记属性。

Explain 没有 runtime counters。任何 runtime metric/Finding 都必须是 unavailable/not_applicable；禁止把 estimate 当 actual，禁止把缺失当 0。

### 5.3 Candidate runtime evidence

产品 v0.1 不执行 `EXPLAIN ANALYZE` 或原始/候选 SQL。UI 提供受控复跑清单，用户在目标 StarRocks 侧自行执行一个修改后的查询并启用 Profile；产品刷新 Audit/ProfileList 后拉取 candidate Profile。开发 Docker E2E harness 可以用专用 `workload_user` 执行 fixture SELECT 产生真实 Profile，但该执行入口不暴露到产品 API，且不得复用安装 AuditLoader 的管理员凭据。

## 6. Recommendation contract

```text
Recommendation
├─ recommendation_id
├─ source               product_rule | starrocks_plan_advisor
├─ source_version / source_refs[]
├─ subject_ids[]
├─ problem_statement
├─ evidence[]
├─ confidence
├─ applicability / limitations[]
├─ action_kind          sql_shape | join_strategy | statistics | session | schema | storage | investigation
├─ suggested_action     bounded text/template, never auto-executed
├─ risk
├─ expected_direction[]
└─ validation_plan
   ├─ control_one_change
   ├─ baseline_requirements[]
   ├─ candidate_evidence  profile_required | plan_only_allowed
   └─ success_metrics[]
```

首轮建议覆盖以证据场景而非数量验收：

- operator time hotspot；
- spill/memory；
- scan/filter/storage；
- exchange/network；
- StarRocks Plan Advisor 的 join order/distribution 与 pre-aggregation guide。

产品规则必须有 3.5.14 metric/formatter/official recipe 证据、typed applicability、正/反/边界/缺失/版本/降级 golden。证据不够的场景只输出 investigation，不给具体改动。官方 Plan Advisor guide 必须以独立 source 标识，固定展示 Official、Experimental、FE-local、volatile，不能伪装成产品推理，也不由产品自动 `ALTER PLAN ADVISOR`。

## 7. Compare 语义

| Baseline | Candidate | 允许结论 |
|---|---|---|
| Profile | Profile | compatibility/structure/runtime metrics/findings 四类 delta；满足控制条件时可标 observed improvement/regression |
| Explain | Explain | plan structure/registered property delta；只能标 plan changed/unchanged |
| Profile | Explain | registered structure delta；所有 runtime delta 为 `not_comparable_missing_runtime_profile` |
| Explain | Profile | registered structure delta；不能凭单侧 runtime 推断 before/after 改善 |

即使 Profile/Profile 都完整，缺少相同 SQL digest、数据窗口、版本、session variables、resource group 或集群条件时，也只能报告“观察到差异”，不能归因于修改。Compare 必须显示 `validation_context` 与每个条件的 matched/mismatched/unknown。

## 8. API 与错误

新增连接端点见 [HTTP API 契约](./09-http-api-contract.md)。稳定错误至少区分：

- `connection_failed | tls_failed | authentication_failed | connection_expired`；
- `capability_unavailable | permission_denied | audit_table_unavailable`；
- `profile_unavailable` + 唯一组合 limitation `not_found_or_expired_or_not_profiled`；3.5.14 返回空时不得猜唯一原因。只有收到独立权限错误证据时才返回 `permission_denied`，不把空字符串解释为 forbidden；
- `explain_rejected | statement_not_allowed`。

Error/log 只记录 connection ID、stage、server error class/code 和时间/大小 bucket，不记录 password、完整 SQL、Profile、host username、绝对路径或 server message 原文。

## 9. Docker 3.5.14 E2E

必须有可重复的 `dev/starrocks-3.5.14/` 环境：

1. 以官方 multi-arch index digest启动 all-in-one；限制端口、CPU/memory 和 volume 路径；
2. 等待 FE 9030/BE healthy 并验证 `current_version()` 为 3.5.14；
3. 安装已登记版本与 SHA-256 的 AuditLoader，创建兼容 audit table；安装只发生在测试 bootstrap，不属于产品能力；
4. 创建最小 fixture schema/data，并分离 admin、`audit_loader_writer`、`doctor_reader`、`workload_user`；产品只拿 reader 凭据；
5. 开启 session profile/SQL digest，生成 baseline query/Profile/Audit row；
6. 获取 Doctor 建议，由 E2E harness 在隔离集群执行一项受控 candidate SELECT；
7. 获取 candidate Profile/Explain 并验证 Compare；
8. 验证凭据/SQL/Profile 不进日志/报告，最后可复现清理。

该 E2E 只证明隔离 3.5.14 环境。真实生产权限、TLS、拓扑、数据规模和负载影响没有证据时保持 `INCONCLUSIVE`。
