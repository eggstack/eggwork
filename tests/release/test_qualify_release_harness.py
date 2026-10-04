"""Behavioural tests for the release qualification harness.

These cover the harness's own failure semantics, not the product. A harness
that misreports a transient upstream error as a product failure, or that treats
a clean 4xx as retryable, produces a closure record that is wrong in the
direction that matters: it either invents a defect or hides one.
"""

from __future__ import annotations

import io
import sys
import tempfile
import unittest
import urllib.error
import urllib.request
from contextlib import contextmanager
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "scripts"))

import qualify_release as harness  # noqa: E402


class _Response:
    def __init__(self, payload: bytes) -> None:
        self._stream = io.BytesIO(payload)

    def read(self, size: int) -> bytes:
        return self._stream.read(size)

    def __enter__(self) -> "_Response":
        return self

    def __exit__(self, *_: object) -> None:
        return None


def _http_error(code: int) -> urllib.error.HTTPError:
    return urllib.error.HTTPError(  # type: ignore[arg-type]
        "https://api.github.com", code, "boom", {}, None
    )


@contextmanager
def _scripted(outcomes: list[object]):
    """Serve `outcomes` to `urlopen`, failing loudly if the harness over-calls."""
    remaining = list(outcomes)

    def _urlopen(request: urllib.request.Request, timeout: int = 0) -> object:
        if not remaining:
            raise AssertionError("urlopen was called more often than the test allows")
        outcome = remaining.pop(0)
        if isinstance(outcome, Exception):
            raise outcome
        return _Response(outcome if isinstance(outcome, bytes) else b"")

    original_urlopen = harness.urllib.request.urlopen
    original_sleep = harness.time.sleep
    original_token = harness.api_token
    original_log = harness.log
    harness.urllib.request.urlopen = _urlopen  # type: ignore[assignment]
    harness.log = lambda _: None
    harness.time.sleep = lambda _: None
    harness.api_token = lambda: "token"  # type: ignore[assignment]
    try:
        yield remaining
    finally:
        harness.urllib.request.urlopen = original_urlopen  # type: ignore[assignment]
        harness.time.sleep = original_sleep
        harness.api_token = original_token  # type: ignore[assignment]
        harness.log = original_log


class AssetDownloadTest(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name)
        self.destination = self.root / "artifact.bin"

    def _partial(self) -> Path:
        return self.destination.with_name(self.destination.name + ".partial")

    def test_download_writes_the_asset(self) -> None:
        with _scripted([b"eggwork-bytes"]):
            harness.download_asset("eggstack/eggwork", 1, self.destination)
        self.assertEqual(self.destination.read_bytes(), b"eggwork-bytes")
        self.assertFalse(self._partial().exists())

    def test_transient_server_error_is_retried(self) -> None:
        with _scripted([_http_error(500), _http_error(502), b"payload"]):
            harness.download_asset("eggstack/eggwork", 1, self.destination)
        self.assertEqual(self.destination.read_bytes(), b"payload")

    def test_transient_rate_limit_is_retried(self) -> None:
        with _scripted([_http_error(429), b"payload"]):
            harness.download_asset("eggstack/eggwork", 1, self.destination)
        self.assertEqual(self.destination.read_bytes(), b"payload")

    def test_transport_error_is_retried(self) -> None:
        with _scripted([urllib.error.URLError("connection reset"), b"payload"]):
            harness.download_asset("eggstack/eggwork", 1, self.destination)
        self.assertEqual(self.destination.read_bytes(), b"payload")

    def test_client_error_is_not_retried(self) -> None:
        # A 404 means the request itself is wrong. Retrying it would hide a
        # harness bug behind a delay instead of reporting it.
        with _scripted([_http_error(404), b"never"]):
            with self.assertRaisesRegex(harness.QualificationFailure, "HTTP 404"):
                harness.download_asset("eggstack/eggwork", 1, self.destination)

    def test_exhausted_retries_fail_loudly(self) -> None:
        with _scripted([_http_error(500)] * harness.TRANSIENT_ATTEMPTS):
            with self.assertRaisesRegex(harness.QualificationFailure, "attempts"):
                harness.download_asset("eggstack/eggwork", 1, self.destination)
        self.assertFalse(self.destination.exists())
        self.assertFalse(self._partial().exists())

    def test_oversized_asset_is_refused_without_retrying(self) -> None:
        original_limit = harness.MAX_ASSET_BYTES
        harness.MAX_ASSET_BYTES = 4
        try:
            with _scripted([b"far too many bytes"]):
                with self.assertRaisesRegex(harness.QualificationFailure, "bounded size"):
                    harness.download_asset("eggstack/eggwork", 1, self.destination)
        finally:
            harness.MAX_ASSET_BYTES = original_limit
        self.assertFalse(self.destination.exists())


if __name__ == "__main__":  # pragma: no cover
    unittest.main()
