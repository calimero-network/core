"""
How CloudClient reports a refusal the node sends in the clear, before any
envelope is opened. A stand-in node (no merod needed) attests with a mock
quote and refuses the sealed handshake the way merod refuses a request past
its 64 MiB sealed cap.
"""

import base64
import hashlib
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

import pytest

from calimero_client_py import CloudClient, CloudError

MOCK_QUOTE_HEADER = b"MOCK_TDX_QUOTE_V1"
TRANSPORT_BINDING_DOMAIN = b"calimero.tee-attest.transport-key.v1"
TOO_LARGE = "a sealed request is limited to 64 MiB"


class StandInNode(BaseHTTPRequestHandler):
    transport_key = os.urandom(32)

    def log_message(self, *_args):
        pass

    def answer(self, status: int, body: dict):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        url = urlparse(self.path)
        if url.path != "/attestation":
            return self.answer(404, {"error": {"code": "not_found"}})
        nonce = bytes.fromhex(parse_qs(url.query)["nonce"][0])
        app_hash = bytes(32)
        binding = hashlib.sha256(
            TRANSPORT_BINDING_DOMAIN + app_hash + self.transport_key
        ).digest()
        quote = MOCK_QUOTE_HEADER + nonce + binding
        self.answer(
            200,
            {
                "quote": base64.b64encode(quote).decode(),
                "transportKey": self.transport_key.hex(),
                "appHash": app_hash.hex(),
                "nonce": nonce.hex(),
                "release": {"version": "0.0.0", "profile": "mock"},
                "cluster": "stand-in",
                "node": 0,
                "mock": True,
            },
        )

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        self.rfile.read(length)
        self.answer(413, {"error": {"code": "too_large", "message": TOO_LARGE}})


@pytest.fixture
def node():
    server = ThreadingHTTPServer(("127.0.0.1", 0), StandInNode)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()
    server.server_close()


def test_a_request_past_the_sealed_cap_is_413_too_large(node):
    client = CloudClient(node, allow_mock=True)
    with pytest.raises(CloudError) as e:
        client.json.apps.blob_upload(app="app", body=b"x" * 16)
    message, status, code, _details = e.value.args
    assert (status, code) == (413, "TOO_LARGE")
    assert message == f"413 TOO_LARGE: {TOO_LARGE}"
