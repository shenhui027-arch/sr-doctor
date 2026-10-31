# 部署指南

## 选择部署方式

| 方式 | 适合 | 历史监控（趋势 / 告警 / 治理） | 访问方式 |
|---|---|---|---|
| [Docker Compose 一键部署](#docker-compose-一键部署推荐) | 团队共用一台 Linux 服务器 | ✅ | 浏览器打开 `http://服务器:8000`，用 `admin` / `123456` 登录 |
| [部署脚本 deploy.sh](#部署脚本-deploysh) | 同上，并且想要状态、备份、升级等运维命令 | ✅ | 同上 |
| [离线安装](#离线安装) | 服务器不能访问外网 | ✅ | 同上 |
| [本机进程](#本机进程) | 个人使用、开发调试 | ✅ | `http://127.0.0.1:8000`，无需登录 |
| [常开主机 + SSH 隧道](#常开主机--ssh-隧道) | 个人使用，采集不随笔记本休眠中断 | ✅ | SSH 隧道后访问 `http://127.0.0.1:8000` |
| [Windows 单机容器](#windows-单机容器) | 在 Windows 上快速看一眼 | ❌ | `http://127.0.0.1:8000`，无需登录 |

前三种是**网络模式**：服务监听所有网卡，需要登录（默认账号 `admin` / `123456`）。后三种是**回环模式**：只监听 `127.0.0.1`，不需要登录。

v1.0 起服务器部署统一用 Docker。

## Docker Compose 一键部署（推荐）

### 准备

- **服务器**：Linux x86_64，已安装 Docker Engine 与 Compose（`docker compose` 插件，或 1.29 以上的 `docker-compose`）。[安装 Docker](https://docs.docker.com/engine/install/)。arm64 服务器同样可以本机构建：已在模拟环境中构建并验证启动、登录与 Profile 分析，尚未在真实 arm64 硬件上运行。
- **资源**：运行时 1 核、512 MiB 内存足够；首次构建镜像需要 2 核以上和数 GB 磁盘（Rust 编译缓存）；用[离线安装包](#离线安装)则不需要构建。
- **网络**：使用者 → 服务器的服务端口（默认 `8000`）；服务器 → StarRocks FE 的 MySQL 端口（默认 `9030`）。
- **权限**：运行命令的账号要能使用 Docker（root，或已加入 `docker` 组）。

### 安装

```bash
git clone https://github.com/shenhui027-arch/sr-doctor.git
cd sr-doctor
docker compose up -d --build
```

首次启动会构建镜像 `sr-doctor:local`（十几分钟，之后复用缓存），然后启动 `doctor`，监听 `8000` 端口。`docker compose ps` 中 `doctor` 显示 `healthy` 即可使用。

浏览器打开 `http://服务器地址:8000`，用默认账号 `admin` / `123456` 登录，点“连接”，填写 StarRocks FE 的 IP、端口（默认 `9030`）、用户名、口令。

默认密码写在公开文档里，服务暴露在不完全可信的网络时请先改密码：在根目录的 `.env` 里写 `DOCTOR_ADMIN_PASSWORD=新密码`，再运行 `docker compose up -d`。

用旧版 `docker-compose` 时，把上面的 `docker compose` 换成 `docker-compose` 即可。

### 配置

端口、监听地址、登录密码等写在仓库根目录的 `.env` 里，`docker compose` 会自动读取：

```bash
DOCTOR_PORT=18000            # 对外端口
DOCTOR_BIND=10.0.0.5         # 只在这一个网卡上监听
DOCTOR_ADMIN_PASSWORD=change-me-2026   # 登录密码（默认 123456）
DOCTOR_COOKIE_SECURE=on      # 经 HTTPS 反向代理访问时
```

改完运行 `docker compose up -d` 生效。需要挂载文件或增加环境变量时，写在根目录的 `compose.override.yaml` 里，Compose 会自动合并，升级时也不会被覆盖。这两个文件都已被 Git 忽略。全部选项见 [配置参考](configuration.md)。

### 升级与常用命令

```bash
git pull --ff-only && docker compose up -d --build --remove-orphans   # 升级；历史数据在卷里，保持不变
docker compose ps                                     # 状态
docker compose logs -f doctor                         # 日志（不含密码、口令或 SQL 原文）
docker compose down                                   # 停止并删除容器，保留历史数据
```

## 部署脚本 deploy.sh

`./deploy.sh` 用的是同一个 `compose.yaml`，另外负责等待就绪、打印访问地址、备份恢复和离线打包：

```bash
./deploy.sh                          # 安装或更新并启动，打印访问地址与登录账号
./deploy.sh --port 18000             # 换端口
./deploy.sh --bind 10.0.0.5          # 只在这一个网卡上监听
./deploy.sh --password 新密码        # 改登录密码（默认 123456），已登录的浏览器要重新登录
./deploy.sh --https --bind 127.0.0.1 # 放在 HTTPS 反向代理后面（见下文）
```

这些选项会写入根目录的 `.env`，之后的命令、升级以及直接运行的 `docker compose` 都会沿用。重复运行 `./deploy.sh` 是安全的：已有的配置和历史数据都会保留。

| 命令 | 作用 |
|---|---|
| `./deploy.sh status` | 运行状态、健康检查与访问地址 |
| `./deploy.sh logs` | 跟随查看日志。日志里不会出现密码、口令或 SQL 原文 |
| `./deploy.sh stop` / `start` | 停止 / 启动。停止不会删除历史数据 |
| `./deploy.sh restart` | 重启 |
| `./deploy.sh upgrade` | `git pull --ff-only` 后重新构建并启动 |
| `./deploy.sh backup [文件]` | 备份历史库。会暂停服务几秒，保证 SQLite 文件一致；备份里没有密码 |
| `./deploy.sh restore 文件` | 用备份覆盖当前历史库（会先确认） |
| `./deploy.sh uninstall` | 删除容器，保留历史数据与配置 |
| `./deploy.sh uninstall --purge` | 同时删除历史数据与配置，不可恢复（会先确认） |

需要确认的命令在非交互环境下要加 `--yes`。完整用法：`./deploy.sh help`。

### 从早期版本升级

早期版本用访问令牌登录，现在改为账号密码：升级后用 `admin` / `123456`（或 `.env` 里设置的 `DOCTOR_ADMIN_PASSWORD`）登录，旧令牌不再使用。存令牌的卷 `sr-doctor_doctor-secrets` 与 `token-init` 容器不再需要：脚本启动时会删掉 `token-init` 容器，令牌卷可以自行删除（`docker volume rm sr-doctor_doctor-secrets`）。

再早的版本 Compose 项目叫 `starrocks-doctor`，数据卷是 `starrocks-doctor_doctor-history`；更早的版本还把 `.env` 和 `compose.override.yaml` 放在 `deploy/linux/` 下。升级后第一次运行 `./deploy.sh` 时：

- 旧数据卷会复制到新卷 `sr-doctor_doctor-history`，历史数据保留，旧项目的容器会先停掉；旧卷不删除；
- `deploy/linux/.env` 会复制到根目录；
- 如果有 `deploy/linux/compose.override.yaml`，脚本会提示你把它移到根目录并检查其中的相对路径。

确认无误后可以删掉旧卷（`docker volume rm starrocks-doctor_doctor-history starrocks-doctor_doctor-secrets`）和 `deploy/linux/` 下的旧文件。不用脚本时，等价的做法是：

```bash
docker rm -f $(docker ps -aq --filter label=com.docker.compose.project=starrocks-doctor) 2>/dev/null || true
docker compose build
docker volume create --label com.docker.compose.project=sr-doctor --label com.docker.compose.volume=doctor-history sr-doctor_doctor-history
docker run --rm --network none --user 0 -v starrocks-doctor_doctor-history:/from:ro -v sr-doctor_doctor-history:/to \
  --entrypoint bash sr-doctor:local -c 'cp -a /from/. /to/ && chown -R 10001 /to'
cp -p deploy/linux/.env .env 2>/dev/null || true
docker compose up -d --remove-orphans
```

## 离线安装

服务器不能访问外网时，在一台能联网、架构相同的机器上打包：

```bash
./deploy.sh package
# ==> 离线安装包：dist/sr-doctor-<版本>-linux-amd64.tar.gz（约 45 MB）
```

把这个文件复制到服务器，然后：

```bash
tar xzf sr-doctor-<版本>-linux-amd64.tar.gz
cd sr-doctor-<版本>-linux-amd64
./deploy.sh
```

安装包里有部署脚本、`compose.yaml` 和镜像，服务器只需要 Docker Engine 与 Compose。

**升级**：解压新包，把旧目录里的 `.env`（如果有）复制过去，再在新目录运行脚本。历史数据保存在 Docker 卷里，不随目录变化：

```bash
tar xzf sr-doctor-<新版本>-linux-amd64.tar.gz
cp -p <旧目录>/.env sr-doctor-<新版本>-linux-amd64/ 2>/dev/null || true
cd sr-doctor-<新版本>-linux-amd64 && ./deploy.sh
```

## 放在 HTTPS 反向代理后面

使用者经 HTTPS 访问时，会话 Cookie 需要带 `Secure` 标记：

```bash
./deploy.sh --https --bind 127.0.0.1
# 或在 .env 里写 DOCTOR_COOKIE_SECURE=on、DOCTOR_BIND=127.0.0.1，再 docker compose up -d
```

`127.0.0.1` 让服务只接受本机代理的访问。Nginx 示例：

```nginx
server {
    listen 443 ssl;
    server_name doctor.example.com;
    ssl_certificate     /etc/nginx/tls/doctor.crt;
    ssl_certificate_key /etc/nginx/tls/doctor.key;
    client_max_body_size 40m;          # Profile 上传上限为 34 MiB
    location / {
        proxy_pass http://127.0.0.1:8000;
        proxy_set_header Host $host;   # 服务按 Host 校验同源
        proxy_read_timeout 300s;
    }
}
```

## 连接开启 SSL 的 StarRocks

连接表单里选 **TLS verify-full**：加密传输，并校验证书链与主机名。

- FE 使用公共 CA 签发的证书时，不需要额外配置。
- 使用私有 CA 时，在仓库根目录新建 `compose.override.yaml`，把 CA 证书挂进容器。这个文件已被 Git 忽略，Compose 和部署脚本都会自动加载，升级时也不会被覆盖：

  ```yaml
  services:
    doctor:
      environment:
        SSL_CERT_FILE: /etc/ssl/starrocks-ca.pem
      volumes:
        - ./ca.pem:/etc/ssl/starrocks-ca.pem:ro
  ```

  然后运行 `docker compose up -d`（或 `./deploy.sh`）重建容器。

## 本机进程

适合个人使用和开发调试。需要 Node.js 22 与 Rust 1.97 以上。在仓库根目录：

```bash
npm --prefix frontend ci
npm --prefix frontend run build
cargo build --release --locked -p doctor-api
DOCTOR_FRONTEND_DIST=frontend/dist ./target/release/doctor-api
```

Windows PowerShell 下把最后一行换成：

```powershell
$env:DOCTOR_FRONTEND_DIST = "frontend/dist"; .\target\release\doctor-api.exe
```

- 进程只监听 `127.0.0.1:8000`，不需要登录；端口用 `DOCTOR_PORT` 修改。
- 历史库默认写在用户数据目录：Windows 是 `%LOCALAPPDATA%\sr-doctor\history.sqlite`；Linux 是 `$XDG_DATA_HOME/sr-doctor/`，未设置时是 `~/.local/share/sr-doctor/`。
- 本机进程连接本机的 StarRocks（`127.0.0.1`）时，后台采集会自动运行。

## 常开主机 + SSH 隧道

想让采集不随笔记本休眠中断，又不想开放网络端口：把[本机进程](#本机进程)运行在一台常开主机上，再从自己的电脑建隧道：

```bash
ssh -N -L 8000:127.0.0.1:8000 user@always-on-host
```

然后在浏览器打开 `http://127.0.0.1:8000`。SSH 负责加密与认证；连接表单里填的 StarRocks 口令经隧道到达常开主机的进程，只保存在内存里。

## Windows 单机容器

只想在 Windows 上快速看一眼，并且不需要历史监控时，可以用 `deploy/local/compose.yaml`（回环模式，无需登录）：

```powershell
docker compose -f deploy/local/compose.yaml up -d --build
Start-Process "http://127.0.0.1:8000"
```

- 这个容器的文件系统只读，并关闭了历史，所以没有趋势、告警与治理；需要这些功能时，改用[本机进程](#本机进程)，或在 Linux 服务器上[一键部署](#docker-compose-一键部署推荐)。
- 在容器里连接 Windows 上的数据库时，主机填 `host.docker.internal`。
- 停止：`docker compose -f deploy/local/compose.yaml down`。

## 远端集群的后台采集

趋势、告警与治理的数据来自后台采集：用户连接 StarRocks 之后，服务按固定周期读取审计日志，只存脱敏模板与小时聚合。

- 连接 `127.0.0.1`/`::1` 上的 StarRocks 时，采集自动开启。
- 连接其他主机（远端集群）时，**默认只连接、不采集**。这是生产只读试点（G1）通过前的保护：采集对生产集群的负载影响还没有实测证据。
- G1 通过后，在根目录的 `.env` 里加一行 `DOCTOR_COLLECTION_ALLOW_REMOTE=after-g1`，再运行 `docker compose up -d`（或 `./deploy.sh`）重建容器。

采集的周期、时间片与回补时长见 [配置参考](configuration.md#历史与采集)。

## 数据都在哪里

| 数据 | 位置 | 内容 |
|---|---|---|
| 历史库 | Docker 卷 `sr-doctor_doctor-history`（本机进程见上文） | 脱敏 SQL 模板、指纹、小时聚合、告警与治理记录；导入的小时汇总、Routine Load 快照，以及导入错误日志的原因与原始行（明文，总量 50 MB）；告警通知的渠道、发送对象（密钥只存密文）与投递记录；默认保留 30 天。备份文件同样包含错误日志原文 |
| 通知主密钥 | 同一个卷里的 `secret.key`，或 `.env` 的 `DOCTOR_SECRET_KEY` | 解密通知密钥用；见 [配置参考](configuration.md#告警通知的主密钥) |
| 部署配置 | 根目录的 `.env`、`compose.override.yaml` | 端口、监听地址、登录密码等；备份里不含 |
| StarRocks 口令 | 只在服务进程内存里 | 断开、空闲 15 分钟或重启后删除 |

不会保存的：SQL 字面量、Profile 原文、StarRocks 口令、告警通知的消息正文。导入的 Profile 只在内存中分析。

服务主动连出的地址只有：用户连接的 StarRocks，以及用户在“通知渠道”里填写的邮件服务器、飞书/钉钉官方地址（`open.feishu.cn`、`open.larksuite.com`、`oapi.dingtalk.com`）和短信平台（`dysmsapi.aliyuncs.com`、`sms.tencentcloudapi.com`）。服务器需要能访问这些地址，告警通知才能送达。

## 边界

- 所有使用者共用一个登录账号，没有权限区分。能登录的人，都能用这个服务连接任何自己知道口令的 StarRocks。默认密码 `123456` 人人都知道：服务暴露在不完全可信的网络时，先改密码，并用防火墙限制可访问的网段。
- 历史、告警与治理队列在服务器上共享，所有使用者看到同一份数据。
- 明文连接时，口令与 SQL 在网络上不加密；跨不可信网络时请使用 TLS verify-full。
- 生产集群的连接安全与负载影响尚未经 G1 验证，不宣称 Production-safe。
