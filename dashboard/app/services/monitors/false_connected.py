"""Monitor: cross-layer consistency check.

Detects false-Connected state by comparing the state machine's self-reported
state against a ground-truth IB protocol handshake with the Gateway API port.

Root cause from the 2026-04-16 incident: the state machine can enter Connected
while the underlying Gateway is still at the login form (fail-open paths +
liveness check looking at the wrong component). The state machine's own
observations can't always be trusted — an external observer needs to verify.

How it works:
  1. For each instance reporting state == "Connected", open a TCP socket to
     the Gateway's API port (4001 live / 4002 paper, internal to the container).
  2. If the protocol handshake fails for N consecutive probes, the state is a lie.
  3. Issue one automated RESTART and fire a critical alert.

The state machine remains responsible for recovery policy; this independent
watchdog contributes one bounded recovery signal per incident.
"""

from __future__ import annotations

import asyncio
import logging
import os
import struct
import time

from app.services.monitor_manager import Alert, Monitor

logger = logging.getLogger("dashboard.services.monitors.false_connected")

# How many consecutive failed probes before alerting. Protects against
# transient blips during warm restart bursts or brief socket hiccups.
FAIL_THRESHOLD = 3

# TCP connect timeout per probe
PROBE_TIMEOUT_SECS = 2.0


def _port_for_mode(mode: str) -> int:
    """Return the internal Gateway API port for the given mode.

    Reads from env vars with the same defaults as ibctl config.
    These are the in-container ports Gateway listens on — distinct from
    the socat-forwarded ports published to the host.
    """
    env_var = "LIVE_API_PORT" if mode == "live" else "PAPER_API_PORT"
    default = 4001 if mode == "live" else 4002
    try:
        return int(os.environ.get(env_var, str(default)))
    except ValueError:
        return default


async def _probe_port(host: str, port: int, timeout: float) -> bool:
    """Perform an IB API v100 server-version handshake."""
    writer = None
    try:
        reader, writer = await asyncio.wait_for(asyncio.open_connection(host, port), timeout=timeout)
        payload = b"v100..178\0"
        writer.write(b"API\0" + struct.pack(">I", len(payload)) + payload)
        await writer.drain()
        size = struct.unpack(">I", await asyncio.wait_for(reader.readexactly(4), timeout=timeout))[0]
        if size < 2 or size > 4096:
            return False
        response = await asyncio.wait_for(reader.readexactly(size), timeout=timeout)
        first = response.split(b"\0", 1)[0]
        reachable = first.isdigit() and int(first) >= 100
        return reachable
    except asyncio.CancelledError:
        # Shutdown-time cancellation must propagate so the monitor loop
        # can exit cleanly. Never swallow it.
        raise
    except (asyncio.TimeoutError, OSError, ConnectionRefusedError):
        return False
    except Exception as e:
        logger.debug("Probe of %s:%d raised unexpected error: %s", host, port, e)
        return False
    finally:
        if writer is not None:
            writer.close()
            try:
                await writer.wait_closed()
            except asyncio.CancelledError:
                raise
            except Exception as e:
                logger.debug("writer.wait_closed() raised: %s", e)


class FalseConnectedMonitor(Monitor):
    """Cross-layer consistency monitor.

    Verifies the state machine's "Connected" claim by probing the underlying
    Gateway API port. The state machine can't always detect when its own
    assumptions diverge from reality (see incident 2026-04-16); this external
    check catches those cases regardless of what the state machine thinks.
    """

    event_type = "false_connected"
    # Probe every 10s. Combined with FAIL_THRESHOLD=3, operators get an
    # alert within 30s of a false-Connected state — down from the previous
    # 3 minutes (60s × 3). Still keeps a 2s timeout per probe, so 3 probes ×
    # 2 instances × 2s worst-case = 12s bounded work inside each 10s tick,
    # well within the monitor-manager budget.
    interval_seconds = 10

    def __init__(self):
        # mode -> consecutive failed probe count
        self._fail_streak: dict[str, int] = {}
        # mode -> whether alert already sent for current streak (dedup until recovery)
        self._alerted: dict[str, bool] = {}
        self._restart_sent: dict[str, bool] = {}

    async def check(self, registry, ns) -> list[Alert]:
        if not ns.is_event_enabled(self.event_type):
            return []

        alerts: list[Alert] = []
        instances = registry.cached_all_status()

        for inst in instances:
            mode = inst.mode
            status = inst.status or {}
            state = status.get("state", "unknown")

            # Only probe when the state machine claims to be ready.
            # In any other state (Launching, WaitingForLogin, Restarting, etc.)
            # the API port is expected to be unreachable — probing would
            # produce false positives.
            if state != "Connected":
                self._fail_streak.pop(mode, None)
                self._alerted.pop(mode, None)
                self._restart_sent.pop(mode, None)
                continue

            port = _port_for_mode(mode)
            reachable = await _probe_port("127.0.0.1", port, PROBE_TIMEOUT_SECS)

            if reachable:
                if self._fail_streak.get(mode, 0) > 0:
                    logger.info(
                        "%s API port %d recovered after %d failed probes",
                        mode.upper(), port, self._fail_streak[mode],
                    )
                self._fail_streak[mode] = 0
                self._alerted[mode] = False
                self._restart_sent[mode] = False
                continue

            # Probe failed — increment streak
            self._fail_streak[mode] = self._fail_streak.get(mode, 0) + 1
            streak = self._fail_streak[mode]
            logger.warning(
                "%s: state=Connected but IB API handshake to 127.0.0.1:%d failed (streak=%d/%d)",
                mode.upper(), port, streak, FAIL_THRESHOLD,
            )

            if streak >= FAIL_THRESHOLD and not self._alerted.get(mode, False):
                restart_result = "not attempted"
                if not self._restart_sent.get(mode, False):
                    try:
                        restart_result = await registry.get_client(mode).send_command("RESTART")
                        self._restart_sent[mode] = True
                        logger.error("%s false-Connected watchdog issued RESTART: %s", mode.upper(), restart_result)
                    except Exception as error:
                        restart_result = f"failed: {error}"
                        logger.exception("%s false-Connected automatic RESTART failed", mode.upper())
                alerts.append(Alert(
                    event_type=self.event_type,
                    title=f"ibctl: {mode.upper()} false-Connected detected",
                    body=(
                        f"State machine reports {mode.upper()}=Connected but IB API handshake "
                        f"to 127.0.0.1:{port} has failed {streak} times in a row.\n"
                        f"Gateway may be at login form or API server may be down.\n"
                        f"Automatic RESTART result: {restart_result}."
                    ),
                    priority="urgent",
                    tags="rotating_light,warning",
                ))
                self._alerted[mode] = True

        return alerts
