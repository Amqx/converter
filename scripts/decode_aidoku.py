#!/usr/bin/env python3
import argparse
import base64
import datetime
import json
import plistlib
import sys
from pathlib import Path

def _default_encoder(obj):
    """Make plist-only types JSON-serializable."""
    if isinstance(obj, datetime.datetime):
        # plist dates are UTC; emit ISO-8601 with a trailing Z.
        if obj.tzinfo is None:
            obj = obj.replace(tzinfo=datetime.timezone.utc)
        return obj.astimezone(datetime.timezone.utc).isoformat().replace("+00:00", "Z")
    if isinstance(obj, (bytes, bytearray)):
        return base64.b64encode(obj).decode("ascii")
    if isinstance(obj, plistlib.UID):
        return obj.data
    raise TypeError(f"Object of type {type(obj).__name__} is not JSON serializable")


def load_aidoku(raw: bytes):
    """Parse Aidoku backup bytes from either binary plist or JSON."""
    if raw[:8] == b"bplist00":
        return plistlib.loads(raw)
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        # Fall back to plist for XML plists or other plist encodings.
        return plistlib.loads(raw)


def main(argv=None):
    parser = argparse.ArgumentParser(description="Decode an Aidoku backup to JSON.")
    parser.add_argument("input", type=Path, help="Aidoku backup (.aib / .json)")
    parser.add_argument(
        "output",
        type=Path,
        nargs="?",
        help="output .json path (default: input with .json extension)",
    )
    args = parser.parse_args(argv)

    if not args.input.is_file():
        parser.error(f"input file not found: {args.input}")

    output = args.output or args.input.with_suffix(".json")
    if output.resolve() == args.input.resolve():
        parser.error("refusing to overwrite the input file; pass an explicit output path")

    data = load_aidoku(args.input.read_bytes())
    with output.open("w", encoding="utf-8") as f:
        json.dump(data, f, default=_default_encoder, ensure_ascii=False, indent=2)
        f.write("\n")

    print(f"decoded {args.input} -> {output}", file=sys.stderr)


if __name__ == "__main__":
    main()
