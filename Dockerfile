# syntax=docker/dockerfile:1

FROM node:22-bookworm-slim AS frontend-build
WORKDIR /src
COPY frontend/package.json frontend/package-lock.json ./frontend/
COPY contracts/ ./contracts/
COPY fixtures/ ./fixtures/
COPY scripts/release/verify-frontend-bundles.mjs ./scripts/release/verify-frontend-bundles.mjs
# Type-checked tests read the version pack manifest.
COPY backend/crates/version-packs/starrocks_3_5_14/version-pack.json ./backend/crates/version-packs/starrocks_3_5_14/version-pack.json
WORKDIR /src/frontend
# The lock file already pins the Linux rollup/esbuild binaries, so npm ci
# installs them; a separate `npm install` of rollup broke on newer npm.
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1.97.1-bookworm AS api-build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY backend/ ./backend/
COPY contracts/ ./contracts/
COPY fixtures/ ./fixtures/
RUN cargo build --locked --release -p doctor-api --bin doctor-api

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --no-install-recommends --yes ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --home-dir /nonexistent --shell /usr/sbin/nologin --uid 10001 doctor
# The history and access-token volumes of the server deployment (compose.yaml)
# mount here; a new named volume copies this owner and mode, so the non-root
# user can write to them and nobody else can read the token.
RUN mkdir -p /var/lib/sr-doctor /etc/sr-doctor \
    && chown doctor /var/lib/sr-doctor /etc/sr-doctor \
    && chmod 700 /etc/sr-doctor
WORKDIR /app
COPY --from=api-build /src/target/release/doctor-api /app/doctor-api
COPY --from=frontend-build /src/frontend/dist /app/frontend
COPY LICENSE NOTICE /app/
ENV DOCTOR_DEPLOYMENT_MODE=docker-loopback-published \
    DOCTOR_FRONTEND_DIST=/app/frontend
USER doctor
EXPOSE 8000
ENTRYPOINT ["/app/doctor-api"]
