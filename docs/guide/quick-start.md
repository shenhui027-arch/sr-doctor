# 快速开始

5 分钟把 SR Doctor 跑起来，并完成第一次诊断。

## 你需要

- 一台 Linux 服务器（x86_64；arm64 可在本机构建，尚未验证），装有 **Docker Engine** 与 **Compose**（`docker compose` 插件，或 1.29 以上的 `docker-compose`）。
- 服务器能访问 StarRocks FE 的 MySQL 端口（默认 `9030`）。支持 StarRocks 3.1 及以后的版本（已审计到 4.1，2.5、3.0 不支持）。
- 使用者的电脑能访问服务器的服务端口（默认 `8000`）。

没有 StarRocks 集群也可以：服务起来后，在首页导入 Profile 文件即可离线分析。

> Windows 个人试用：装好 Docker Desktop 后，同样运行下面的命令，然后用 `http://127.0.0.1:8000` 访问（Docker Desktop 只把端口发布在本机回环地址上）。macOS 尚未验证。

## 第 1 步：部署

```bash
git clone https://github.com/shenhui027-arch/sr-doctor.git
cd sr-doctor
docker compose up -d --build
```

首次运行会编译前后端，需要十几分钟；之后启动只要几秒。`docker compose ps` 里 `doctor` 显示 `healthy` 后，查看首次启动时自动生成的访问令牌：

```bash
docker compose exec doctor cat /etc/sr-doctor/access-token
```

把访问地址 `http://服务器地址:8000` 和令牌发给需要使用的人。

- 端口被占用：在仓库根目录的 `.env` 里写 `DOCTOR_PORT=18000`，再运行 `docker compose up -d`。
- 想要状态、备份、升级等运维命令：用 `./deploy.sh`，它包装的是同一个 `compose.yaml`，见 [部署脚本](deployment.md#部署脚本-deploysh)。
- 服务器无法访问外网：见 [离线安装](deployment.md#离线安装)。

## 第 2 步：登录

浏览器打开访问地址，输入访问令牌。令牌只用来换取浏览器会话（12 小时有效），页面不会保存令牌本身。

## 第 3 步：连接 StarRocks

点右上角 **连接**，填写：

| 字段 | 示例 |
|---|---|
| 主机 | `10.0.0.21`（FE 的 IP 或主机名） |
| 端口 | `9030` |
| 用户名 / 口令 | 建议使用只读账号 |
| 传输方式 | **明文**（默认，任意可达主机）或 **TLS verify-full**（FE 已开启 SSL） |

口令只保存在服务进程的内存里，断开、空闲 15 分钟或服务重启后即删除。

## 第 4 步：诊断第一条 SQL

1. 打开 **查询 → 审计历史**，找到一条慢 SQL；也可以在 **Profile 列表** 里直接选。
2. 点 **拉取 Profile 并诊断**，查看执行结构图、发现项和证据。
3. 切到 **建议**，查看排查步骤与验证计划。
4. 在 StarRocks 上按建议改完并重跑后，用 **前后对比** 确认是否真的变快了。

不连接时：首页点 **导入 Profile**，选择从 StarRocks 导出的 JSON 或文本 Profile。

## 下一步

- [使用指南](user-guide.md)：每个工作区的用法与读法
- [部署指南](deployment.md)：HTTPS、离线安装、备份与升级
- [常见问题](faq.md)
