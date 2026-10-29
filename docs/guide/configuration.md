# 配置参考

所有配置都通过环境变量传给 `doctor-api`。值不合法时服务拒绝启动，并在控制台说明是哪一项出了问题。

## 部署脚本与 Compose

用根目录的 `compose.yaml`（[一键部署](deployment.md#docker-compose-一键部署推荐)）或 [`./deploy.sh`](deployment.md#部署脚本-deploysh) 部署时，只需要关心这几项。它们写在仓库根目录的 `.env` 里，`docker compose` 会自动读取；脚本的 `--port`、`--bind`、`--password`、`--https` 选项也会更新这个文件。

| 变量 | 默认 | 说明 |
|---|---|---|
| `DOCTOR_PORT` | `8000` | 服务器对外端口 |
| `DOCTOR_BIND` | `0.0.0.0` | 对外监听的地址。设为某个网卡地址，可只对该网段开放；放在反向代理后时设为 `127.0.0.1` |
| `DOCTOR_ADMIN_PASSWORD` | `123456` | 登录密码，见[下文](#登录账号网络模式) |
| `DOCTOR_COOKIE_SECURE` | `off` | 使用者经 HTTPS 访问时设为 `on`，会话 Cookie 会带 `Secure` 标记 |
| `DOCTOR_COLLECTION_ALLOW_REMOTE` | 空 | 远端集群的后台采集开关，见[下文](#历史与采集) |
| `DOCTOR_SECRET_KEY` | 空 | 告警通知密钥的主密钥，见[下文](#告警通知的主密钥) |

`.env` 示例：

```dotenv
DOCTOR_PORT=18000
DOCTOR_BIND=10.0.0.5
DOCTOR_ADMIN_PASSWORD=change-me-2026
DOCTOR_COOKIE_SECURE=off
```

改完运行 `docker compose up -d`（或 `./deploy.sh`）生效：配置有变化时会重建容器，历史数据不受影响。`docker compose restart` 和 `./deploy.sh restart` 只重启进程，不会读取新配置。

需要挂载文件或增加环境变量时，写在根目录的 `compose.override.yaml` 里，不要改 `compose.yaml`：前者已被 Git 忽略，Compose 和脚本都会自动加载，升级时也不会被覆盖。

## 运行模式与监听

| 变量 | 默认 | 说明 |
|---|---|---|
| `DOCTOR_DEPLOYMENT_MODE` | 未设置 | 未设置：回环模式，只监听 `127.0.0.1`，不需要登录。`network`：网络模式，需要登录。`docker-loopback-published`：仅供 `deploy/local/compose.yaml` 使用 |
| `DOCTOR_PORT` | `8000` | 回环模式的监听端口；网络模式下未设置 `DOCTOR_LISTEN` 时也用它 |
| `DOCTOR_LISTEN` | `0.0.0.0:<DOCTOR_PORT>` | 仅网络模式：监听的地址与端口，例如 `0.0.0.0:8000` |
| `DOCTOR_PUBLIC_PORT` | `8000` | 仅 `docker-loopback-published`：宿主机上发布的端口，用于校验 Host 与 Origin |
| `DOCTOR_FRONTEND_DIST` | `frontend/dist` | 前端构建目录。镜像里是 `/app/frontend` |

## 登录账号（网络模式）

| 变量 | 默认 | 说明 |
|---|---|---|
| `DOCTOR_ADMIN_USER` | `admin` | 登录用户名 |
| `DOCTOR_ADMIN_PASSWORD` | `123456` | 登录密码。首尾空白会被去掉；留空等于默认值 |
| `DOCTOR_COOKIE_SECURE` | `off` | `on` 时会话 Cookie 带 `Secure` 标记 |

- 所有使用者共用这一个账号，没有权限区分。
- 默认密码写在公开的文档里，人人都知道。服务暴露在不完全可信的网络时，先在 `.env` 里改密码，或运行 `./deploy.sh --password 新密码`。仍在用默认密码时，服务启动会在日志里提醒一次（不打印密码）。
- 登录成功后发放 HttpOnly、SameSite=Strict 的会话 Cookie，有效期 12 小时；同时最多保留 256 个会话。
- 同一来源地址 60 秒内输错 5 次后，该地址的登录会被暂时拒绝，直到窗口过去；其他地址不受影响，已登录的会话也不受影响。经反向代理访问时，服务看到的都是代理的地址，所有人共用这一个计数。
- 改密码后运行 `docker compose up -d` 重建容器，所有已登录的浏览器都需要重新登录。

## 历史与采集

| 变量 | 默认 | 范围 | 说明 |
|---|---|---|---|
| `DOCTOR_HISTORY` | 开启 | `off` | 设为 `off` 关闭历史库，趋势、告警与治理不可用 |
| `DOCTOR_HISTORY_PATH` | 用户数据目录 | | 历史库文件路径。默认：Windows `%LOCALAPPDATA%\sr-doctor\history.sqlite`；Linux `$XDG_DATA_HOME` 或 `~/.local/share` 下的 `sr-doctor/history.sqlite`；容器里是 `/var/lib/sr-doctor/history.sqlite` |
| `DOCTOR_COLLECTION_PERIOD_SECONDS` | `300` | 5–3600 | 两次读取审计日志的间隔 |
| `DOCTOR_COLLECTION_SLICE_SECONDS` | `300` | 60–3600 | 每次读取的时间片长度 |
| `DOCTOR_COLLECTION_LAG_SECONDS` | `120` | 5–3600 | 时间片结束后再等这么久才读。更晚才写入审计表的行不会被读到，这是已知限制 |
| `DOCTOR_COLLECTION_BACKFILL_SECONDS` | `3600` | 0–86400 | 首次连接时回补多长时间 |
| `DOCTOR_COLLECTION_ALLOW_REMOTE` | 未设置 | 只接受 `after-g1` | 未设置时，远端集群（非 `127.0.0.1`/`::1`）只连接、不采集。**G1 生产只读试点通过前不要设置** |

固定的采集上限，不可配置：

- 每次读取最多 1000 行，单条语句超时 10 秒，只用一条连接；
- 读满 1000 行的时间片标为“部分”，不加大读取量；
- 读取失败按周期做指数退避，最长 30 分钟；
- 断开不超过 24 小时，重新连接后补读；更早的部分记为“未采集”，不插值；
- 历史数据保留 30 天，过期自动删除。

## 告警通知的主密钥

邮件口令、飞书/钉钉机器人的 Webhook 与签名密钥、短信平台的 AccessKey Secret 在界面里填写，用 AES-256-GCM 加密后存进历史库；界面和接口只显示“已保存”，不回显。加密用的主密钥：

| 变量 | 默认 | 说明 |
|---|---|---|
| `DOCTOR_SECRET_KEY` | 未设置 | 64 位十六进制字符（32 字节），例如 `openssl rand -hex 32` 的输出。格式不对时服务拒绝启动 |

- 未设置时，首次启动在历史库同目录生成 `secret.key`（Linux 上权限 600），之后一直使用它。容器部署时它和历史库在同一个 Docker 卷里，`./deploy.sh backup` 会把两者一起备份。
- 想让数据库备份单独泄露时也读不出密钥，就把主密钥写进 `.env` 的 `DOCTOR_SECRET_KEY`，并把 `.env` 与备份分开保管。
- 主密钥更换或丢失后，已保存的密钥无法解密：界面会标“需重新填写”，对应渠道在重新填写前不会发送。
- 加密只防止数据库文件被单独复制或备份时泄露密钥，不防主机本身被攻破。
- 历史库关闭（`DOCTOR_HISTORY=off`）或主密钥不可用时，告警通知不可用。

## 连接 StarRocks 的证书

选择 TLS verify-full 时，使用系统信任的 CA 校验 FE 证书。私有 CA 用 `SSL_CERT_FILE` 指向 PEM 文件，容器部署的写法见 [部署指南](deployment.md#连接开启-ssl-的-starrocks)。

## 固定行为

以下行为是产品边界，不提供开关：

- 连接只读：不执行 DDL/DML、多语句或 `EXPLAIN ANALYZE`；“运行时 Top”只接受一条 `SELECT`/`WITH`，超时 10 秒，丢弃结果行。
- 连接会话空闲 15 分钟后过期，口令随之删除。
- 上传的 Profile 不超过 34 MiB。
- 不向外发送任何遥测，不把 SQL、Profile 或口令写入日志。告警通知只发往用户在“通知渠道”里填写的邮件服务器、飞书/钉钉官方地址与短信平台。
- 告警通知：每次发送超时 10 秒；网络类失败按 1、5、30 分钟重试，共 4 次；同一时间最多排队 500 条；投递记录保留 30 天。
