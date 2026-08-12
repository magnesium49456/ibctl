# Autonomous operations

The Docker deployment is configured to recover without an operator while
avoiding tight login loops and preserving known-good Gateway settings.

## Recovery layers

1. ibctl verifies `Connected` with an IB API v100 protocol handshake, not just
   an open TCP port. Three failed probes revoke the state.
2. The dashboard performs the same independent handshake every 10 seconds and
   sends one `RESTART` command per false-Connected incident.
3. JVM recovery uses capped exponential delays. After
   `IBCTL_CONTAINER_EXIT_AFTER_RESTARTS` failures (default 8), ibctl exits so
   Docker can recreate the entire container.
4. Autonomous recovery stays in bounded backoff indefinitely and never parks
   in `GivenUp` waiting for a person.
5. Docker health is diagnostic; authentication and legitimate 2FA waits remain
   healthy. PID 1 exits only through the internal recovery policy.

An authenticated Gateway explicitly reporting `API Server: disconnected` is
left intact for `IBCTL_API_DISCONNECTED_GRACE_SECS` (default 600 seconds),
giving its native broker reconnection logic time to work without another 2FA
cycle. An unknown UI state still fails closed after two minutes.

## 2FA time and retry handling

At startup, every five minutes, and immediately after a detected code
rejection, ibctl samples Cloudflare, Google, and pool.ntp.org. A bounded median
offset is applied only to TOTP generation. It does not grant the container
permission to change the host clock.

If Gateway says `try again in 43 seconds` (or uses minutes), ibctl parses that
countdown, adds a two-second safety margin, and will not submit another login
before the deadline. It also avoids generating a code at the unsafe edge of a
30-second TOTP window.

For Docker Desktop on Windows, install the companion host task. An elevated
PowerShell prompt permits Windows' force-sync actions; a standard-user install
still performs public-NTP offset verification and best-effort sync requests:

```powershell
.\host\windows\Install-IbctlTimeSync.ps1 -PeriodicMinutes 5
```

The task checks every minute for a container sync request and otherwise forces
and verifies W32Time at least every five minutes. It additionally verifies a
public-NTP offset when UDP/123 is available. State is recorded under
`%LOCALAPPDATA%\ibctl-time-sync` and the result is copied to the persistent
container volume.

## Persistence and retention

`ibctl-settings` persists `/home/ibgateway/settings`, while the image-owned
`/home/ibgateway/Jts` continues to carry the latest Gateway installation.
`ibctl-persist` stores logs, recovery markers, and encrypted settings backups. A backup is created only
after the configured stable-Connected dwell. On a full-container recovery,
the newest known-good snapshot is restored. Logs default to 45 days, settings
backups to 30 days, and Docker's local JSON logs rotate at 10 MB x 5 files.

The encryption key is generated with mode `0600` at
`/opt/ibctl/persist/settings-backup.key`. Back up that key together with the
Docker volume; a backup archive cannot be restored without it.

## Latest Gateway updates with rollback

The Dockerfile resolves `IB_GATEWAY_VERSION=latest` at build time. Each canary
build supplies a unique refresh token so Docker cannot reuse a stale version-resolution layer. The canary
deployment script retains the current image as `ibctl:last-known-good`, builds
with `--pull`, deploys the candidate, and promotes it only after the status API
is continuously ready for 90 seconds. Otherwise it automatically restores the
last-known-good image.

Run it manually:

```powershell
.\docker\Deploy-LatestWithRollback.ps1
```

Or install the weekly task:

```powershell
.\host\windows\Install-IbctlGatewayUpdate.ps1 -Day Sunday -At 11:00
```

Useful status fields are `twofa_clock` (verified offset, last verification,
and retry countdown) and `watchdog` (JVM restart count, container-exit
threshold, and probe type).
