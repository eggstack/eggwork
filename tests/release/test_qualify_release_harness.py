"""Behavioural tests for the release qualification harness.

These cover the harness's own failure semantics, not the product. A harness
that misreports a transient upstream error as a product failure, or that treats
a clean 4xx as retryable, produces a closure record that is wrong in the
direction that matters: it either invents a defect or hides one.
"""

from __future__ import annotations

import contextlib
import io
import os
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


class PublicReadTest(unittest.TestCase):
    """The publication boundary has to be reproducible by a consumer.

    The post-publication bootstrap evidence is only meaningful if the read that
    produced it is anonymous. A tokened run that reported `public` would make
    the distinction between "a consumer can install this" and "we can install
    this with a maintainer credential" unobservable, which is exactly the
    distinction the closure record needs.
    """

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = Path(self._tmp.name)
        self._previous_token = os.environ.get(harness.TOKEN_VARIABLE)
        self._previous_mode = harness.PUBLIC_MODE
        self.addCleanup(self._restore)

    def _restore(self) -> None:
        harness.PUBLIC_MODE = self._previous_mode
        if self._previous_token is None:
            os.environ.pop(harness.TOKEN_VARIABLE, None)
        else:
            os.environ[harness.TOKEN_VARIABLE] = self._previous_token

    def test_a_tokened_read_cannot_masquerade_as_public(self) -> None:
        os.environ[harness.TOKEN_VARIABLE] = "a-token"
        harness.PUBLIC_MODE = True
        with self.assertRaisesRegex(harness.QualificationFailure, "not public bootstrap"):
            harness.api_token()

    def test_a_public_read_needs_no_token(self) -> None:
        os.environ.pop(harness.TOKEN_VARIABLE, None)
        harness.PUBLIC_MODE = True
        self.assertEqual(harness.api_token(), "")

    def test_a_draft_read_still_requires_a_token(self) -> None:
        os.environ.pop(harness.TOKEN_VARIABLE, None)
        harness.PUBLIC_MODE = False
        with self.assertRaisesRegex(harness.QualificationFailure, "draft release"):
            harness.api_token()

    def test_public_is_rejected_for_the_privileged_stages(self) -> None:
        # Only the inventory check is a consumer-shaped read. Letting `--public`
        # qualify an install would quietly downgrade a privileged
        # pre-publication check to an anonymous one.
        for stage in ("install", "installer", "service", "update"):
            with self.subTest(stage=stage):
                with self.assertRaises(SystemExit):
                    with contextlib.redirect_stderr(io.StringIO()):
                        harness.main(
                            [
                                stage,
                                "--public",
                                "--installation-root",
                                self.root.name,
                            ]
                        )
