# 文档中心

## 使用者

| 文档 | 内容 |
|---|---|
| [快速开始](guide/quick-start.md) | 5 分钟部署并完成第一次诊断 |
| [部署指南](guide/deployment.md) | 一键部署、离线安装、HTTPS、本机进程、SSH 隧道、备份与升级 |
| [配置参考](guide/configuration.md) | 全部环境变量、默认值与范围 |
| [使用指南](guide/user-guide.md) | 查询、诊断、建议、对比、趋势、告警、治理 |
| [常见问题](guide/faq.md) | 登录、连接、监控相关的排错 |
| [安全策略](../SECURITY.md) | 安全模型、已知限制与漏洞报告 |
| [更新日志](../CHANGELOG.md) | 各版本的主要变化 |

## 开发者

| 文档 | 内容 |
|---|---|
| [开发指南](development/README.md) | 环境、仓库结构、构建、测试、本地 CI、Docker E2E |
| [架构概览](development/architecture.md) | 系统结构、请求路径与关键设计 |
| [参与开发](../CONTRIBUTING.md) | Issue、PR 与必须遵守的规则 |

## 设计规范与契约

v0.1 起建立、之后持续沿用的规范。文首的“状态”说明了它写于哪个阶段；

| 编号 | 文档 |
|---|---|
| 00 | [产品边界与开源基线策略](00-product-scope-and-baselines.md) |
| 01 | [目标系统架构](01-target-architecture.md) |
| 02 | [统一 IR、Graph、RuleSet 与 Compare 契约](02-unified-ir-and-diagnosis.md) |
| 03 | [研发路线与验收门禁](03-delivery-roadmap.md) |
| 04 | [源码分析证据账本](04-source-analysis-evidence.md) |
| 05 | [基线采用裁决](05-baseline-adoption-decision.md) |
| 06 | [Typed Metric Parser 契约](06-metric-parser-contract.md) |
| 07 | [v0.1 实现与 Candidate 验收矩阵](07-v0.1-acceptance-matrix.md) |
| 08 | [多基线三角校验协议](08-comparative-baseline-protocol.md) |
| 09 | [HTTP API 与失败语义](09-http-api-contract.md) |
| 10 | [Fixture、VersionPack 与 RuleSet 治理](10-fixture-versionpack-and-ruleset-contract.md) |
| 11 | [StarRocks 连接器、建议与优化闭环](11-starrocks-connector-and-optimization-loop.md) |

## 素材

- [产品截图](assets/screenshots/)：README 使用的真实产品截图。
