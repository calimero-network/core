"""The streams' `timeout`: any float is a wait or a `ValueError`, never a
panic (which Python sees as a `BaseException` most handlers let through)."""

import socket

import pytest

from calimero_client_py import Client, CloudError


def closed_port_url() -> str:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return f"http://127.0.0.1:{s.getsockname()[1]}"


@pytest.mark.parametrize("timeout", [float("inf"), 1e300])
@pytest.mark.parametrize("stream", ["events", "presence_stream"])
def test_a_huge_timeout_waits_rather_than_panics(stream, timeout):
    client = Client(closed_port_url(), allow_mock=True)
    # The node is out of reach, so the stream fails as any request would.
    with pytest.raises(CloudError):
        getattr(client, stream)("app", timeout=timeout)


@pytest.mark.parametrize("stream", ["events", "presence_stream"])
def test_a_nan_timeout_is_a_value_error(stream):
    client = Client(closed_port_url(), allow_mock=True)
    with pytest.raises(ValueError):
        getattr(client, stream)("app", timeout=float("nan"))
