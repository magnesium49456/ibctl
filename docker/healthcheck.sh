#!/bin/sh
set -eu

port="${IBCTL_COMMAND_PORT:-7462}"
case "${IBCTL_COMMAND_SERVER_ENABLED:-false}" in
    true|TRUE|yes|YES|1) ;;
    *) pgrep -x ibctl >/dev/null; exit $? ;;
esac
python3 - "$port" <<'PY'
import json, socket, sys

port = int(sys.argv[1])
with socket.create_connection(("127.0.0.1", port), timeout=3) as sock:
    sock.sendall(b"STATUS\n")
    sock.settimeout(3)
    data = b""
    while b"\n" not in data and len(data) < 65536:
        chunk = sock.recv(4096)
        if not chunk:
            break
        data += chunk
body = data.decode().strip()
if body.startswith("OK "):
    body = body[3:]
status = json.loads(body)
state = status.get("state")
if state in {"Shutdown"} or str(state).startswith("Error"):
    raise SystemExit(1)
if status.get("jvm", {}).get("alive") is False and state not in {
    "Init", "Launching", "Restarting", "WaitingForIB", "WaitingForLaunch"
}:
    raise SystemExit(1)
PY
