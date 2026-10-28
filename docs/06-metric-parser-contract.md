# v0.1 Typed Metric Parser 契约

状态：Approved Phase 0C contract baseline / 测试：已实现，随 v0.7.0 冻结

## 1. 唯一入口与状态

全仓唯一正式入口：

```text
parse_metric(definition: MetricDefinition, input: MetricInput, mode: ParseMode)
  -> MetricParseOutcome

ParseMode = strict | diagnostic_compat
```

`MetricInput` 保留 JSON null/number/string 或 Text lexeme；JSON number 先以原始十进制字面量读取，不先变成 f64。

```text
MetricParseOutcome
├─ parse_state           exact | missing | invalid | partial | overflow
├─ canonical_number      integer/decimal string | null
├─ canonical_unit        string | null
├─ fidelity              exact | rounded | truncated | interval | unknown
├─ bounds                BoundedInterval | null
├─ consumed_ranges[]
├─ unconsumed_ranges[]
├─ error_code            stable enum | null
└─ notes[]
```

生产规范化始终调用 `mode=strict`：整串解释才 exact。`diagnostic_compat` 仍是同一 parser 的只诊断模式，仅供 Adapter 定位可识别前缀；它可返回 `partial`，但 canonical value/bounds 必须 null，且不进入聚合、规则或 Compare。一个 metric exact、另一个 section 截断时，前者仍 exact，整体 AnalysisSnapshotReport 为 partial。

parse exact 只表示词法完整消费，不等于原始 Counter 无损。exact zero 是 canonical `"0"`；missing/invalid/partial/overflow 均不得携带伪造的 0。

fidelity 互斥定义：

- `exact`：可证明 canonical number 等于 source Counter；bounds 为 null。
- `rounded`：已知 formatter 四舍五入；必须给 `BoundedInterval`。
- `truncated`：已知 formatter 向下丢弃较小分量；必须给 `BoundedInterval`。
- `interval`：多源/派生后只剩可证区间，无法归为单一 rounding/truncation；必须给 `BoundedInterval`。
- `unknown`：无法建立数值误差界；bounds 为 null，不能参与确定阈值判断。

```text
BoundedInterval
├─ lower / upper         CanonicalNumber
├─ lower_inclusive       bool
└─ upper_inclusive       bool
```

formatter source bounds 通常是半开 `[lower, upper)`；derived 可以保留端点包含性，无法精确保留时只能向外扩成保守闭区间。canonical_number 在非 exact fidelity 下只是 display representative；规则和 Compare 必须使用 bounds 做区间判断，不得把 representative 当原始值。

## 2. 公共词法规则

- 只 trim 外围 ASCII/Unicode whitespace；trim 后必须完整消费。component 之间不允许空白。
- 单位大小写敏感；含义由 `MetricDefinition.dimension + parse_policy` 决定，不能只看后缀。
- 不接受指数、正负号、NaN、Infinity、`.5`、`1.` 或 locale comma。
- 数字语法为 `digit {digit}`，可选 `.` 后至少一位 digit；v0.1 明确允许前导零。
- raw-exact integer policy 的 Decimal × unit multiplier 必须得到整数，否则 `non_integral_canonical`；不截断、不四舍五入。
- 3.5.14 Text display policy 可以把有损 lexeme 规范化为 decimal representative + integer Counter interval；不能把 representative 冒充原值 exact。三位小数 formatter 按非负十进制 `HALF_UP` 反演：显示值 `d` 对应 `[d-0.0005,d+0.0005)` × multiplier，再与 i64 整数域及 unit-selection 区间求交；golden 必须回放官方 formatter 验证边界。
- duration/bytes/count 的底层 Counter 区间必须落在 `0..i64::MAX`；越界或中间运算越界为 overflow。
- ratio 使用精确十进制字符串，最大 scale 18；超过精度/范围为 overflow，不经 f64。
- raw lexeme、range、state、canonical value/unit 一起序列化。Formatter 只接收 canonical value，UI 不反解析 display。

## 3. Duration grammar

```ebnf
duration       = component, { component } ;
component      = number, duration_unit ;
number         = digit, { digit }, [ ".", digit, { digit } ] ;
duration_unit  = "ms" | "us" | "ns" | "h" | "m" | "s" ;
```

词法器按最长 token 匹配；`m` 不能抢占 `ms`，`s` 不能抢占 `ms/us/ns`。3.5.14 官方 formatter/parser 只使用 ASCII `us`；Greek `μs` (U+03BC) 与 micro sign `µs` (U+00B5) 在 strict pack 下均为 `unknown_unit`，也不得作为版本检测证据。

约束：

- unit 严格从大到小：`h > m > s > ms > us > ns`，不得重复。
- 只有最后一个 component 可含小数；`1m30.5s` 合法，`1.5m30s` 非法。
- 结果必须是整数 ns；`1.000000001s` 合法，`1.0000000001s` 为 `non_integral_canonical`。
- 不允许 unitless duration，除非某个 JSON metric definition 明确声明 source number 已是 ns；该情况不走字符串 unit grammar。
- 3.5.14 Text fidelity policy：`<1us` 和带完整 ns remainder 的 us 显示可 exact；`>=1ms` 截断到 us，`>=1s` 截断到 ms，`>=1m` 截断到 s，`>=1h` 截断到 min。parser 返回 displayed representative 与半开 Counter 区间 `[lower, upper)`，不将 `500ms` 称为原始 Counter exact。

## 4. Count grammar

```ebnf
count          = number, [ count_unit ], [ raw_count ] ;
count_unit     = "K" | "M" | "B" ;
raw_count      = " ", "(", integer, ")" ;
```

- multiplier：K=1,000；M=1,000,000；B=1,000,000,000；canonical unit=`item`。
- Decimal × multiplier 必须是 exact integer；`1.5K=1500`，`0.0001K` 非整数则 invalid。
- parenthetical raw count 若存在，它是 authoritative exact integer；缩写值只做显示一致性校验，允许官方三位小数 rounding 区间覆盖 raw count。区间不含 raw count 才 `raw_count_mismatch`。
- 3.5.14 官方 formatter 在 K/M/B 后总附括号 raw count；有 suffix 但无括号只允许 registry 明示的 `display_compat` policy，并标为 rounded/interval，不能声称是原生 3.5.14 exact 输出。
- unitless count 是否允许由 registry 决定；`m` 在 count 维度非法。

## 5. Bytes grammar

```ebnf
bytes          = number, [ " " ], bytes_unit ;
bytes_unit     = "B" | "KB" | "MB" | "GB" | "TB" ;
```

- StarRocks 3.5.14 虽使用 `KB/MB/GB/TB` 标签，`DebugUtil.getByteUint` 实际倍率为 1024；canonical unit=`byte`。不能按 SI 1000 解释。
- 3.5.14 Text bytes 固定三位小数，是 rounded/interval；JSON/raw integer policy 才要求 exact integer。
- 大小写严格；`KiB/MiB/GiB/TiB` 不是 3.5.14 输出，在 v0.1 pack 下与 `kb/kib/Kib/1M` 一样为 unknown_unit。
- unitless bytes 是否允许以及 source number 的 unit 由 registry 明确；无声明时 invalid。

## 6. Ratio / Percent grammar

v0.1 只保留一个 dimension=`ratio`、canonical unit=`ratio`。parse policy 指定输入是裸 ratio 或 percent：

```ebnf
ratio          = number ;
percent        = number, "%" ;
```

- percent 规范化除以 100：`25% -> 0.25 ratio`。
- 是否限制 0..1/0..100 由 metric definition 的 min/max policy 决定；P0 bounded-percent fixture 对 `101%` 返回 `out_of_range`。
- `25 %`、`25%%`、`%25`、负数、NaN 非法。
- rate 不是 v0.1 parser dimension。遇到 `MB/s`、`row/s` 等不调用正式 parser，由 registry/adapter 保留 raw/provenance 并生成 semantic unavailable diagnostic；后续版本另开契约。

## 7. 稳定错误代码

| code | state | 含义 |
|---|---|---|
| `metric_missing` | missing | 字段缺失、null、空/全空白 |
| `invalid_number` | invalid | 数字词法不合法 |
| `unknown_unit` | invalid | unit 不属于 dimension policy |
| `unit_required` | invalid | 此 metric policy 禁止 unitless 输入 |
| `trailing_input` | invalid/partial | strict 有未消费后缀；compatibility 仅诊断 partial |
| `leading_input` | invalid | 有未消费前缀 |
| `duplicate_unit` | invalid | unit 或 alias 重复 |
| `unit_order` | invalid | 组合单位逆序 |
| `decimal_not_last` | invalid | 非最后 component 使用小数 |
| `non_integral_canonical` | invalid | integer dimension 换算后有小数 |
| `raw_count_mismatch` | invalid | 括号 raw count 与缩写不一致 |
| `out_of_range` | invalid | 违反 metric min/max policy |
| `numeric_overflow` | overflow | 超过 canonical 范围/精度 |

错误响应不回显整份 Profile；Metric diagnostic 只引用短 lexeme/source range 和 allowlisted code。

## 8. P0 golden matrix

每例同时断言 parse_state、canonical representative/unit、fidelity/bounds、consumed/unconsumed range、error code 和序列化 DTO。`raw_exact` 表示 JSON 原始 counter 或明确无损输入；`sr3514_text_display` 表示 3.5.14 `RuntimeProfile.printCounter` 文本。

### 8.1 Duration

| input / policy | expected |
|---|---|
| `500ms` / sr3514_text_display | parse exact；representative `500000000 ns`；fidelity truncated；bounds `[500000000,500001000)` |
| `500us` / sr3514_text_display | parse exact；fidelity exact；`500000 ns`；此 formatter 分支保留完整 ns remainder |
| `500μs` / `500µs` | invalid `unknown_unit`；不是 3.5.14 官方 alias |
| `500ns` / sr3514_text_display | parse exact；fidelity exact；`500 ns` |
| `1m30s` / sr3514_text_display | parse exact；representative `90000000000 ns`；truncated；bounds `[90000000000,91000000000)` |
| `1m59s` / sr3514_text_display | parse exact；representative `119000000000 ns`；truncated；bounds `[119000000000,120000000000)`；证明 minute 分支仍保留 seconds |
| `1h30m` / sr3514_text_display | parse exact；representative `5400000000000 ns`；truncated；bounds `[5400000000000,5460000000000)` |
| `1h` / sr3514_text_display | parse exact；representative `3600000000000 ns`；truncated；bounds `[3600000000000,3660000000000)`；hour 分支丢弃 sub-minute |
| `2s345ms` / sr3514_text_display | parse exact；representative `2345000000 ns`；truncated；bounds `[2345000000,2346000000)` |
| `1m30s500ms` / raw_exact | parse exact；fidelity exact；`90500000000 ns` |
| `1m2ms` / raw_exact | parse exact；fidelity exact；`60002000000 ns`，验证最长匹配 |
| `1.5s` / `1m30.5s` / raw_exact | parse exact；fidelity exact；`1500000000` / `90500000000 ns` |
| `1.000000001s` / raw_exact | parse exact；fidelity exact；`1000000001 ns` |
| `01.5s` / raw_exact | parse exact；fidelity exact；`1500000000 ns`，冻结允许前导零 |
| `1.0000000001s` | invalid `non_integral_canonical` |
| `1.5m30s` / `1.5s500ms` | invalid `decimal_not_last` |
| `1m2m` / `1us1us` | invalid `duplicate_unit` |
| `30s1m` / `1ms2s` | invalid `unit_order` |
| `junk500ms` / `500msjunk` | invalid `leading_input` / `trailing_input`；无 value |
| `1e3ms` / `+1s` / `-1s` / `.5s` / `1.s` / `NaNms` | invalid `invalid_number` |
| missing / JSON null / empty / whitespace | missing `metric_missing` |
| string `null` / unitless `0` | invalid `invalid_number` / `unit_required` |
| `0ms` / `0s0ms` / raw_exact | parse exact；fidelity exact；canonical zero |
| `9223372036854775807ns` / raw_exact | exact boundary |
| `9223372036854775808ns` | overflow `numeric_overflow` |
| `9223372037s` / `9223372036s854775808ns` | intermediate multiply / add overflow `numeric_overflow` |

Strict `500msjunk` 无 canonical value；compatibility diagnostic 可为 partial、记录 consumed/unconsumed，但仍无 value。

### 8.2 Count

| input / policy | expected |
|---|---|
| `258.794M` / display_compat | parse exact；representative `258794000 item`；fidelity rounded；bounds `[258793500,258794500)` |
| `1K` / `1M` / `1B` / raw_exact | parse exact；fidelity exact；`1000/1000000/1000000000 item` |
| `1.5M` / raw_exact | parse exact；fidelity exact；`1500000 item` |
| `0.0000001M` | invalid `non_integral_canonical` |
| `258.794M (258794000)` | parse exact；parenthetical authoritative；fidelity exact |
| `258.794M (258794001)` | parse exact；parenthetical authoritative；fidelity exact（仍在显示 rounding 区间） |
| `258.794M (258793500)` / `(258794499)` | exact lower/upper-inside boundary |
| `258.794M (258794500)` | invalid `raw_count_mismatch`，upper endpoint excluded |
| `258.794M (258800000)` | invalid `raw_count_mismatch` |
| `1m` / `NaNM` | invalid `unknown_unit` / `invalid_number` |
| `0M` / raw_exact | parse exact；fidelity exact；canonical zero |
| `000M` / raw_exact | parse exact；fidelity exact；canonical zero，冻结允许前导零 |
| `258.794M` as duration | invalid `unknown_unit` |
| `9223372037B` | overflow `numeric_overflow` |

`1B` 必须另在 bytes dimension 测试，证明同字面量由 registry/dimension 决定含义。

### 8.3 Bytes

| input / policy | expected |
|---|---|
| `1B` / raw_exact | parse exact；fidelity exact；`1 byte` |
| `1KB` / `1MB` / `1GB` / `1TB` / raw_exact | parse exact；fidelity exact；`1024/1048576/1073741824/1099511627776 byte` |
| `1.5KB` / raw_exact | parse exact；fidelity exact；`1536 byte` |
| `1.000 KB` / sr3514_text_display | parse exact；representative `1024 byte`；fidelity rounded；bounds `[1024,1025)`，倍率必须是 1024 |
| `1.021 KB` / sr3514_text_display | parse exact；representative `1045.504 byte`；fidelity rounded；bounds `[1045,1047)`；1045/1046 replay 成 1.021，1044/1047 在区间外 |
| `1KiB` / `1MiB` / `1GiB` / `1TiB` | invalid `unknown_unit` |
| `0B` / raw_exact | parse exact；fidelity exact；canonical zero |
| `0.1B` | invalid `non_integral_canonical` |
| `1kb/1kib/1Kib/1M` | invalid `unknown_unit` |
| unitless `1024` | allowed policy exact；forbidden policy invalid `unit_required` |
| `9223372036854775807B` / raw_exact | exact boundary |
| `9223372036854775808B` | overflow `numeric_overflow` |

### 8.4 Ratio

| input/policy | expected |
|---|---|
| `25%` bounded percent | exact `0.25 ratio` |
| `0%` | exact zero |
| `0.25` raw ratio | exact `0.25 ratio` |
| `101%` bounded percent | invalid `out_of_range` |
| `25 %` / `25%%` | invalid `trailing_input` |
| `%25` | invalid `leading_input` |
| `NaN%` | invalid `invalid_number` |
| `0.123456789012345678` / `0.1234567890123456789` raw ratio | exact scale-18 boundary / overflow `numeric_overflow` |
| null/empty | missing `metric_missing` |

DerivedMetric 的除零不由 parser 处理；由公式返回 `invalid_denominator`。

## 9. 官方证据锚点

| evidence | 3.5.14 事实 | 契约影响 |
|---|---|---|
| `fe/fe-core/src/main/java/com/starrocks/common/util/RuntimeProfile.java::printCounter`，420-468 | count 后缀保留括号 raw value；bytes 固定三位；TIME_NS 分量按量级截断 | parenthetical count 优先；bytes/time 携带 fidelity/bounds |
| `fe/fe-core/src/main/java/com/starrocks/common/util/DebugUtil.java::getByteUint` | KILOBYTE 起按 1024 连乘，但显示标签仍是 KB/MB/GB/TB | 3.5.14 pack 禁止按 SI 1000 或 IEC `KiB` 解释 |
| `gensrc/thrift/RuntimeProfile.thrift` | Counter strategy 有 aggregate/merge/min-max 三组枚举 | parser 只负责单值；source merge 与产品分析聚合分层 |

证据固定到官方基线 commit `23a56ec517db763ea9bc7ee50b19357acdee0303`。后续版本不得复用这些行为假定，必须由对应 VersionPack 重新确认。

## 10. Aggregation 与阈值类型

这里的 Registry enum 固定为产品阶段 `analysis_aggregation`：`sum | max | min | min_max_range | weighted | non_additive`。它不是官方 Counter 的 aggregate/merge/min-max strategy；官方 strategy 作为 source metadata 先完成 source merge。未知字段默认 non_additive。规则阈值使用相同 dimension/canonical unit；规则加载时做类型检查，duration ns 不得与无单位裸数比较。

## 11. 最少充分测试

- table-driven goldens：第 8 节全部案例。
- raw_exact equivalence：`1s == 1000ms == 1000000us == 1000000000ns`；不把不同 Text display fidelity 的 formatter 输出混入此断言。
- bounded property cases：固定 seed，每 dimension 最多 5,000 case；无无界 fuzz。
- integer boundary、中间乘法/加法 overflow、Unicode μ/µ strict rejection、prefix/suffix、重复/逆序单位。
- parser 不 panic；超长 lexeme 在请求/field limit 前被拒绝。
- lossless export formatter 可 round-trip；人类 display formatter 允许舍入，因此不做 display -> parse 等值承诺。UI 不参与 parser 测试。

本矩阵和 Rust/TypeScript serialization goldens 未通过前，禁止实现 RuleSet 阈值或 Performance UI。
