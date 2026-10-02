"""Bounded tests for the Windows reproducibility preflight tooling.

The preflight itself is native evidence produced on an MSVC host by
``.github/workflows/windows-reproducibility.yml``. What is proven here, on any
host, is that the PE inspection is correct: it finds an RSDS/CodeView record in
an image that has one, rejects the images the deterministic policy removes,
fails closed on a file it cannot parse, and reports the COFF timestamp that the
historical ``v0.1.0`` same-tag rerun mismatch turned on.

Nothing here executes a candidate or imports PE data. Every fixture is a
synthesized byte buffer with hand-computed offsets, so the parser is tested
against the format rather than against itself.
"""

from __future__ import annotations

import os
import struct
import sys
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
sys.path.insert(0, str(SCRIPTS))

import verify_windows_reproducibility as repro  # noqa: E402

DOS_HEADER_SIZE = 0x40
PE_OFFSET = DOS_HEADER_SIZE
COFF_OFFSET = PE_OFFSET + 4
OPTIONAL_OFFSET = COFF_OFFSET + repro.COFF_HEADER_SIZE
OPTIONAL_SIZE = 0xF0  # PE32+ with the full 16-entry data directory table.
SECTION_OFFSET = OPTIONAL_OFFSET + OPTIONAL_SIZE
SECTION_RVA = 0x1000
SECTION_RAW = 0x400
SECTION_RAW_SIZE = 0x400
DEBUG_ENTRY_RVA = SECTION_RVA + 0x40
DEBUG_ENTRY_RAW = SECTION_RAW + 0x40
CODEVIEW_RAW = SECTION_RAW + 0x80
# A raw bytes literal would keep "\x00" as four literal characters, so the
# CodeView PDB path terminator is appended explicitly.
PDB_PATH = rb"C:\build\eggworkd.pdb" + b"\x00"


def build_pe(*, timestamp: int = 0x66000000, codeview: bool = True, nb55: bool = False) -> bytes:
    """Return a synthesized PE32+ image with one section and one CodeView record.

    ``codeview=False`` produces the image shape ``/DEBUG:NONE`` produces: the
    debug directory is absent, so no CodeView record can exist.
    """
    image = bytearray()
    image += b"MZ"
    image += b"\x00" * (0x3C - 2)
    image += struct.pack("<I", PE_OFFSET)
    image += b"\x00" * (PE_OFFSET - len(image))
    image += b"PE\x00\x00"
    image += struct.pack(
        "<HHIIIHH",
        0x8664,  # Machine: x86-64.
        1,  # NumberOfSections.
        timestamp,  # TimeDateStamp.
        0,  # PointerToSymbolTable.
        0,  # NumberOfSymbols.
        OPTIONAL_SIZE,
        0x0022,  # Characteristics: executable, large-address-aware.
    )
    optional = bytearray(OPTIONAL_SIZE)
    struct.pack_into("<H", optional, 0, repro.OPTIONAL_HEADER_PE32_PLUS)
    struct.pack_into("<I", optional, repro.NUMBER_OF_RVA_AND_SIZES_OFFSET[
        repro.OPTIONAL_HEADER_PE32_PLUS], 16)
    if codeview:
        struct.pack_into(
            "<II",
            optional,
            repro.NUMBER_OF_RVA_AND_SIZES_OFFSET[repro.OPTIONAL_HEADER_PE32_PLUS]
            + 4
            + repro.DEBUG_DATA_DIRECTORY_INDEX * repro.DATA_DIRECTORY_SIZE,
            DEBUG_ENTRY_RVA,
            repro.DEBUG_DIRECTORY_ENTRY_SIZE,
        )
    image += optional
    image += b".text\x00\x00\x00"
    image += struct.pack(
        "<IIIIIIHHI",
        0x40,  # VirtualSize.
        SECTION_RVA,
        SECTION_RAW_SIZE,  # SizeOfRawData.
        SECTION_RAW,  # PointerToRawData.
        0,
        0,
        0,
        0,
        0x60000020,
    )
    image += b"\x00" * (SECTION_RAW - len(image))

    if codeview:
        codeview_payload = bytearray()
        codeview_payload += (b"NB55" if nb55 else repro.RSDS_SIGNATURE)
        codeview_payload += bytes(range(16))
        codeview_payload += struct.pack("<I", 7)  # age
        codeview_payload += PDB_PATH
        # The debug directory entry is addressed by RVA, so its file offset must
        # agree with the section mapping the parser performs.
        image += b"\x00" * (DEBUG_ENTRY_RAW - len(image))
        image += struct.pack(
            "<IIHHIIII",
            0,  # Characteristics.
            0x66000001,  # TimeDateStamp.
            0,
            0,
            repro.CODEVIEW_DEBUG_TYPE,
            len(codeview_payload),
            0,  # AddressOfRawData (unused for images).
            CODEVIEW_RAW,
        )
        image += b"\x00" * (CODEVIEW_RAW - len(image))
        image += codeview_payload
        image += b"\x00" * (SECTION_RAW + SECTION_RAW_SIZE - len(image))
    else:
        image += b"\x00" * (SECTION_RAW + SECTION_RAW_SIZE - len(image))
    assert len(image) == SECTION_RAW + SECTION_RAW_SIZE, len(image)
    return bytes(image)


@contextmanager
def environment(**overrides: str):
    saved = {key: os.environ.get(key) for key in overrides}
    try:
        for key, value in overrides.items():
            os.environ[key] = value
        yield
    finally:
        for key, value in saved.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


class PeParsingTest(unittest.TestCase):
    def test_reads_the_coff_timestamp_that_the_historical_mismatch_turned_on(self) -> None:
        self.assertEqual(repro.parse_pe_image(build_pe(timestamp=0x6A1B2C3D)).coff_timestamp, 0x6A1B2C3D)

    def test_finds_the_rsds_record_a_debug_build_carries(self) -> None:
        records = repro.parse_pe_image(build_pe()).codeview_records
        self.assertEqual(len(records), 1)
        self.assertEqual(records[0].age, 7)
        self.assertEqual(records[0].pdb_path, r"C:\build\eggworkd.pdb")
        self.assertEqual(records[0].timestamp, 0x66000001)
        self.assertRegex(records[0].guid, r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
        self.assertIn("age=7", records[0].describe())

    def test_a_debug_none_image_has_no_codeview_record(self) -> None:
        image = repro.parse_pe_image(build_pe(codeview=False))
        self.assertEqual(image.codeview_records, ())
        self.assertEqual(image.coff_timestamp, 0x66000000)

    def test_non_rsds_codeview_variants_are_rejected_not_tolerated(self) -> None:
        # NB55 embeds a timestamp of its own, so it is nondeterministic for the
        # same reason RSDS is; tolerating it would leave the nondeterminism in.
        with self.assertRaises(repro.ReproducibilityFailure) as caught:
            repro.parse_pe_image(build_pe(nb55=True))
        self.assertIn("not an RSDS record", str(caught.exception))

    def test_unparseable_input_fails_closed(self) -> None:
        for image in [
            b"",
            b"MZ" + b"\x00" * 0x3E,
            b"MZ" + struct.pack("<I", 0xFFFFFFF0) + b"\x00" * 0x20,
            b"MZ" + struct.pack("<I", DOS_HEADER_SIZE) + b"XX\x00\x00" + b"\x00" * 0x40,
        ]:
            with self.subTest(length=len(image)):
                with self.assertRaises(repro.ReproducibilityFailure):
                    repro.parse_pe_image(image)

    def test_unsupported_optional_header_magic_fails_closed(self) -> None:
        image = bytearray(build_pe(codeview=False))
        struct.pack_into("<H", image, OPTIONAL_OFFSET, 0x107)
        with self.assertRaises(repro.ReproducibilityFailure) as caught:
            repro.parse_pe_image(bytes(image))
        self.assertIn("optional header magic", str(caught.exception))

    def test_debug_directory_outside_file_data_fails_closed(self) -> None:
        image = bytearray(build_pe())
        directory_offset = (
            OPTIONAL_OFFSET
            + repro.NUMBER_OF_RVA_AND_SIZES_OFFSET[repro.OPTIONAL_HEADER_PE32_PLUS]
            + 4
            + repro.DEBUG_DATA_DIRECTORY_INDEX * repro.DATA_DIRECTORY_SIZE
        )
        struct.pack_into("<I", image, directory_offset, 0x7F000000)
        with self.assertRaises(repro.ReproducibilityFailure) as caught:
            repro.parse_pe_image(bytes(image))
        self.assertIn("not backed by file data", str(caught.exception))


class PreflightTest(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.root = Path(self.dir.name)

    def write(self, name: str, data: bytes) -> Path:
        path = self.root / name
        path.write_bytes(data)
        return path

    def test_two_identical_stripped_builds_pass(self) -> None:
        first = self.write("a.exe", build_pe(codeview=False))
        second = self.write("b.exe", build_pe(codeview=False))
        digest, image, other = repro.require_byte_identical(first, second)
        self.assertEqual(digest, repro.sha256_file(first))
        self.assertEqual(image.coff_timestamp, other.coff_timestamp)

    def test_a_timestamp_only_difference_is_reported_with_both_timestamps(self) -> None:
        first = self.write("a.exe", build_pe(timestamp=0x66000000, codeview=False))
        second = self.write("b.exe", build_pe(timestamp=0x6A1B2C3D, codeview=False))
        with self.assertRaises(repro.ReproducibilityFailure) as caught:
            repro.require_byte_identical(first, second)
        message = str(caught.exception)
        self.assertIn(f"coff_timestamp={0x66000000}", message)
        self.assertIn(f"coff_timestamp={0x6A1B2C3D}", message)
        # The digest of the *staged-equivalent* build is reported too, so a
        # qualification log identifies both candidates unambiguously.
        self.assertIn(repro.sha256_file(first), message)
        self.assertIn(repro.sha256_file(second), message)

    def test_a_codeview_difference_alone_fails(self) -> None:
        stripped = self.write("a.exe", build_pe(codeview=False))
        # Same code bytes, but the debug build appends its RSDS record, which is
        # exactly the difference seen between the staged and rerun `v0.1.0` asset.
        with self.assertRaises(repro.ReproducibilityFailure):
            repro.require_byte_identical(stripped, self.write("b.exe", build_pe(codeview=True)))

    def test_require_no_codeview_rejects_a_debug_build(self) -> None:
        with self.assertRaises(repro.ReproducibilityFailure) as caught:
            repro.require_no_codeview(self.write("a.exe", build_pe()))
        self.assertIn("CodeView debug record", str(caught.exception))

    def test_require_no_codeview_accepts_a_debug_none_build(self) -> None:
        repro.require_no_codeview(self.write("a.exe", build_pe(codeview=False)))

    def test_check_rejects_an_environment_rustflags_override(self) -> None:
        first = self.write("a.exe", build_pe(codeview=False))
        second = self.write("b.exe", build_pe(codeview=False))
        # Any environment spelling replaces, rather than extends, the
        # target-scoped deterministic rustflags.
        for name in ["RUST" "FLAGS", "CARGO_ENCODED_RUST" "FLAGS", "CARGO_BUILD_RUST" "FLAGS"]:
            with self.subTest(variable=name):
                with environment(**{name: "-C link-arg=/DEBUG"}):
                    with self.assertRaises(repro.ReproducibilityFailure) as caught:
                        repro.check(first, second)
                self.assertIn("replaces the target-scoped", str(caught.exception))

    def test_check_passes_with_no_override_and_identical_stripped_builds(self) -> None:
        first = self.write("a.exe", build_pe(codeview=False))
        second = self.write("b.exe", build_pe(codeview=False))
        summary = repro.check(first, second)
        self.assertIn(repro.sha256_file(first), summary)
        self.assertIn("no CodeView record", summary)

    def test_out_of_bounds_and_missing_inputs_fail_closed(self) -> None:
        with self.assertRaises(repro.ReproducibilityFailure):
            repro.sha256_file(self.root / "missing.exe")
        empty = self.write("empty.exe", b"")
        with self.assertRaises(repro.ReproducibilityFailure):
            repro.sha256_file(empty)


if __name__ == "__main__":
    unittest.main()
