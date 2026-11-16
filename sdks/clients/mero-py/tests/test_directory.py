"""`Client.connect`: a cluster's nodes from the node directory.

A local HTTP stub plays the directory (`GET {directory}/{cluster}`); the nodes
it lists are closed ports, so nothing here needs a running cluster. Nothing is
sent to a node until the first request, which is what the failover test makes.
"""

import json
import socket
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

from calimero_client_py import Client, CloudError


def closed_port_url() -> str:
    """A URL on which nothing listens: connecting to it is refused."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return f"http://127.0.0.1:{s.getsockname()[1]}"


class Directory:
    """`listings` maps a cluster to its answer body; any other is 404."""

    def __init__(self):
        self.listings = {}
        self.hits = []
        directory = self

        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                cluster = self.path.rsplit("/", 1)[-1]
                directory.hits.append(cluster)
                body = directory.listings.get(cluster)
                if body is None:
                    self.send_response(404)
                    self.end_headers()
                    return
                data = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *args):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}/api/v2/directory"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


@pytest.fixture
def directory():
    d = Directory()
    yield d
    d.close()


def listing(cluster, nodes, refresh=60):
    return {"cluster": cluster, "nodes": nodes, "refreshAfterSeconds": refresh}


def test_connect_is_a_static_constructor():
    assert callable(Client.connect)


def test_the_zone_comes_first(directory):
    near, far = closed_port_url(), closed_port_url()
    directory.listings["eu-1"] = listing(
        "eu-1",
        [
            {"id": 1, "url": far, "zone": "europe-west4-a", "weight": 100},
            {"id": 2, "url": near, "zone": "europe-west4-b", "weight": 100},
        ],
    )
    client = Client.connect(
        "eu-1", allow_mock=True, directory=directory.url, zone="europe-west4-b"
    )
    assert client.node_url == near
    assert client.node_urls == [near, far]
    assert directory.hits == ["eu-1"]


def test_unusable_entries_are_dropped(directory):
    good = closed_port_url()
    directory.listings["c"] = listing(
        "c",
        [
            {"id": 1, "url": "ftp://x.example", "weight": 100},
            {"id": 2, "url": "https://zero.example", "weight": 0},
            {"id": 3, "url": "https://half.example", "weight": 0.5},
            {"id": 4, "url": good, "weight": 1},
        ],
    )
    client = Client.connect("c", allow_mock=True, directory=directory.url)
    assert client.node_urls == [good]


def test_an_unknown_cluster_is_a_clear_error(directory):
    with pytest.raises(CloudError) as e:
        Client.connect("nowhere", allow_mock=True, directory=directory.url)
    message, status, code, _ = e.value.args
    assert (status, code) == (404, "DIRECTORY")
    assert "nowhere" in message


def test_a_listing_with_no_usable_node_is_an_error(directory):
    directory.listings["empty"] = listing("empty", [])
    directory.listings["bad"] = listing(
        "bad", [{"id": 1, "url": "https://x.example", "weight": -1}]
    )
    for cluster in ("empty", "bad"):
        with pytest.raises(CloudError) as e:
            Client.connect(cluster, allow_mock=True, directory=directory.url)
        assert "no usable node" in e.value.args[0]
        assert e.value.args[2] == "DIRECTORY"


def test_an_unreachable_directory_is_an_error():
    with pytest.raises(CloudError) as e:
        Client.connect("c", allow_mock=True, directory=closed_port_url())
    assert e.value.args[1:3] == (0, "DIRECTORY")


def test_unreachable_nodes_are_each_tried_once_then_the_directory_is_asked_again(
    directory,
):
    urls = [closed_port_url() for _ in range(3)]
    directory.listings["c"] = listing(
        "c", [{"id": i, "url": u, "weight": 1} for i, u in enumerate(urls)]
    )
    client = Client.connect("c", allow_mock=True, directory=directory.url)
    first = client.node_urls
    assert sorted(first) == sorted(urls)
    with pytest.raises(CloudError) as e:
        client.get("apps")
    assert e.value.args[1:3] == (0, "TRANSPORT")
    # Each node once: the client ends on the last it tried.
    assert client.node_url == first[2]
    assert directory.hits == ["c"]
    # The failover makes a listing due: the next request asks again, and the
    # node in use stays first.
    with pytest.raises(CloudError):
        client.get("apps")
    assert directory.hits == ["c", "c"]
    assert client.node_urls[0] == first[2]
    assert sorted(client.node_urls) == sorted(urls)
