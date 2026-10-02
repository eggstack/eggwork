#!/usr/bin/env python3
"""Prove that one exact source revision reproduces one exact Windows binary.

Ownership: this is Eggwork product build evidence (Operations M004), not
producer authority. Eggpack owns the release matrix, assets, sidecars, manifest,
and staging; it cannot detect that a fixed source produced a different Windows
binary on the second attempt, which is exactly what the historical `v0.1.0`
same-tag rerun (`36868105194`) exposed. The staged asset and the rerun asset had
different SHA-256 values, different COFF timestamps, and different CodeView/PDB
RSDS signatures, so Eggpack correctly failed closed on the same-name digest
mismatch.

``.cargo/config.toml`` removes both causes for the `x86_64-pc-windows-msvc`
target:

- ``/BREPRO`` makes ``link.exe`` derive the PE COFF timestamp deterministically
  instead of stamping wall-clock time;
- ``/DEBUG:NONE`` suppresses the CodeView (RSDS) debug record, whose GUID and
  age are regenerated per build.

This module checks the resulting property directly instead of trusting the
flags: two independent release builds of one revision must be byte-identical,
and the produced PE must carry no CodeView record at all.

The module is importable so its PE parsing is unit-testable off-Windows; the
native evidence comes from
``.github/workflows/windows-reproducibility.yml``, which runs this script on a
real MSVC host before any release tag is created.
"""

from __future__ import annotations

import hashlib
import os
import struct
import sys
from dataclasses import dataclass
from pathlib import Path

MAX_IMAGE_BYTES = 512 * 1024 * 1024

DOS_SIGNATURE = b"MZ"
PE_SIGNATURE = b"PE\x00\x00"
COFF_HEADER_SIZE = 20
SECTION_HEADER_SIZE = 40
SECTION_NAME_SIZE = 8
DEBUG_DIRECTORY_ENTRY_SIZE = 28
CODEVIEW_DEBUG_TYPE = 2
RSDS_SIGNATURE = b"RSDS"
OPTIONAL_HEADER_PE32 = 0x10B
OPTIONAL_HEADER_PE32_PLUS = 0x20B
#: Offset of ``NumberOfRvaAndSizes`` inside the optional header, per magic.
NUMBER_OF_RVA_AND_SIZES_OFFSET = {
    OPTIONAL_HEADER_PE32: 92,
    OPTIONAL_HEADER_PE32_PLUS: 108,
}
DATA_DIRECTORY_SIZE = 8
DEBUG_DATA_DIRECTORY_INDEX = 6


class ReproducibilityFailure(Exception):
    """A byte-identity, debug-record, or version-coherence failure."""


@dataclass(frozen=True)
class CodeViewRecord:
    """One IMAGE_DEBUG_TYPE_CODEVIEW entry of a PE image."""

    guid: str
    age: int
    pdb_path: str
    timestamp: int

    def describe(self) -> str:
        return f"RSDS guid={self.guid} age={self.age} pdb={self.pdb_path!r} timestamp={self.timestamp}"


@dataclass(frozen=True)
class PeImage:
    """The parsed PE facts this qualification needs.

    Only the COFF timestamp and the CodeView debug records are read; nothing
    here is executable and nothing is imported or run.
    """

    coff_timestamp: int
    codeview_records: tuple[CodeViewRecord, ...]


def sha256_file(path: Path) -> str:
    """Return the lowercase SHA-256 of one bounded regular file."""
    if not path.is_file():
        raise ReproducibilityFailure(f"{path} is not a regular file")
    size = path.stat().st_size
    if size == 0 or size > MAX_IMAGE_BYTES:
        raise ReproducibilityFailure(f"{path} has an out-of-bounds size")
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def _u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def _u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def _rva_to_offset(data: bytes, sections: list[tuple[int, int, int]], rva: int) -> int | None:
    for virtual_address, virtual_size, raw_pointer, raw_size in sections:
        span = max(virtual_size, raw_size)
        if span and virtual_address <= rva < virtual_address + span:
            return raw_pointer + (rva - virtual_address)
    return None


def _format_guid(raw: bytes) -> str:
    first, second, third = struct.unpack_from("<IHH", raw, 0)
    tail = raw[8:16]
    return f"{first:08x}-{second:04x}-{third:04x}-{tail[0]:02x}{tail[1]:02x}-{tail[2:].hex()}"


def parse_pe_image(data: bytes) -> PeImage:
    """Parse the COFF timestamp and CodeView records of a PE image.

    Raises ``ReproducibilityFailure`` for a truncated, non-PE, or unsupported
    image rather than guessing: a qualification check must fail closed on a
    file it does not fully understand.
    """
    if len(data) < 0x40 or not data.startswith(DOS_SIGNATURE):
        raise ReproducibilityFailure("image does not start with an MZ DOS header")
    pe_offset = _u32(data, 0x3C)
    if pe_offset + 4 + COFF_HEADER_SIZE > len(data) or data[pe_offset : pe_offset + 4] != PE_SIGNATURE:
        raise ReproducibilityFailure("image has no PE signature")
    coff_offset = pe_offset + 4
    section_count = _u16(data, coff_offset + 2)
    coff_timestamp = _u32(data, coff_offset + 4)
    optional_size = _u16(data, coff_offset + 16)
    optional_offset = coff_offset + COFF_HEADER_SIZE
    if optional_size < 2 or optional_offset + optional_size > len(data):
        raise ReproducibilityFailure("optional header is out of bounds")
    magic = _u16(data, optional_offset)
    counts_offset = NUMBER_OF_RVA_AND_SIZES_OFFSET.get(magic)
    if counts_offset is None:
        raise ReproducibilityFailure(f"unsupported optional header magic {magic:#x}")
    directory_count = _u32(data, optional_offset + counts_offset)
    if directory_count <= DEBUG_DATA_DIRECTORY_INDEX:
        return PeImage(coff_timestamp=coff_timestamp, codeview_records=())
    directory_offset = (
        optional_offset
        + counts_offset
        + 4
        + DEBUG_DATA_DIRECTORY_INDEX * DATA_DIRECTORY_SIZE
    )
    if directory_offset + DATA_DIRECTORY_SIZE > len(data):
        raise ReproducibilityFailure("data directory is out of bounds")
    debug_rva = _u32(data, directory_offset)
    debug_size = _u32(data, directory_offset + 4)
    if debug_rva == 0 or debug_size == 0:
        return PeImage(coff_timestamp=coff_timestamp, codeview_records=())

    sections: list[tuple[int, int, int, int]] = []
    section_offset = optional_offset + optional_size
    for index in range(section_count):
        base = section_offset + index * SECTION_HEADER_SIZE
        if base + SECTION_HEADER_SIZE > len(data):
            raise ReproducibilityFailure("section table is out of bounds")
        name = data[base : base + SECTION_NAME_SIZE].rstrip(b"\x00")
        if not name:
            raise ReproducibilityFailure("section table has an unnamed entry")
        sections.append(
            (
                _u32(data, base + 12),
                _u32(data, base + 8),
                _u32(data, base + 20),
                _u32(data, base + 16),
            )
        )
    debug_offset = _rva_to_offset(data, sections, debug_rva)
    if debug_offset is None or debug_offset + debug_size > len(data):
        raise ReproducibilityFailure("debug directory is not backed by file data")
    if debug_size % DEBUG_DIRECTORY_ENTRY_SIZE != 0:
        raise ReproducibilityFailure("debug directory size is not a multiple of its entry size")

    records: list[CodeViewRecord] = []
    for entry in range(debug_size // DEBUG_DIRECTORY_ENTRY_SIZE):
        base = debug_offset + entry * DEBUG_DIRECTORY_ENTRY_SIZE
        entry_type = _u32(data, base + 12)
        data_size = _u32(data, base + 16)
        data_pointer = _u32(data, base + 24)
        if entry_type != CODEVIEW_DEBUG_TYPE:
            continue
        if data_size < 24 or data_pointer + data_size > len(data):
            raise ReproducibilityFailure("CodeView record is out of bounds")
        payload = data[data_pointer : data_pointer + data_size]
        if not payload.startswith(RSDS_SIGNATURE):
            # NB55/NB09 CodeView variants embed a timestamp of their own. They
            # are nondeterministic too, so they are rejected rather than
            # tolerated under the same "no CodeView record" requirement.
            raise ReproducibilityFailure("CodeView record is not an RSDS record")
        records.append(
            CodeViewRecord(
                guid=_format_guid(payload[4:20]),
                age=_u32(payload, 20),
                pdb_path=payload[24:].split(b"\x00", 1)[0].decode("utf-8", "replace"),
                timestamp=_u32(data, base + 4),
            )
        )
    return PeImage(coff_timestamp=coff_timestamp, codeview_records=tuple(records))


def inspect_image(path: Path) -> PeImage:
    """Read one bounded PE image from disk and parse it."""
    data = path.read_bytes()
    if not data or len(data) > MAX_IMAGE_BYTES:
        raise ReproducibilityFailure(f"{path} has an out-of-bounds size")
    return parse_pe_image(data)


def require_byte_identical(first: Path, second: Path) -> tuple[str, PeImage, PeImage]:
    """Require two builds of one revision to be the same bytes.

    Returns the shared digest and both parsed images. A mismatch is reported
    with the two digests and both COFF timestamps, because the historical
    `v0.1.0` rerun mismatch was exactly a timestamp plus CodeView difference
    and a bare "digests differ" would not identify it.
    """
    first_digest = sha256_file(first)
    second_digest = sha256_file(second)
    first_image = inspect_image(first)
    second_image = inspect_image(second)
    if first_digest != second_digest:
        raise ReproducibilityFailure(
            f"two builds of one revision differ: {first.name} sha256={first_digest} "
            f"coff_timestamp={first_image.coff_timestamp} and {second.name} "
            f"sha256={second_digest} coff_timestamp={second_image.coff_timestamp}"
        )
    if first.read_bytes() != second.read_bytes():
        raise ReproducibilityFailure("identical digests but different bytes")
    return first_digest, first_image, second_image


def require_no_codeview(path: Path) -> PeImage:
    """Require a PE image to carry no CodeView/PDB debug record.

    ``/DEBUG:NONE`` is what makes this true; a CodeView record means the
    deterministic policy did not apply, either because the target-scoped
    rustflags were replaced or because they were never present.
    """
    image = inspect_image(path)
    if image.codeview_records:
        described = "; ".join(record.describe() for record in image.codeview_records)
        raise ReproducibilityFailure(f"{path} carries a CodeView debug record: {described}")
    return image


def check(first: Path, second: Path) -> str:
    """Require byte identity, no CodeView record, and no caller env override.

    Returns a bounded one-line summary so the caller decides how to report it.
    """
    for name in sorted(os.environ):
        # Spelled by concatenation because Cargo gives the environment
        # precedence over `target.*.rustflags`: any environment spelling of this
        # variable replaces the deterministic policy instead of extending it.
        if name.upper() == "RUST" "FLAGS" or name.upper().endswith("_RUST" "FLAGS"):
            raise ReproducibilityFailure(
                f"environment {name} replaces the target-scoped deterministic "
                "MSVC rustflags; unset it before proving reproducibility"
            )
    digest, first_image, second_image = require_byte_identical(first, second)
    require_no_codeview(first)
    require_no_codeview(second)
    return (
        f"reproducible: sha256={digest} "
        f"coff_timestamp={first_image.coff_timestamp}/{second_image.coff_timestamp} "
        f"no CodeView record in {first.name} or {second.name}"
    )


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: verify_windows_reproducibility.py <build-a-exe> <build-b-exe>")
    try:
        print(check(Path(sys.argv[1]), Path(sys.argv[2])))
    except ReproducibilityFailure as failure:
        raise SystemExit(f"windows reproducibility preflight failed: {failure}") from failure
