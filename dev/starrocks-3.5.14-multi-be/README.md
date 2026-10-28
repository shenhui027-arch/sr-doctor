# StarRocks 3.5.14 isolated 1 FE + 3 BE fixture

Test infrastructure, not a product deployment path. It exists to capture
official `ANALYZE PROFILE` answers from more than one BE (cross-BE broadcast,
shuffle, instance skew, multi-instance fragments) and real Level 2 / running
Profiles. The product connects to it only in the opt-in connected E2E below,
never in CI.

## Pinned inputs

- FE: `starrocks/fe-ubuntu:3.5.14@sha256:…`, BE: `starrocks/be-ubuntu:3.5.14@sha256:…` (exact digests in [`common.ps1`](./common.ps1)).
- Expected server and every BE: `3.5.14-23a56ec`; `up.ps1` refuses anything else.

## Isolation contract

For a constrained developer machine, pass `-LowResource` to `up.ps1`. It
starts the same pinned 1 FE + 3 BE topology with these hard limits from
container creation:

| Process | CPU | Container memory | Application limit |
|---|---:|---:|---:|
| FE | 1 | 1536 MiB | JVM heap 768 MiB |
| Each of 3 BEs | 0.5 | 1280 MiB | `mem_limit=896M` |

The container memory caps total 5376 MiB (5.25 GiB), CPU caps total 2.5,
and memory plus swap is capped at the same value as memory. The profile is
for tiny, serial topology checks; the standard oracle dataset is larger.
The bounded runner at
`docs/v1.0/tasks/06-bounded-docker-topology/run.ps1` uses 8056 total business
rows, a 15-second / 128-MiB limit on its complex queries, and always tears
down its owned fixture. It does not change production G1/G2 acceptance.

Without `-LowResource`, the existing FE/BE CPU, memory and application limits
are retained. Bootstrap receipts include the chosen resource profile.

- Names: `sr-doctor-v0.1-e2e-3514-mbe-{fe,be1,be2,be3}` on the dedicated bridge network `sr-doctor-v0.1-e2e-3514-mbe-net`; every object carries `com.sr-doctor.e2e=3.5.14-multi-be`. These names never collide with the allin1 fixtures (`sr-doctor-v0.1-e2e-3514`, `-oracle`, `-runtime`, …), and both scripts refuse a container under one of these names whose image digest or label differs.
- The network uses the explicit subnet `10.235.14.0/24`: on a host with many projects Docker's predefined address pools can be exhausted. `up.ps1` refuses to start if any existing Docker network overlaps it; it never prunes or edits another network.
- Only the FE publishes ports, and only on `127.0.0.1`: `38530→8030`, `39530→9030`. BEs publish nothing.
- Data: `<SR_DOCTOR_CONTAINER_ROOT>\sr-doctor-v0.1-e2e-3514-mbe\{fe-meta,be1-storage,be2-storage,be3-storage,shared-files}`.
- `shared-files` is mounted at `/tmp/oracle_shapes` in the FE and every BE. It stands in for the shared NFS directory that `file://` paths need on a multi-node cluster: each BE writes its own `INSERT INTO FILES` part locally and the FE lists `FILES()` paths on its own file system, so without it `FILES('file:///tmp/oracle_shapes/…')` finds no file (observed `ERROR 5600 … No files were found matching the pattern(s)`). Teardown removes it with the data root.
- Limits: FE 2 CPUs / 2.5 GiB (JVM `-Xmx1536m`), each BE 3 CPUs / 3 GiB (`mem_limit = 2048M`).
- `up.ps1` is cold-start only: it fails if any container, the network, the data root or a host port is already present.

## Credentials

`STARROCKS_E2E_SECRET` must be a process-only token (24–64 characters of `[A-Za-z0-9_-]`). Root's password is derived from it and set as native-password ciphertext right after the three `ALTER SYSTEM ADD BACKEND` statements; until then root has no password and the FE is reachable only from `127.0.0.1` and the fixture network. Scripts pass the password to `mysql` inside the FE container through the inherited `MYSQL_PWD` variable (`docker exec -e MYSQL_PWD`), never on a command line, and never print or store it.

```powershell
$env:DOCKER_EXE = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'   # only when docker is not on PATH
$bytes = [byte[]]::new(30); [Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
$env:STARROCKS_E2E_SECRET = [Convert]::ToBase64String($bytes).Replace('+','-').Replace('/','_').TrimEnd('=')
try {
    .\dev\starrocks-3.5.14-multi-be\up.ps1
    .\scripts\e2e\capture-oracle-corpus.ps1 -Topology multi-be -CorpusName oracle-corpus-multi-be
    .\scripts\e2e\capture-profile-states.ps1
} finally {
    .\dev\starrocks-3.5.14-multi-be\teardown.ps1
    [Environment]::SetEnvironmentVariable('STARROCKS_E2E_SECRET', $null, 'Process')
}
```

## Connected read-only E2E

`connected-fixture.ps1` turns a cluster from `up.ps1` into the connected-mode
fixture that `dev/starrocks-3.5.14/bootstrap.ps1` and `run-fixture-workload.ps1`
provide for allin1: AuditLoader 5.0.0 (same pinned archive and SHA-256), the
`doctor_reader` / `audit_writer` / `workload_user` roles as native-password
ciphertext, a Profile table whose six tablets land on all three BEs, two
finished Level 1 Profiles and one terminally failed Audit row. Differences from
allin1:

- AuditLoader keeps the official `frontend_host_port=127.0.0.1:8030`. The
  allin1-only `127.0.0.1:8040` override is not needed here: the split FE has
  no local BE, and the observed Stream Load through 8030 publishes Audit rows.
- `doctor_reader` may also `SELECT` the Profile table, so Runtime Top can run
  one query whose scan, aggregation and shuffle span all three BEs.
- No digest / F05 fixture: those are browser-release assertions.

`scripts/e2e/run-connected-multi-be-e2e.ps1` runs the whole flow from a cold
start over HTTP (no browser) and always tears down in `finally`. It refuses to
start while the allin1 release fixture `sr-doctor-v0.1-e2e-3514`
exists, generates the process token itself when `STARROCKS_E2E_SECRET` is
unset (and clears it afterwards), starts its own `doctor-api` on
`127.0.0.1:18063`, reuses `scripts/e2e/connected-smoke.ps1` unchanged, checks
in the Audit table that the topology-only path sent only `EXPLAIN COSTS` and
that no rejected statement reached the server, scans the owned API logs, and
writes its receipt to
`<SR_DOCTOR_WORK_ROOT>\e2e\connected-multi-be-<UTC>.json`:

```powershell
$env:DOCKER_EXE = 'C:\Program Files\Docker\Docker\resources\bin\docker.exe'   # only when docker is not on PATH
.\scripts\e2e\run-connected-multi-be-e2e.ps1
```

`teardown.ps1` removes the four containers, the network and the data root, then fails unless all of them are gone and both host ports are free. `-WhatIf` lists the owned targets without deleting anything.

The evidence proves only this local Docker topology (one host, one bridge network, synthetic data). Production-safe behavior and production load impact remain `INCONCLUSIVE`.
