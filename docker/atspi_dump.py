#!/usr/bin/env python3
"""Dump a window's AT-SPI accessibility tree as ibctl-compatible JSON."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Iterator

try:
    import pyatspi
except Exception as exc:  # pragma: no cover - depends on container packages
    print(json.dumps({"error": f"pyatspi import failed: {exc}"}))
    sys.exit(2)


def norm(value: object) -> str:
    return str(value or "").strip()


def role_name(node) -> str:
    try:
        return norm(node.getRoleName()).lower()
    except Exception:
        return ""


def node_text(node) -> str:
    values: list[str] = []
    name = norm(getattr(node, "name", ""))
    if name:
        values.append(name)
    try:
        text = node.queryText()
        content = norm(text.getText(0, text.characterCount))
        if content and content not in values:
            values.append(content)
    except Exception:
        pass
    return " ".join(values).strip()


def walk(node, out: dict[str, list], depth: int = 0, max_depth: int = 40) -> None:
    if depth > max_depth:
        return

    role = role_name(node)
    text = node_text(node)
    if text:
        if "button" in role or role in {"menu", "menu item", "check box", "radio button"}:
            out["buttons"].append({"text": text, "role": role})
        elif "text" in role or "password" in role:
            out["textfields"].append({"text": text, "role": role})
        elif "table" in role:
            out["tables"].append({"rows": [[text]]})
        else:
            out["labels"].append(text)

    try:
        count = int(getattr(node, "childCount", 0))
    except Exception:
        count = 0
    for index in range(count):
        try:
            walk(node[index], out, depth + 1, max_depth)
        except Exception:
            continue


def find_window(title: str):
    wanted = title.lower()
    desktop = pyatspi.Registry.getDesktop(0)
    for app_index in range(desktop.childCount):
        app = desktop[app_index]
        for child_index in range(app.childCount):
            child = app[child_index]
            name = norm(getattr(child, "name", ""))
            if not wanted or wanted in name.lower() or name.lower() in wanted:
                return child
    return None


def iter_nodes(node, depth: int = 0, max_depth: int = 40) -> Iterator:
    if depth > max_depth:
        return
    yield node
    try:
        count = int(getattr(node, "childCount", 0))
    except Exception:
        count = 0
    for index in range(count):
        try:
            yield from iter_nodes(node[index], depth + 1, max_depth)
        except Exception:
            continue


def editable_metadata(node) -> str:
    values = [
        norm(getattr(node, "name", "")),
        norm(getattr(node, "description", "")),
        role_name(node),
    ]
    return " ".join(value for value in values if value).lower()


def type_text(window, value: str, hints: list[str]) -> dict:
    candidates: list[tuple[int, object, str]] = []
    for node in iter_nodes(window):
        role = role_name(node)
        if not any(token in role for token in ("text", "entry", "password")):
            continue
        try:
            editable = node.queryEditableText()
        except Exception:
            continue

        metadata = editable_metadata(node)
        score = 0
        for hint in hints:
            if hint and hint in metadata:
                score += 120 if hint == "code" else 220
        if any(token in metadata for token in ("verification", "one-time", "passcode", "otp")):
            score += 180
        if any(
            token in metadata
            for token in ("username", "user name", "password", "account", "search", "filter")
        ):
            score -= 500
        try:
            states = node.getState()
            if states.contains(pyatspi.STATE_FOCUSED):
                score += 80
            if not states.contains(pyatspi.STATE_ENABLED):
                continue
        except Exception:
            pass
        candidates.append((score, editable, metadata))

    if not candidates:
        return {"typed": False, "error": "No editable AT-SPI text field found"}
    if len(candidates) == 1:
        score, editable, metadata = candidates[0]
        score += 250
    else:
        score, editable, metadata = max(candidates, key=lambda candidate: candidate[0])

    try:
        editable.setTextContents(value)
    except Exception as exc:
        return {"typed": False, "error": f"AT-SPI text entry failed: {exc}"}
    return {
        "typed": True,
        "selector": "semantic-metadata" if metadata else "single-editable-field",
        "score": score,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--title", default="")
    parser.add_argument("--type-text-stdin", action="store_true")
    parser.add_argument("--hints", default="")
    args = parser.parse_args()

    window = find_window(args.title)
    if window is None:
        print(json.dumps({"error": f"AT-SPI window not found: {args.title}"}))
        return 1

    if args.type_text_stdin:
        value = sys.stdin.read().strip()
        if not value:
            print(json.dumps({"typed": False, "error": "No text supplied on stdin"}))
            return 1
        hints = [item.strip().lower() for item in args.hints.split("|") if item.strip()]
        result = type_text(window, value, hints)
        print(json.dumps(result))
        return 0 if result.get("typed") else 1

    out = {
        "source": "atspi",
        "title": norm(getattr(window, "name", "")),
        "buttons": [],
        "textfields": [],
        "labels": [],
        "tables": [],
    }
    walk(window, out)

    # Stable de-duplication keeps dumps small and deterministic.
    out["labels"] = list(dict.fromkeys(out["labels"]))
    print(json.dumps(out, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
