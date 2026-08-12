import asyncio

import pytest

from app.services.monitors.false_connected import FalseConnectedMonitor, _probe_port


@pytest.mark.asyncio
async def test_protocol_probe_rejects_plain_tcp_listener():
    async def plain_listener(_reader, writer):
        try:
            await asyncio.sleep(1)
        finally:
            writer.close()

    server = await asyncio.start_server(plain_listener, "127.0.0.1", 0)
    try:
        port = server.sockets[0].getsockname()[1]
        assert not await _probe_port("127.0.0.1", port, 0.1)
    finally:
        server.close()
        await server.wait_closed()


class _Status:
    mode = "live"
    status = {"state": "Connected"}


class _Client:
    def __init__(self):
        self.commands = []

    async def send_command(self, command):
        self.commands.append(command)
        return "OK"


class _Registry:
    def __init__(self):
        self.client = _Client()

    def cached_all_status(self):
        return [_Status()]

    def get_client(self, _mode):
        return self.client


class _Ns:
    def is_event_enabled(self, _event):
        return True


@pytest.mark.asyncio
async def test_watchdog_restarts_once_after_threshold(monkeypatch):
    async def failed_probe(*_args, **_kwargs):
        return False

    monkeypatch.setattr("app.services.monitors.false_connected._probe_port", failed_probe)
    monitor = FalseConnectedMonitor()
    registry = _Registry()
    alerts = []
    for _ in range(4):
        alerts.extend(await monitor.check(registry, _Ns()))
    assert registry.client.commands == ["RESTART"]
    assert len(alerts) == 1
