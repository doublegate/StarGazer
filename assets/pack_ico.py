#!/usr/bin/env python3
"""Pack square PNG files into one multi-resolution .ico with PNG entries.

PNG-compressed icon entries are supported by Windows Vista and later and keep
the file small (ImageMagick writes uncompressed bitmaps, 30x larger).

Usage: pack_ico.py OUT.ico IN-16.png IN-24.png ... IN-256.png
"""

import pathlib
import struct
import sys

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    out = pathlib.Path(sys.argv[1])
    blobs = []
    for name in sys.argv[2:]:
        data = pathlib.Path(name).read_bytes()
        if data[:8] != PNG_MAGIC:
            sys.exit(f"{name}: not a PNG")
        width, height = struct.unpack(">II", data[16:24])  # IHDR
        if width != height or width > 256:
            sys.exit(f"{name}: must be square and at most 256 px, got {width}x{height}")
        blobs.append((width, data))

    blobs.sort(key=lambda b: b[0])
    header = struct.pack("<HHH", 0, 1, len(blobs))  # reserved, type 1 = icon, count
    offset = 6 + 16 * len(blobs)
    entries, payload = b"", b""
    for size, data in blobs:
        dim = 0 if size == 256 else size  # 0 encodes 256 in an ICONDIRENTRY
        entries += struct.pack(
            "<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset + len(payload)
        )
        payload += data
    out.write_bytes(header + entries + payload)
    print(f"wrote {out}: {len(blobs)} sizes, {out.stat().st_size} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
