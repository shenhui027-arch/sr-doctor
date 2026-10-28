#!/usr/bin/env bash
# SR Doctor 一键部署与运维脚本（Linux 服务器，Docker Compose）。
#
#   ./deploy.sh                 # 安装或更新并启动，打印访问地址与登录账号
#   ./deploy.sh help            # 全部命令与选项
#
# 包装的是根目录的 compose.yaml；只想用 Compose 时，直接 docker compose up -d --build 即可。
# 文档：docs/guide/deployment.md
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$ROOT/.env"
# 旧版本的部署目录：.env 与 compose.override.yaml 在首次安装时迁移过来。
LEGACY_DIR="$ROOT/deploy/linux"
IMAGE="sr-doctor:local"
IMAGE_ARCHIVE="sr-doctor-image.tar.gz"
SERVICE="doctor"
VOLUME="sr-doctor_doctor-history"
# 早期版本存访问令牌的卷；现在改用账号密码，只在 uninstall --purge 时顺带删除。
OLD_SECRETS_VOLUME="sr-doctor_doctor-secrets"
HEALTH_TIMEOUT="${DOCTOR_HEALTH_TIMEOUT:-180}"
# docker 只收相对路径；Git Bash 下不要改写 `卷:/路径` 这类参数。
export MSYS_NO_PATHCONV=1

if [ -t 1 ]; then BOLD=$'\033[1m'; BLUE=$'\033[1;34m'; YELLOW=$'\033[1;33m'; RED=$'\033[1;31m'; RESET=$'\033[0m'
else BOLD=""; BLUE=""; YELLOW=""; RED=""; RESET=""; fi
info() { printf '%s==>%s %s\n' "$BLUE" "$RESET" "$*"; }
warn() { printf '%s警告：%s%s\n' "$YELLOW" "$*" "$RESET" >&2; }
die()  { printf '%s错误：%s%s\n' "$RED" "$*" "$RESET" >&2; exit 1; }

usage() {
  cat <<'EOF'
SR Doctor 部署脚本

用法：./deploy.sh [命令] [选项]

命令：
  install         安装或更新并启动（默认命令）
  status          查看运行状态与访问地址
  logs            跟随查看服务日志（日志里没有密码、口令或 SQL 原文）
  start | stop    启动 / 停止服务（停止不删除历史数据）
  restart         重启服务
  upgrade         拉取最新代码（git pull --ff-only）后重新构建并启动
  backup [文件]   备份历史库（默认 ./sr-doctor-history-<时间>.tgz）
  restore 文件    用备份覆盖当前历史库
  package         构建镜像并打成离线安装包（服务器无法访问外网时用）
  uninstall       删除容器；加 --purge 同时删除历史数据与配置
  help            显示本帮助

选项：
  --port 端口       对外端口（默认 8000）
  --bind 地址       只在这个地址上监听（默认 0.0.0.0，即所有网卡）
  --password 密码   登录密码（默认账号 admin / 123456）；可用字母、数字和 ._@%+=:,/~!*^-
  --https           使用者经 HTTPS 反向代理访问时使用（会话 Cookie 加 Secure）
  --http            取消 --https
  --image 文件      从离线镜像包加载，不在本机构建
  --output 文件     package 的输出路径
  --purge           与 uninstall 一起用：同时删除历史数据与配置
  -y, --yes         不询问，直接确认

--port、--bind、--password、--https 会写入根目录的 .env，之后的命令（以及直接运行的
docker compose）沿用。改密码：./deploy.sh --password 新密码
EOF
}

# ---- 参数 ----
COMMAND=""
ARGS=()
OPT_PORT="" OPT_BIND="" OPT_PASSWORD="" OPT_COOKIE="" OPT_IMAGE="" OPT_OUTPUT="" OPT_PURGE=0 OPT_YES=0
while [ $# -gt 0 ]; do
  case "$1" in
    --port) [ $# -ge 2 ] || die "--port 需要一个值"; OPT_PORT="$2"; shift 2 ;;
    --bind) [ $# -ge 2 ] || die "--bind 需要一个值"; OPT_BIND="$2"; shift 2 ;;
    --password) [ $# -ge 2 ] || die "--password 需要一个值"; OPT_PASSWORD="$2"; shift 2 ;;
    --https) OPT_COOKIE="on"; shift ;;
    --http) OPT_COOKIE="off"; shift ;;
    --image) [ $# -ge 2 ] || die "--image 需要一个文件"; OPT_IMAGE="$2"; shift 2 ;;
    --output) [ $# -ge 2 ] || die "--output 需要一个文件"; OPT_OUTPUT="$2"; shift 2 ;;
    --purge) OPT_PURGE=1; shift ;;
    -y|--yes) OPT_YES=1; shift ;;
    -h|--help) COMMAND="help"; shift ;;
    -*) die "未知选项：$1（./deploy.sh help 查看用法）" ;;
    *) if [ -z "$COMMAND" ]; then COMMAND="$1"; else ARGS+=("$1"); fi; shift ;;
  esac
done
COMMAND="${COMMAND:-install}"

if [ -n "$OPT_PORT" ]; then
  case "$OPT_PORT" in ''|*[!0-9]*) die "--port 必须是 1～65535 的数字" ;; esac
  [ "$OPT_PORT" -ge 1 ] && [ "$OPT_PORT" -le 65535 ] || die "--port 必须是 1～65535 的数字"
fi
if [ -n "$OPT_BIND" ]; then
  case "$OPT_BIND" in *[!0-9A-Fa-f.:]*) die "--bind 必须是一个 IP 地址" ;; esac
fi
# 密码写进 .env，Compose 会解析其中的引号、空格、# 与 $，所以只收不需要转义的字符。
if [ -n "$OPT_PASSWORD" ]; then
  case "$OPT_PASSWORD" in *[!A-Za-z0-9._@%+=:,/~!*^-]*) die "--password 只能用字母、数字和 ._@%+=:,/~!*^-" ;; esac
fi

# ---- 工具 ----
# compose.override.yaml 放本地定制（私有 CA 等），升级时不会被覆盖。
COMPOSE=(docker compose)
compose() {
  (cd "$ROOT" && if [ -f compose.override.yaml ]; then
    "${COMPOSE[@]}" -f compose.yaml -f compose.override.yaml "$@"
  else
    "${COMPOSE[@]}" -f compose.yaml "$@"
  fi)
}

need_docker() {
  command -v docker >/dev/null 2>&1 || die "没有找到 docker。安装方法：https://docs.docker.com/engine/install/"
  docker info >/dev/null 2>&1 || die "无法访问 Docker。请确认 Docker 已启动；非 root 用户需要加入 docker 组，或用 sudo 运行本脚本。"
  if docker compose version >/dev/null 2>&1; then COMPOSE=(docker compose)
  elif command -v docker-compose >/dev/null 2>&1; then COMPOSE=(docker-compose)
  else die "需要 Docker Compose（docker compose 插件，或 1.29 以上的 docker-compose）。安装方法：https://docs.docker.com/compose/install/linux/"
  fi
  [ -f "$ROOT/compose.yaml" ] || die "缺少 compose.yaml，请在仓库或离线包的根目录运行本脚本。"
}

confirm() {
  [ "$OPT_YES" = 1 ] && return 0
  [ -t 0 ] || die "$1 需要确认：交互运行，或加 --yes。"
  local answer
  read -r -p "$1 [y/N] " answer
  case "$answer" in y|Y|yes|YES) return 0 ;; *) die "已取消。" ;; esac
}

env_get() {
  local key="$1" default="$2" value=""
  [ -f "$ENV_FILE" ] && value="$(sed -n "s/^${key}=//p" "$ENV_FILE" | tail -n 1)"
  printf '%s' "${value:-$default}"
}

env_set() {
  local key="$1" value="$2" tmp
  tmp="$(mktemp "$ROOT/.env.XXXXXX")"
  [ -f "$ENV_FILE" ] && grep -v "^${key}=" "$ENV_FILE" > "$tmp" || true
  printf '%s=%s\n' "$key" "$value" >> "$tmp"
  mv "$tmp" "$ENV_FILE"
}

apply_options() {
  [ -n "$OPT_PORT" ] && env_set DOCTOR_PORT "$OPT_PORT"
  [ -n "$OPT_BIND" ] && env_set DOCTOR_BIND "$OPT_BIND"
  [ -n "$OPT_PASSWORD" ] && env_set DOCTOR_ADMIN_PASSWORD "$OPT_PASSWORD"
  [ -n "$OPT_COOKIE" ] && env_set DOCTOR_COOKIE_SECURE "$OPT_COOKIE"
  return 0
}

# 早期版本的 Compose 项目叫 starrocks-doctor。新卷还不存在而旧卷存在时，停掉旧项目的
# 容器（数据都在卷里），把旧卷复制到新卷；旧卷保留，确认无误后可自行删除。
LEGACY_PROJECT="starrocks-doctor"
migrate_volumes() {
  local old_containers old="${LEGACY_PROJECT}_doctor-history"
  docker volume inspect "$old" >/dev/null 2>&1 || return 0
  docker volume inspect "$VOLUME" >/dev/null 2>&1 && return 0
  old_containers="$(docker ps -aq --filter "label=com.docker.compose.project=$LEGACY_PROJECT")"
  if [ -n "$old_containers" ]; then
    info "停止早期版本的容器…"
    # shellcheck disable=SC2086
    docker rm -f $old_containers >/dev/null
  fi
  info "把数据卷 $old 复制到 $VOLUME（旧卷保留）…"
  docker volume create --label com.docker.compose.project=sr-doctor \
    --label "com.docker.compose.volume=doctor-history" "$VOLUME" >/dev/null
  docker run --rm --network none --user 0 -v "$old:/from:ro" -v "$VOLUME:/to" --entrypoint bash "$IMAGE" \
    -c 'cp -a /from/. /to/ && chown -R 10001 /to' \
    || die "复制数据卷 $old 失败。"
}

# 旧版本把 .env 与 compose.override.yaml 放在 deploy/linux。首次安装时把 .env 复制到
# 根目录；旧文件不删除，确认无误后可自行删掉。
migrate_legacy() {
  if [ -f "$LEGACY_DIR/.env" ] && [ ! -f "$ENV_FILE" ]; then
    cp -p "$LEGACY_DIR/.env" "$ENV_FILE"
    info "已把 deploy/linux/.env 复制到根目录的 .env。"
  fi
  if [ -f "$LEGACY_DIR/compose.override.yaml" ] && [ ! -f "$ROOT/compose.override.yaml" ]; then
    warn "发现 deploy/linux/compose.override.yaml。请把它移到根目录并检查其中的相对路径，然后重新运行。"
  fi
  return 0
}

container_id() { compose ps -q "$SERVICE" 2>/dev/null | head -n 1; }

# 在容器里直接问健康接口，不必等 compose 的 30 秒健康检查周期。
wait_ready() {
  local cid state waited=0
  info "等待服务就绪（最多 ${HEALTH_TIMEOUT} 秒）…"
  while [ "$waited" -lt "$HEALTH_TIMEOUT" ]; do
    cid="$(container_id)"
    if [ -n "$cid" ]; then
      state="$(docker inspect -f '{{.State.Status}}' "$cid" 2>/dev/null || true)"
      case "$state" in
        exited|dead) compose logs --tail 30 "$SERVICE" >&2 || true; die "服务启动失败，日志见上方。" ;;
      esac
      if docker exec "$cid" bash -c "exec 3<>/dev/tcp/127.0.0.1/8000 && printf 'GET /api/v1/health HTTP/1.0\r\nHost: 127.0.0.1:8000\r\n\r\n' >&3 && grep -q '\"ok\"' <&3" >/dev/null 2>&1; then
        return 0
      fi
    fi
    sleep 2
    waited=$((waited + 2))
  done
  compose logs --tail 30 "$SERVICE" >&2 || true
  die "服务在 ${HEALTH_TIMEOUT} 秒内没有就绪，日志见上方。"
}

server_address() {
  local bind
  bind="$(env_get DOCTOR_BIND 0.0.0.0)"
  if [ "$bind" != "0.0.0.0" ] && [ "$bind" != "::" ]; then printf '%s' "$bind"; return; fi
  local address=""
  command -v hostname >/dev/null 2>&1 && address="$(hostname -I 2>/dev/null | awk '{print $1}')" || true
  if [ -z "$address" ] && command -v ip >/dev/null 2>&1; then
    address="$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") print $(i + 1)}' | head -n 1)"
  fi
  printf '%s' "${address:-<服务器地址>}"
}

print_access() {
  local port url user
  port="$(env_get DOCTOR_PORT 8000)"
  if [ "$(env_get DOCTOR_COOKIE_SECURE off)" = "on" ]; then
    url="https://<你的 HTTPS 域名>（反向代理到本机 $port 端口）"
  else
    url="http://$(server_address):$port"
  fi
  printf '\n%sSR Doctor 已就绪%s\n' "$BOLD" "$RESET"
  printf '  访问地址：%s\n' "$url"
  if [ "$(env_get DOCTOR_COOKIE_SECURE off)" != "on" ]; then
    case "$(env_get DOCTOR_BIND 0.0.0.0)" in 0.0.0.0|127.0.0.1) printf '  本机访问：http://127.0.0.1:%s\n' "$port" ;; esac
  fi
  user="$(env_get DOCTOR_ADMIN_USER admin)"
  if [ -z "$(env_get DOCTOR_ADMIN_PASSWORD "")" ]; then
    printf '  登录账号：%s / 123456（默认密码；改密码：./deploy.sh --password 新密码）\n' "$user"
  else
    printf '  登录账号：%s（密码已在 .env 中设置）\n' "$user"
  fi
  printf '\n  浏览器打开地址 → 登录 → 点“连接”，填写 StarRocks FE 的 IP、端口（默认 9030）、用户名、口令。\n'
  printf '  常用命令：./deploy.sh status | logs | upgrade | backup | stop\n\n'
}

# 镜像来源：--image 指定的离线包 > 本目录下的离线包（且没有源码） > 本机构建。
prepare_image() {
  local archive="$OPT_IMAGE"
  if [ -z "$archive" ] && [ ! -f "$ROOT/Dockerfile" ] && [ -f "$ROOT/$IMAGE_ARCHIVE" ]; then
    archive="$ROOT/$IMAGE_ARCHIVE"
  fi
  if [ -n "$archive" ]; then
    [ -f "$archive" ] || die "找不到镜像包：$archive"
    info "加载离线镜像 $(basename "$archive") …"
    docker load < "$archive" >/dev/null
    docker image inspect "$IMAGE" >/dev/null 2>&1 || die "镜像包里没有 $IMAGE。"
    UP_ARGS=(--no-build)
  elif [ -f "$ROOT/Dockerfile" ]; then
    info "构建镜像（首次需要十几分钟，之后会复用缓存）…"
    compose build
    UP_ARGS=(--no-build)
  elif docker image inspect "$IMAGE" >/dev/null 2>&1; then
    UP_ARGS=(--no-build)
  else
    die "既没有源码也没有离线镜像包。请在仓库根目录运行，或用 --image 指定镜像包。"
  fi
}

# ---- 命令 ----
# --remove-orphans 顺带删掉早期版本生成令牌的 token-init 容器。
cmd_install() {
  need_docker
  apply_options
  prepare_image
  migrate_volumes
  migrate_legacy
  info "启动服务…"
  compose up -d --remove-orphans "${UP_ARGS[@]}"
  wait_ready
  print_access
}

cmd_status() {
  need_docker
  compose ps
  if [ -n "$(container_id)" ]; then
    printf '\n访问地址：http://%s:%s\n' "$(server_address)" "$(env_get DOCTOR_PORT 8000)"
  fi
}

cmd_logs() { need_docker; compose logs -f --tail 200 "$SERVICE"; }

cmd_start() {
  need_docker
  apply_options
  docker image inspect "$IMAGE" >/dev/null 2>&1 || die "还没有镜像，请先运行 ./deploy.sh install。"
  migrate_volumes
  migrate_legacy
  compose up -d --remove-orphans --no-build
  wait_ready
  print_access
}

cmd_stop() { need_docker; compose down; info "已停止。历史数据保留在卷 $VOLUME 中。"; }

cmd_restart() { need_docker; compose restart "$SERVICE"; wait_ready; info "已重启。"; }

cmd_upgrade() {
  if [ -d "$ROOT/.git" ] && command -v git >/dev/null 2>&1; then
    info "拉取最新代码…"
    (cd "$ROOT" && git pull --ff-only) || die "git pull 失败；请先处理本地修改或分叉。"
  elif [ -z "$OPT_IMAGE" ]; then
    warn "这里不是 Git 仓库：使用当前文件升级。离线升级请用 --image 指定新的镜像包。"
  fi
  cmd_install
}

volume_exists() { docker volume inspect "$VOLUME" >/dev/null 2>&1; }

# 备份与恢复时短暂停止服务，保证 SQLite 文件一致。
with_service_paused() {
  local cid running=0
  cid="$(container_id)"
  if [ -n "$cid" ] && [ "$(docker inspect -f '{{.State.Running}}' "$cid")" = "true" ]; then
    running=1
    info "暂停服务…"
    compose stop "$SERVICE" >/dev/null
  fi
  local status=0
  "$@" || status=$?
  if [ "$running" = 1 ]; then
    compose start "$SERVICE" >/dev/null
    wait_ready
  fi
  return "$status"
}

cmd_backup() {
  need_docker
  volume_exists || die "还没有历史数据卷 $VOLUME。"
  docker image inspect "$IMAGE" >/dev/null 2>&1 || die "缺少镜像 $IMAGE。"
  local file="${ARGS[0]:-sr-doctor-history-$(date +%Y%m%d-%H%M%S).tgz}"
  [ -e "$file" ] && die "$file 已存在。"
  backup_to() { docker run --rm --network none -v "$VOLUME:/data:ro" --entrypoint tar "$IMAGE" czf - -C /data . > "$file"; }
  with_service_paused backup_to || { rm -f "$file"; die "备份失败。"; }
  info "已备份到 $file（$(du -h "$file" | cut -f1)）。里面只有脱敏模板、指纹与小时聚合，不含 SQL 原文或口令。"
}

cmd_restore() {
  need_docker
  local file="${ARGS[0]:-}"
  [ -n "$file" ] || die "用法：./deploy.sh restore <备份文件>"
  [ -f "$file" ] || die "找不到 $file"
  docker image inspect "$IMAGE" >/dev/null 2>&1 || die "缺少镜像 $IMAGE，请先运行 ./deploy.sh install。"
  tar tzf "$file" >/dev/null 2>&1 || die "$file 不是有效的备份文件。"
  confirm "将用 $file 覆盖当前历史数据。继续？"
  volume_exists || docker volume create "$VOLUME" >/dev/null
  restore_from() {
    docker run --rm -i --network none -v "$VOLUME:/data" --user 0 --entrypoint bash "$IMAGE" \
      -c 'find /data -mindepth 1 -delete && tar xzf - -C /data && chown -R 10001 /data' < "$file"
  }
  with_service_paused restore_from || die "恢复失败。"
  info "已从 $file 恢复历史数据。"
}

cmd_package() {
  need_docker
  [ -f "$ROOT/Dockerfile" ] || die "package 需要在源码仓库里运行。"
  local version arch name output
  version="$(cd "$ROOT" && git describe --tags --always --dirty 2>/dev/null || echo local)"
  info "构建镜像…"
  compose build
  arch="$(docker image inspect -f '{{.Architecture}}' "$IMAGE")"
  name="sr-doctor-$version-linux-$arch"
  output="${OPT_OUTPUT:-dist/$name.tar.gz}"
  [ -e "$output" ] && die "$output 已存在。"
  mkdir -p "$(dirname "$output")"
  PACKAGE_STAGE="$(mktemp -d "${TMPDIR:-/tmp}/sr-doctor-package.XXXXXX")"
  trap 'rm -rf "$PACKAGE_STAGE"' EXIT
  local stage="$PACKAGE_STAGE"
  mkdir -p "$stage/$name"
  cp "$ROOT/deploy.sh" "$ROOT/compose.yaml" "$ROOT/LICENSE" "$ROOT/NOTICE" "$stage/$name/"
  info "导出镜像…"
  docker save "$IMAGE" | gzip > "$stage/$name/$IMAGE_ARCHIVE"
  cat > "$stage/$name/README.txt" <<EOF
SR Doctor 离线安装包（$version，linux/$arch）

把整个压缩包复制到服务器后：

  tar xzf $name.tar.gz
  cd $name
  ./deploy.sh

服务器只需要 Docker Engine 与 Compose（docker compose 插件或 1.29 以上的
docker-compose），不需要访问外网。默认登录账号 admin / 123456，
改密码：./deploy.sh --password 新密码
常用命令：./deploy.sh status | logs | backup | stop
EOF
  tar czf "$output" -C "$stage" "$name"
  info "离线安装包：$output（$(du -h "$output" | cut -f1)）"
}

cmd_uninstall() {
  need_docker
  if [ "$OPT_PURGE" = 1 ]; then
    confirm "将删除容器、历史数据卷 $VOLUME 与 .env 配置，不可恢复。继续？"
    compose down -v --remove-orphans
    docker volume rm "$OLD_SECRETS_VOLUME" >/dev/null 2>&1 || true
    rm -f "$ENV_FILE"
    info "已全部删除。镜像 $IMAGE 保留，可用 docker image rm $IMAGE 删除。"
  else
    compose down --remove-orphans
    info "已删除容器。历史数据与配置保留；加 --purge 可一并删除。"
  fi
}

case "$COMMAND" in
  install) cmd_install ;;
  status) cmd_status ;;
  logs) cmd_logs ;;
  start) cmd_start ;;
  stop) cmd_stop ;;
  restart) cmd_restart ;;
  upgrade) cmd_upgrade ;;
  backup) cmd_backup ;;
  restore) cmd_restore ;;
  package) cmd_package ;;
  uninstall) cmd_uninstall ;;
  help) usage ;;
  *) die "未知命令：$COMMAND（./deploy.sh help 查看用法）" ;;
esac
