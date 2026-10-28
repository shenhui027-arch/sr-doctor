# StarRocks 3.5.14 isolated E2E fixture

This directory is test infrastructure, not a product deployment path. The product only creates a short-lived, read-only `doctor_reader` connection after this fixture already exists.

## Pinned inputs

- Image: `starrocks/allin1-ubuntu:3.5.14@sha256:5e1fa9c1a490f49457b8220b10ef2ea211af271b2626654ba151429f4603715f`
- Expected server: `3.5.14-23a56ec`
- AuditLoader: see [`auditloader.manifest.json`](./auditloader.manifest.json). The archive is downloaded into the fixture data root (`<SR_DOCTOR_WORK_ROOT>che`) and its SHA-256 is verified before installation.
- The fixture writes the dedicated AuditLoader password only into its short-lived configured plugin archive inside the isolated container; `secret_key` is explicitly blank so the fixture passes the plaintext password required by the plugin. No product or repository file receives that credential.
- Fixture account statements use StarRocks native-password ciphertext derived locally from the process token, so FE audit statements never contain the plaintext fixture password. AuditLoader still receives its writer password only inside its short-lived configured archive.
- **Docker-only compatibility exception:** official guidance recommends the FE HTTP port `127.0.0.1:8030`. In this fixed, single-node allin1 image, that endpoint redirects Stream Load to `127.0.0.1:8040`; AuditLoader 5.0.0 loses Basic authentication on this redirect (`8030` first response `307`, same credential with a trusted redirect reaches `200`). The fixture writes `frontend_host_port=127.0.0.1:8040` only to avoid that observed redirect. It is not a production setting or an override of the official recommendation.

## Isolation contract

- Default container: `sr-doctor-v0.1-e2e-3514`, labelled `com.sr-doctor.e2e=3.5.14`.
- Ports bind only to `127.0.0.1`: `38130:8030`, `38140:8040`, `39120:9020`, `39130:9030`.
- Persistent FE metadata and BE storage live below `<SR_DOCTOR_CONTAINER_ROOT>\sr-doctor-v0.1-e2e-3514`; no source repository receives runtime data, Profile bytes or credentials.
- Bootstrap limits the dedicated container to four CPUs, 6 GiB memory and 1024 PIDs. It never stops, prunes or alters a container without the fixture label.

## Roles

| Role | Fixture responsibility | Product use |
|---|---|---|
| `root` | bootstrap administrator inside the isolated container | never supplied to product |
| `audit_writer` | AuditLoader stream-load writer | never supplied to product |
| `doctor_reader` | `SELECT` audit table; Profile/Advisor/Explain read paths | only credential used by E2E product flow |
| `workload_user` | creates the two fixture Profiles outside the product | never supplied to product |

Set a process-only token before bootstrap. It is used to derive fixture passwords, is never written to the repository or printed, and must be reused when restarting the same persistent fixture:

```powershell
$env:STARROCKS_E2E_SECRET = '<24-64 character local test token>'
.\dev\starrocks-3.5.14\bootstrap.ps1
.\dev\starrocks-3.5.14\run-fixture-workload.ps1
```

`bootstrap.ps1 -LowResource` (also `run-browser-e2e.ps1 -LowResource` and `run-tls-verify-full-e2e.ps1 -LowResource`) creates the container with 2.5 CPUs and 5.25 GiB, `--memory-swap` equal to the limit, and before the first start writes an FE heap of 1024 MiB (`JAVA_OPTS`, overriding the image default `-Xmx8192m`) and a BE `mem_limit` of 3072 MiB. Without the switch the original 4 CPU / 6 GB sizing is kept. The switch applies only when the container is created.

The workload runner deliberately waits 65 seconds after plugin installation. In
3.5.14, the FE audit-plugin registry refreshes on a 60-second cadence; only
after that refresh does it issue the two fixture queries. The installed fixture
sets AuditLoader `max_batch_interval_sec=5` and verifies publication with a
bounded 30-second audit-table poll. This is fixture timing evidence, not
product polling behavior.

`run-browser-e2e.ps1 -Project connected-readonly-docker` performs those steps, captures a real Profile under `<SR_DOCTOR_WORK_ROOT>\e2e`, runs the HTTP smoke, and runs the Chrome browser flow:

```powershell
.\scripts\e2e\run-browser-e2e.ps1 -Project connected-readonly-docker
```

That connected command calls `teardown.ps1` in `finally` by default. Use
`-KeepFixture` only while diagnosing a failing local run, then run
`teardown.ps1` manually; a retained fixture is not release evidence.

The browser runner retains only its safe screenshots and JSON receipt beneath
`<SR_DOCTOR_WORK_ROOT>\e2e\browser`. To remove one known run
after review, pass its absolute receipt directory to the guarded cleanup script;
it refuses every path outside that work root:

```powershell
.\scripts\e2e\clear-runtime-artifacts.ps1 -RunDirectory '<SR_DOCTOR_WORK_ROOT>\e2e\browser\offline-local-YYYY-MM-DDTHH-MM-SS-Z-#####'
```

The resulting evidence proves only this local Docker fixture. Production-safe behavior, production TLS, topology, authorization and load impact remain `INCONCLUSIVE`.

## TLS variant (v0.7-01)

[`tls-common.ps1`](./tls-common.ps1) starts the same pinned image as
`sr-doctor-v0.1-e2e-3514-tls` with the official 3.5.14 MySQL-protocol SSL
settings (`ssl_keystore_location`, `ssl_keystore_password`, `ssl_key_password`,
`ssl_force_secure_transport = true`). Each run generates a one-off CA, a valid
`DNS:localhost` server certificate and an expired one in a temporary directory,
copies the two keystores into the created container before the FE starts, and
deletes the host copies at once. 3.5.14 `MysqlProto.negotiate()` exempts
clients it sees as `127.0.0.1` from `ssl_force_secure_transport`, so in-container
fixture clients may stay plaintext; host connections through Docker's port
forwarding are not exempt. Only the MySQL port is published
(`127.0.0.1:39183` by default); there is no AuditLoader. The runner drives the product
through three owned API processes that differ only in `SSL_CERT_FILE`:

```powershell
.\scripts\e2e\run-tls-verify-full-e2e.ps1
```

It always tears the fixture down and checks that the container, data root,
temporary TLS material and ports are gone. It is a logic check with small data,
not production TLS evidence.

## Sensitive fixture teardown

After every E2E run, remove the dedicated container and its runtime directory under `<SR_DOCTOR_CONTAINER_ROOT>` so the fixture-only plugin archive, credentials, and FE logs do not remain. This script first verifies the exact image, label and data-root path; it never enumerates or touches any other container or directory:

```powershell
.\dev\starrocks-3.5.14\teardown.ps1
```

Use `-WhatIf` to review the two owned deletion targets. Retaining a live fixture is an explicit debugging opt-in and is not release-pass evidence.
