# 常见问题

## 部署

<details>
<summary><b>打开页面后一直要求输入令牌 / 提示令牌不正确</b></summary>

- 在服务器上用 `./deploy.sh token` 确认当前令牌。换过令牌（`rotate-token`）后，旧令牌立即失效。
- 60 秒内累计输错 5 次后，所有登录都会被暂时拒绝，等一分钟再试。
- 经 HTTPS 反向代理访问时，确认用 `./deploy.sh --https` 部署过，并且代理转发了 `Host` 头。

</details>

<details>
<summary><b>浏览器打不开 <code>http://服务器:8000</code></b></summary>

1. 在服务器上运行 `docker compose ps`（或 `./deploy.sh status`），确认 `doctor` 是 `healthy`。
2. 在服务器上运行 `curl http://127.0.0.1:8000/api/v1/health`，应返回 `{"status":"ok"}`。
3. 检查服务器防火墙和云安全组是否放通了端口；如果部署时用了 `--bind`，确认使用者的网络能到达那个地址。
4. Windows 的 Docker Desktop 只把端口发布在本机回环地址上，其他电脑访问不到，这是 Docker Desktop 的限制。

</details>

<details>
<summary><b>构建镜像时下载依赖失败</b></summary>

首次构建要从 Docker Hub、npm 和 crates.io 下载依赖。服务器不能访问外网时，在能联网的机器上运行 `./deploy.sh package`，把离线安装包复制到服务器安装，见 [离线安装](deployment.md#离线安装)。

</details>

<details>
<summary><b>报错 “all predefined address pools have been fully subnetted”</b></summary>

Docker 的默认地址池用完了。根目录的 `compose.yaml` 已经使用默认的 `bridge` 网络，不会新建网络；如果你在 `compose.override.yaml` 里加了自定义网络，请去掉它，或者清理不用的网络：`docker network prune`。

</details>

<details>
<summary><b>非 root 用户运行脚本提示无法访问 Docker</b></summary>

把用户加入 `docker` 组（`sudo usermod -aG docker $USER`，重新登录后生效），或用 `sudo ./deploy.sh`。注意：`docker` 组的成员实际上拥有 root 权限。

</details>

## 连接

<details>
<summary><b>连接失败</b></summary>

- 主机和端口填的是 **FE** 的地址与 MySQL 端口（默认 `9030`），不是 HTTP 端口 `8030`。
- 从**服务器**（不是你的电脑）测试连通性：`nc -vz <FE地址> 9030`。
- 选了 TLS verify-full 时，FE 必须开启 SSL，证书的主机名要和填写的主机一致；私有 CA 的配置见 [部署指南](deployment.md#连接开启-ssl-的-starrocks)。
- 容器里连接宿主机上的 StarRocks 时，不要填 `127.0.0.1`（那是容器自己）。Docker Desktop 填 `host.docker.internal`；Linux 填宿主机在网桥上的地址，通常是 `172.17.0.1`。

</details>

<details>
<summary><b>页面显示“会话已过期 · 请重连”</b></summary>

连接会话空闲 15 分钟、服务重启或手动断开后，口令会从内存中删除，需要重新输入。这是有意的设计：口令永远不落盘。

</details>

<details>
<summary><b>审计历史是空的或不可用</b></summary>

审计历史读取 AuditLoader 写入的审计表。请确认集群安装并启用了 AuditLoader，并且连接账号有读取审计表的权限。没有审计表时，仍然可以用 **Profile 列表**、**运行中** 或 **导入 Profile**。

</details>

<details>
<summary><b>支持哪些 StarRocks 版本？</b></summary>

支持 3.1 及以后的版本，目前已审计 3.1、3.2、3.3、3.4、3.5、4.0、4.1。**2.5、3.0 不支持**：它们没有官方的 `ExplainAnalyzer`，Profile 里也没有拓扑和最终/合并标记，执行图、耗时占比和结论都只能自行推导、无法用官方结果验证，而且官方早已停止维护。完整的版本表与原因见 [README](../../README.md#支持的-starrocks-版本)。

规则只有一套（3.5.14 基线），其他版本的差异来自对官方源码的逐版本审计，例如：

- 3.1.0 的文本 Profile 打印格式不同（`1.5ms` 表示 1 毫秒 5 微秒），会按版本解码；
- 3.3.0～3.3.11、3.4.0～3.4.1 的官方合并缺陷会把最大值算错，这些版本不使用 `__MAX_OF_` 作为证据；
- 3.4 之前没有 Plan Advisor，界面会如实显示为不可用。

页面顶部的版本标签会写明“3.5.14 基线 + 某版本线差异”。每条差异的源码证据见 [多版本源码审计报告](../../fixtures/version-audit/version-audit.md)。

</details>

<details>
<summary><b>提示“无版本包”或“版本未知”</b></summary>

只有已审计的版本（3.1 及以后各版本线中已审计的补丁版本）会套用版本包。2.5、3.0 一律按不支持处理。审计范围外的版本，包括已支持版本线中更新的补丁，不会自动套用最近版本的语义：

- 连接这样的集群时，连接标为受限，不提供审计与 Profile 读取；
- 导入这样的 Profile 时显示“无版本包”，指标标为不兼容或不可用。

新版本不新增版本包：先对官方源码做审计，把相对 3.5.14 基线的差异写进 `version-deltas.json`，再在官方镜像上冒烟，逐节点对照官方 `ANALYZE PROFILE`，通过后才登记为支持，见 [开发指南](../development/README.md#新增-starrocks-版本)。

</details>

## 监控

<details>
<summary><b>趋势、告警、治理页面没有数据</b></summary>

- **“此运行方式未开启历史监控”**：你用的是 `deploy/local/compose.yaml` 的单机容器，它关闭了历史。改用根目录 `compose.yaml` 的服务器部署或本机进程，见 [部署指南](deployment.md#选择部署方式)。
- **连接的是远端集群**：远端集群默认只连接、不采集，直到生产只读试点（G1）通过，见 [远端集群的后台采集](deployment.md#远端集群的后台采集)。
- **刚开始采集**：基线需要足够的样本，之前会显示“样本不足”。

</details>

<details>
<summary><b>为什么没有一个总的健康分？</b></summary>

单一分数会把严重度、置信度和证据完整度混在一起，也很难说明它的含义。本产品分开展示这几项，并写明限制，方便你判断结论有多可靠。

</details>

<details>
<summary><b>历史数据会保存多久？包含 SQL 原文吗？</b></summary>

默认保留 30 天，过期自动删除。历史库只存脱敏模板、指纹、小时聚合与告警治理记录，不存 SQL 字面量、Profile 原文或口令。用 `./deploy.sh backup` 备份。

</details>

## 其他问题

请在 [GitHub Issues](https://github.com/shenhui027-arch/sr-doctor/issues) 提问。安全问题请按 [SECURITY.md](../../SECURITY.md) 私下报告，不要公开提交。
