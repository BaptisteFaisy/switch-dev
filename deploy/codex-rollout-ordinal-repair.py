#!/usr/bin/env python3
"""Normalize ordinals in one or more paginated Codex rollout JSONL files."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import stat
import sys
import time


def normalize_rollout(path: Path) -> bool:
    if not path.is_file():
        return False

    with path.open("r", encoding="utf-8") as source:
        first_line = source.readline()
    if not first_line:
        return False
    first_record = json.loads(first_line)
    if not isinstance(first_record, dict) or "ordinal" not in first_record:
        return False

    metadata = path.stat()
    nonce = f"{os.getpid()}-{time.time_ns()}"
    temporary = path.with_name(f".{path.name}.cst-ordinal-{nonce}.tmp")
    changed = False

    try:
        with path.open("r", encoding="utf-8") as source, temporary.open(
            "x", encoding="utf-8"
        ) as target:
            for ordinal, line in enumerate(source):
                record = json.loads(line)
                if not isinstance(record, dict):
                    raise ValueError(f"record {ordinal} is not a JSON object")
                current = record.get("ordinal")
                if type(current) is not int or current != ordinal:
                    changed = True
                record["ordinal"] = ordinal
                json.dump(record, target, ensure_ascii=False, separators=(",", ":"))
                target.write("\n")
            target.flush()
            os.fsync(target.fileno())

        if not changed:
            temporary.unlink()
            return False

        backup = path.with_name(
            f"{path.name}.pre-cst-ordinal-{time.strftime('%Y%m%d-%H%M%S', time.gmtime())}-{os.getpid()}.bak"
        )
        shutil.copy2(path, backup)
        os.chmod(temporary, stat.S_IMODE(metadata.st_mode))
        try:
            os.chown(temporary, metadata.st_uid, metadata.st_gid)
        except PermissionError:
            pass
        os.replace(temporary, path)
        print(f"cst-codex: repaired rollout ordinals in {path}", file=sys.stderr)
        return True
    except Exception:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
        raise


def main(arguments: list[str]) -> int:
    failed = False
    for raw_path in arguments:
        try:
            normalize_rollout(Path(raw_path))
        except Exception as error:
            failed = True
            print(f"cst-codex: ordinal repair failed for {raw_path}: {error}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
