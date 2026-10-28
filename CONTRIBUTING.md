# 参与开发

感谢你愿意改进 SR Doctor。本文说明如何提交 Issue 与 PR；本地开发环境、测试与目录结构见 [开发指南](docs/development/README.md)。参与讨论请遵守 [行为准则](CODE_OF_CONDUCT.md)。

## 提 Issue

- **缺陷**：请附上 StarRocks 版本、部署方式、复现步骤与预期结果。需要 Profile 时，请先脱敏（可用 `scripts/calibration/desensitize-profile.mjs`），**不要上传生产数据**。
- **功能建议**：说明你要解决的问题，而不只是方案。
- **安全问题**：不要公开提交，按 [SECURITY.md](SECURITY.md) 私下报告。

## 提 PR

1. 从 `main` 新建分支。
2. 一个 PR 只做一件事；改动同时涉及后端契约与前端时，在一个 PR 里一起改。
3. 本机运行 CI 并全部通过：

   ```powershell
   pwsh -NoProfile -File scripts/ci/local-ci.ps1
   ```

   仓库不使用 GitHub Actions，合并门禁就是这个脚本。把它生成的 `summary.md` 贴进 PR 描述。
4. 提交信息使用中文，说明“做了什么、为什么”。

## 必须遵守的规则

以下几条最常被问到：

- **正确性**：
  - 所有指标只经唯一的 Typed Metric Parser 解析；
  - `missing`、`invalid`、`partial`、`overflow`、`unknown_version` 与真实的 `0` 必须可区分，禁止把缺失转成 `0`；
  - 执行结构只在关系唯一可证时推导边，禁止伪造树或 DAG；
  - 不引入综合健康分。
- **前端**：只渲染后端 DTO，不解析原始 Profile、不重算指标或规则，不使用不受信的 `innerHTML`。
- **安全**：
  - StarRocks 凭据不落盘、不进日志、不返回前端；
  - 连接保持只读；
  - 不向外部服务发送 SQL 或 Profile。
- **测试**：Parser、执行结构、信任边界与安全相关的改动必须带测试；契约变更需要重新生成契约（见开发指南）。
- **依赖**：保持最小依赖；新增依赖需要在 PR 中说明理由与许可证。
- **clean-room**：不复制任何参考项目或社区工具的源码、规则文案、阈值或素材。StarRocks 官方源码只作为语义依据。

## 许可证

提交的贡献按 [Apache License 2.0](LICENSE) 授权。
