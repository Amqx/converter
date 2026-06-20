#!/usr/bin/env python3
import argparse
import gzip
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

try:
    from google.protobuf import (
        descriptor_pb2,
        descriptor_pool,
        json_format,
        message_factory,
    )
except ImportError:
    sys.exit(
        "error: the 'protobuf' package is required.\n"
        "Install it into the project venv, e.g.:\n"
        "    uv pip install --python .wslvenv/bin/python protobuf"
    )

PROTO_PACKAGE_MESSAGE = "tachiyomisy.backup.Backup"
GZIP_MAGIC = b"\x1f\x8b"

def find_proto(explicit: Path | None) -> Path:
    """Locate proto/tachiyomi.proto relative to this script or the cwd."""
    if explicit is not None:
        if not explicit.is_file():
            sys.exit(f"error: proto file not found: {explicit}")
        return explicit
    candidates = [
        Path(__file__).resolve().parent.parent / "proto" / "tachiyomi.proto",
        Path.cwd() / "proto" / "tachiyomi.proto",
    ]
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    sys.exit(
        "error: could not find proto/tachiyomi.proto; pass --proto to point at it."
    )


def build_backup_class(proto_path: Path):
    """Compile the .proto with protoc and return a dynamic Backup message class."""
    if shutil.which("protoc") is None:
        sys.exit("error: 'protoc' was not found on PATH.")

    with tempfile.TemporaryDirectory() as tmp:
        descriptor_set = Path(tmp) / "tachiyomi.pb"
        subprocess.run(
            [
                "protoc",
                "--include_imports",
                f"--descriptor_set_out={descriptor_set}",
                f"-I{proto_path.parent}",
                proto_path.name,
            ],
            check=True,
        )
        file_descriptor_set = descriptor_pb2.FileDescriptorSet()
        file_descriptor_set.ParseFromString(descriptor_set.read_bytes())

    pool = descriptor_pool.DescriptorPool()
    for file_proto in file_descriptor_set.file:
        pool.Add(file_proto)
    message_descriptor = pool.FindMessageTypeByName(PROTO_PACKAGE_MESSAGE)
    return message_factory.GetMessageClass(message_descriptor)


def main(argv=None):
    parser = argparse.ArgumentParser(description="Decode a Tachiyomi backup to JSON.")
    parser.add_argument("input", type=Path, help="Tachiyomi backup (.tachibk)")
    parser.add_argument(
        "output",
        type=Path,
        nargs="?",
        help="output .json path (default: input with .json extension)",
    )
    parser.add_argument(
        "--proto",
        type=Path,
        default=None,
        help="path to tachiyomi.proto (default: repo's proto/tachiyomi.proto)",
    )
    args = parser.parse_args(argv)

    if not args.input.is_file():
        parser.error(f"input file not found: {args.input}")

    output = args.output or args.input.with_suffix(".json")
    if output.resolve() == args.input.resolve():
        parser.error("refusing to overwrite the input file; pass an explicit output path")

    raw = args.input.read_bytes()
    if raw[:2] == GZIP_MAGIC:
        raw = gzip.decompress(raw)

    backup_class = build_backup_class(find_proto(args.proto))
    backup = backup_class()
    backup.ParseFromString(raw)

    data = json_format.MessageToDict(backup, preserving_proto_field_name=True)
    with output.open("w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
        f.write("\n")

    print(f"decoded {args.input} -> {output}", file=sys.stderr)


if __name__ == "__main__":
    main()
