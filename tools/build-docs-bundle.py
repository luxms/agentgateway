#!/usr/bin/env python3
"""Package a built static documentation site; requires only Python 3's standard library."""

import argparse
import gzip
import hashlib
import json
import mimetypes
from pathlib import Path
import struct
import tempfile

MAGIC = b"AGWDOCS1"


def valid_path(path, directory=False):
    value = path.removesuffix("/") if directory else path
    return (directory and not value) or (
        bool(value)
        and not any(c in value for c in "\\?#%")
        and not any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in value)
        and all(part not in ("", ".", "..") for part in value.split("/"))
    )


def build(source, output, version, entrypoint=""):
    source, output = Path(source).resolve(), Path(output).resolve()
    if not source.is_dir():
        raise ValueError("source must be a built site directory")
    if output.is_relative_to(source):
        raise ValueError("output must be outside the input directory")
    if not version or not valid_path(entrypoint, directory=True):
        raise ValueError("version must be nonempty and entrypoint must be a relative site path")
    paths = sorted(source.rglob("*"))
    if any(path.is_symlink() for path in paths):
        raise ValueError("symbolic links are not supported")
    # Start with built-in MIME types, independent of host /etc/mime.types settings.
    types = mimetypes.MimeTypes()
    types.add_type("application/javascript", ".js")
    types.add_type("application/wasm", ".wasm")
    types.add_type("application/manifest+json", ".webmanifest")
    assets = []
    with tempfile.TemporaryFile() as payload:
        for path in paths:
            if not path.is_file():
                continue
            name = path.relative_to(source).as_posix()
            if not valid_path(name):
                raise ValueError(f"unsupported asset path: {name}")
            raw = path.read_bytes()
            compressed = gzip.compress(raw, compresslevel=9, mtime=0)
            data, encoding = (compressed, "gzip") if len(compressed) < len(raw) else (raw, "identity")
            content_type = types.guess_type(name)[0] or "application/octet-stream"
            if content_type.startswith("text/"):
                content_type += "; charset=utf-8"
            assets.append(dict(path=name, contentType=content_type, encoding=encoding,
                               offset=payload.tell(), length=len(data), originalLength=len(raw),
                               sha256=hashlib.sha256(data).hexdigest()))
            payload.write(data)
        entry = entrypoint + "index.html" if not entrypoint or entrypoint.endswith("/") else entrypoint
        if entry not in {asset["path"] for asset in assets}:
            raise ValueError(f"entrypoint does not exist: {entry}")
        manifest = json.dumps(dict(version=version, entrypoint=entrypoint, assets=assets),
                              ensure_ascii=False, separators=(",", ":")).encode()
        # Publish only a complete bundle, including when updating an existing file.
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=output.parent, delete=False) as destination:
                temporary = Path(destination.name)
                destination.write(MAGIC + struct.pack("<Q", len(manifest)) + manifest)
                payload.seek(0)
                while block := payload.read(1024 * 1024):
                    destination.write(block)
            temporary.replace(output)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
    return len(assets), output.stat().st_size


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, help="static site built for /ui/docs/ (including any UI base path)")
    parser.add_argument("output", type=Path)
    parser.add_argument("--version", required=True, help="documentation version, e.g. 1.7.0")
    parser.add_argument("--entrypoint", default="", help="relative landing page, e.g. docs/standalone/latest/")
    args = parser.parse_args()
    try:
        count, size = build(args.source, args.output, args.version, args.entrypoint)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")
    print(f"{args.output}: {count} files, {size:,} bytes")


if __name__ == "__main__":
    main()
