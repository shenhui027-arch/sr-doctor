# 基线采用裁决

状态：v0.1 实现前的裁决，之后持续沿用
日期：2026-08-29

## 1. 决策

开发采用“官方语义裁判 + 独立基线三角校验 + 产品契约自研”，不 fork 任何社区 Profile 工具。

```text
Official StarRocks fixed release source
    -> semantic oracle and calibration fixtures
Community Profile tools
    -> observed interaction habits and counterexamples only
Independent implementations / upstream systems
    -> alternative solutions and counterexamples, never majority truth
Product repository
    -> independent typed parser, IR, graph, rules, API and UI
```

## 2. 为什么官方 ExplainAnalyzer 是最强参考

它与 FE 的 `ProfilingExecPlan`、`RuntimeProfile` 和 counter 语义处于同一代码库，最接近 StarRocks 生成 Profile 时的真实结构、节点映射和聚合含义；源码公开且带 Apache-2.0 头。

但它不是外部文本/JSON 输入解析器，也不是可直接部署的服务。产品只能逐项借鉴并用固定 release 校准，不能把 FE 内部对象依赖带入独立产品。

## 3. 采用清单

| 来源 | 允许采用 | 禁止直接采用 |
|---|---|---|
| 官方 StarRocks | 指标定义、节点语义、聚合方式、版本差异、校准样本 | FE 内部 API 作为运行时依赖；未经许可证审查的大段复制 |
| 社区 Profile 工具 | 可见的页面信息架构、图交互、两份 Profile 的对比任务流 | 任何代码、解析规则、阈值、文案与素材；前端自算指标、`innerHTML` 渲染、公共分享、假拓扑、固定扣分评分（见 [证据账本 §4](./04-source-analysis-evidence.md#4-社区工具中常见的缺陷类别作为反例)） |

## 4. 被否决方案

### fork 某个社区 Profile 工具

否决：观察到的工具要么让浏览器承担语义解析，要么主链存在单位与可达性问题、拓扑降级不透明、评分模型不成立，测试、安全、版本适配与许可证边界也没有闭合；修补成本会演变为重写。

### 直接移植 ExplainAnalyzer

否决：FE 内部类型与离线输入模型不匹配，升级耦合高。应移植“语义测试”，而不是移植内部架构。

## 5. Release 固定策略

- 每个支持的 StarRocks release 都固定 tag/commit，不以滚动 `main` 作为兼容承诺。
- 语义基线是 `StarRocks 3.5.14@23a56ec517db763ea9bc7ee50b19357acdee0303`；其他版本必须经过源码审计、登记版本差异并通过官方镜像冒烟，才改为 `supported`（v1.0 起支持 3.1 及以后）。
- 每次升级都要生成字段、单位、聚合、拓扑、规则适用性的差异，并运行全量黄金语料。
- 未覆盖的版本返回 `unknown_version` 或不支持，不伪装为最近的兼容版本。

## 6. Clean-room 与许可证

- 证据账本记录概念、行为、输入输出和短小必要引用；产品实现从契约和 Fixture 出发。
- 不复制任何第三方 Profile 工具的代码或素材。
- 官方代码复用必须逐文件确认 Apache-2.0 义务并保留 NOTICE/attribution。
- 第三方 Fixture 只有在来源、授权和脱敏通过后才可导入；优先自造等价最小样本。

## 7. 治理规则

任何“参考实现也是这样”的主张都不能直接关闭缺陷。合入前必须回答：官方语义证据是什么、独立参考是否真正独立、输入版本是什么、规范单位是什么、聚合语义是什么、缺失如何表示、负例是否通过。若独立项目也没有可靠解法，不继续拖延，转入有明确 claim boundary 的产品契约自研。

本裁决的复审触发条件：语义基线 release 改变、官方 Profile schema 重大变化，或出现新的官方 standalone analyzer。
