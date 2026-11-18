"""
`ca_bundle`: roots the client trusts for TLS on top of the built-in public
ones, for nodes whose certificates come from a private or staging CA (Let's
Encrypt's staging roots on a non-production cluster).

A stand-in node (no merod needed) serves its mock attestation over HTTPS with
a certificate chained to a throwaway test root, and refuses the sealed
handshake with `413 TOO_LARGE`: reaching that refusal means the TLS handshake,
the attestation fetch and the sealed POST all went through.
"""

import datetime
import ipaddress
import socket
import ssl
import threading
from http.server import ThreadingHTTPServer
from urllib.parse import urlparse

import pytest
from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID

from calimero_client_py import Client, CloudClient, CloudError
from test_sealed_refusals import StandInNode


def _name(cn: str) -> x509.Name:
    return x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, cn)])


def _cert(subject, issuer, public_key, signing_key, ca: bool, sans=None):
    now = datetime.datetime.now(datetime.timezone.utc)
    b = (
        x509.CertificateBuilder()
        .subject_name(subject)
        .issuer_name(issuer)
        .public_key(public_key)
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(minutes=5))
        .not_valid_after(now + datetime.timedelta(days=1))
        .add_extension(
            x509.BasicConstraints(ca=ca, path_length=None),
            critical=True,
        )
    )
    if ca:
        b = b.add_extension(
            x509.KeyUsage(
                digital_signature=True,
                content_commitment=False,
                key_encipherment=False,
                data_encipherment=False,
                key_agreement=False,
                key_cert_sign=True,
                crl_sign=True,
                encipher_only=False,
                decipher_only=False,
            ),
            critical=True,
        )
    if sans:
        b = b.add_extension(x509.SubjectAlternativeName(sans), critical=False)
    return b.sign(signing_key, hashes.SHA256())


@pytest.fixture(scope="module")
def pki(tmp_path_factory):
    """A throwaway root, an intermediate under it and a leaf for 127.0.0.1,
    as the staging chain is (leaf <- intermediate <- root). Returns the root's
    PEM path and the server's chain and key paths."""
    d = tmp_path_factory.mktemp("pki")
    root_key = ec.generate_private_key(ec.SECP256R1())
    root = _cert(
        _name("(TEST) Throwaway Root"),
        _name("(TEST) Throwaway Root"),
        root_key.public_key(),
        root_key,
        ca=True,
    )
    mid_key = ec.generate_private_key(ec.SECP256R1())
    mid = _cert(
        _name("(TEST) Throwaway Intermediate"),
        root.subject,
        mid_key.public_key(),
        root_key,
        ca=True,
    )
    leaf_key = ec.generate_private_key(ec.SECP256R1())
    leaf = _cert(
        _name("127.0.0.1"),
        mid.subject,
        leaf_key.public_key(),
        mid_key,
        ca=False,
        sans=[
            x509.IPAddress(ipaddress.ip_address("127.0.0.1")),
            x509.DNSName("localhost"),
        ],
    )
    pem = serialization.Encoding.PEM
    root_pem = d / "root.pem"
    root_pem.write_bytes(root.public_bytes(pem))
    chain = d / "chain.pem"
    chain.write_bytes(leaf.public_bytes(pem) + mid.public_bytes(pem))
    key = d / "key.pem"
    key.write_bytes(
        leaf_key.private_bytes(
            pem,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        )
    )
    return {"root": str(root_pem), "chain": str(chain), "key": str(key)}


class TlsStandIn(StandInNode):
    """The stand-in node, which also answers the node directory's
    `GET /directory/{cluster}` with itself as the cluster's only node."""

    def do_GET(self):
        path = urlparse(self.path).path
        if path.startswith("/directory/"):
            host, port = self.server.server_address[:2]
            return self.answer(
                200,
                {
                    "cluster": path.rsplit("/", 1)[1],
                    "nodes": [
                        {
                            "id": 1,
                            "url": f"https://{host}:{port}",
                            "zone": "z",
                            "weight": 1,
                        }
                    ],
                    "refreshAfterSeconds": 60,
                },
            )
        return super().do_GET()


@pytest.fixture
def node(pki):
    server = ThreadingHTTPServer(("127.0.0.1", 0), TlsStandIn)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(pki["chain"], pki["key"])
    server.socket = ctx.wrap_socket(server.socket, server_side=True)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"https://127.0.0.1:{server.server_address[1]}"
    server.shutdown()
    server.server_close()


def _reaches_the_node(client):
    """The stand-in's sealed refusal: TLS, attestation and the POST went through."""
    with pytest.raises(CloudError) as e:
        client.json.apps.blob_upload(app="app", body=b"x" * 16)
    _message, status, code, _details = e.value.args
    assert (status, code) == (413, "TOO_LARGE")


def test_without_ca_bundle_a_node_under_a_private_root_is_refused(node):
    client = CloudClient(node, allow_mock=True)
    with pytest.raises(CloudError) as e:
        client.json.apps.blob_upload(app="app", body=b"x" * 16)
    message, status, code, _details = e.value.args
    assert (status, code) == (0, "TRANSPORT")
    assert "UnknownIssuer" in message


def test_with_ca_bundle_a_node_under_that_root_is_reached(node, pki):
    _reaches_the_node(CloudClient(node, allow_mock=True, ca_bundle=pki["root"]))


def test_ca_bundle_takes_a_path_object(node, pki):
    import pathlib

    _reaches_the_node(
        CloudClient(node, allow_mock=True, ca_bundle=pathlib.Path(pki["root"]))
    )


def test_ca_bundle_covers_the_node_directory_and_the_nodes_it_lists(node, pki):
    client = Client.connect(
        "c", directory=f"{node}/directory", allow_mock=True, ca_bundle=pki["root"]
    )
    assert client.node_url == node
    _reaches_the_node(client)


def test_without_ca_bundle_the_directory_under_a_private_root_is_refused(node):
    with pytest.raises(CloudError) as e:
        Client.connect("c", directory=f"{node}/directory", allow_mock=True)
    message, status, code, _details = e.value.args
    assert (status, code) == (0, "DIRECTORY")


def test_a_missing_ca_bundle_fails_at_construction(tmp_path):
    missing = tmp_path / "nope.pem"
    with pytest.raises(FileNotFoundError) as e:
        CloudClient("https://127.0.0.1:9", allow_mock=True, ca_bundle=str(missing))
    assert "ca_bundle" in str(e.value) and str(missing) in str(e.value)


def test_a_ca_bundle_with_no_certificate_fails_at_construction(tmp_path):
    empty = tmp_path / "empty.pem"
    empty.write_text("not a certificate\n")
    with pytest.raises(ValueError) as e:
        CloudClient("https://127.0.0.1:9", allow_mock=True, ca_bundle=str(empty))
    assert "ca_bundle" in str(e.value) and "no PEM certificate" in str(e.value)


def test_a_ca_bundle_with_a_broken_certificate_fails_at_construction(tmp_path):
    broken = tmp_path / "broken.pem"
    broken.write_text(
        "-----BEGIN CERTIFICATE-----\nAAAAAAAAAAAA\n-----END CERTIFICATE-----\n"
    )
    with pytest.raises(ValueError) as e:
        CloudClient("https://127.0.0.1:9", allow_mock=True, ca_bundle=str(broken))
    assert "ca_bundle" in str(e.value)


def test_connect_checks_the_ca_bundle_before_the_directory(tmp_path):
    missing = tmp_path / "nope.pem"
    with pytest.raises(FileNotFoundError):
        Client.connect("c", directory="https://127.0.0.1:9", ca_bundle=str(missing))


def _online(host: str) -> bool:
    try:
        socket.create_connection((host, 443), timeout=3).close()
        return True
    except OSError:
        return False


@pytest.mark.skipif(not _online("example.com"), reason="no network")
def test_ca_bundle_adds_to_the_public_roots(pki):
    """A site under a public root still verifies with `ca_bundle` set: it is
    not a node, so the attestation fails, but not on its certificate."""
    for ca_bundle in (None, pki["root"]):
        client = CloudClient(
            "https://example.com", allow_mock=True, ca_bundle=ca_bundle
        )
        with pytest.raises(CloudError) as e:
            client.json.apps.blob_upload(app="app", body=b"x")
        message = e.value.args[0]
        assert "certificate" not in message.lower(), message
        assert "UnknownIssuer" not in message, message


def test_the_bundle_mdma_builds_parses(tmp_path, pki):
    """A bundle of several roots (as certifi plus the staging roots) loads:
    every certificate in it is added."""
    bundle = tmp_path / "bundle.pem"
    root = open(pki["root"]).read()
    bundle.write_text(root + "\n# a comment between certificates\n" + root)
    CloudClient("https://127.0.0.1:9", allow_mock=True, ca_bundle=str(bundle))
