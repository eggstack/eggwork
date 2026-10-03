"""Behavioural tests for the release qualification harness.

These cover the harness's own failure semantics, not the product. A harness
that misreports a transient upstream error as a product failure, or that treats
a clean 4xx as retryable, produces a closure record that is wrong in the
direction that matters: it either invents a defect or hides one.
"""

from __future__ import annotations

import io
import pathlib
import sys
import urllib.error
import urllib.request

import pytest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / "scripts"))

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
    return urllib.error.HTTPError("https://api.github.com", code, "boom", {}, None)  # type: ignore[arg-type]


@pytest.fixture(autouse=True)
def _no_sleep(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(harness.time, "sleep", lambda _: None)
    monkeypatch.setattr(harness, "api_token", lambda: "token")


def _responses(monkeypatch: pytest.MonkeyPatch, script: list[object]) -> list[object]:
    remaining = list(script)

    def _urlopen(request: urllib.request.Request, timeout: int = 0) -> object:
        if not remaining:
            raise AssertionError("urlopen was called more often than the test allows")
        outcome = remaining.pop(0)
        if isinstance(outcome, Exception):
            raise outcome
        return _Response(outcome if isinstance(outcome, bytes) else b"")

    monkeypatch.setattr(harness.urllib.request, "urlopen", _urlopen)
    return remaining


def test_download_writes_the_asset(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _responses(monkeypatch, [b"eggwork-bytes"])
    destination = tmp_path / "artifact.bin"
    harness.download_asset("eggstack/eggwork", 1, destination)
    assert destination.read_bytes() == b"eggwork-bytes"
    assert not destination.with_name(destination.name + ".partial").exists()


def test_transient_server_error_is_retried(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _responses(monkeypatch, [_http_error(500), _http_error(502), b"payload"])
    destination = tmp_path / "artifact.bin"
    harness.download_asset("eggstack/eggwork", 1, destination)
    assert destination.read_bytes() == b"payload"


def test_transient_rate_limit_is_retried(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _responses(monkeypatch, [_http_error(429), b"payload"])
    destination = tmp_path / "artifact.bin"
    harness.download_asset("eggstack/eggwork", 1, destination)
    assert destination.read_bytes() == b"payload"


def test_transport_error_is_retried(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _responses(monkeypatch, [urllib.error.URLError("connection reset"), b"payload"])
    destination = tmp_path / "artifact.bin"
    harness.download_asset("eggstack/eggwork", 1, destination)
    assert destination.read_bytes() == b"payload"


def test_client_error_is_not_retried(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # A 404 means the request is wrong. Retrying it would hide a harness bug.
    remaining = _responses(monkeypatch, [_http_error(404), b"never"])
    with pytest.raises(harness.QualificationFailure, match="HTTP 404"):
        harness.download_asset("eggstack/eggwork", 1, tmp_path / "artifact.bin")
    assert remaining == [b"never"]


def test_exhausted_retries_fail_loudly(tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _responses(monkeypatch, [_http_error(500)] * harness.TRANSIENT_ATTEMPTS)
    destination = tmp_path / "artifact.bin"
    with pytest.raises(harness.QualificationFailure, match="attempts"):
        harness.download_asset("eggstack/eggwork", 1, destination)
    assert not destination.exists()


def test_oversized_asset_is_refused_without_retrying(
    tmp_path: pathlib.Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(harness, "MAX_ASSET_BYTES", 4)
    _responses(monkeypatch, [b"far too many bytes"])
    with pytest.raises(harness.QualificationFailure, match="bounded size"):
        harness.download_asset("eggstack/eggwork", 1, tmp_path / "artifact.bin")
