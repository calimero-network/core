"""A public link's visitor: `Client.use_visitor`, `Visitor` and `new_visitor_seed`.

The vector is the one core's client (`calimero_client::visitor`), mero-js
(`client/visitor.test.ts`), mero-swift and mero-kotlin check: Ed25519 signs
deterministically, so the credential is the same byte for byte. Calls through a
public link against a real node are the cluster suites' (core's e2e).
"""

import re

import pytest

from calimero_client_py import CloudClient, Visitor, new_visitor_seed

LINK = "d4205f1ac7bb3fe43278131b6e621ebf952f0a5f51a38505327572d863c46c42"
KEY = "fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618"
BEARER = (
    "vis1.pl_token.fd1724385aa0c75b64fb78cd602fa1d991fdebf76b13c58ed702eac835e9f618.1000."
    "9b5205e4deb4b534a612c3b03be773b60aea16192f2fb6d99496d194f2dd2591"
    "73173731e56a46ec7b37cc3d2d2482fe7be5d70c0fadda54c22f5ea2b8fa4e09"
)


def test_the_shared_vector_holds():
    v = Visitor("pl_token", bytes([9] * 32))
    assert v.link == LINK
    assert v.public_key == KEY
    assert v.bearer(1000) == BEARER


def test_a_bearer_defaults_to_now():
    bearer = Visitor("pl_token", bytes([9] * 32)).bearer()
    assert re.fullmatch(rf"vis1\.pl_token\.{KEY}\.\d+\.[0-9a-f]{{128}}", bearer)
    assert int(bearer.split(".")[3]) > 1_700_000_000


def test_two_seeds_are_two_visitors_of_one_link():
    one, two = Visitor("pl_token", bytes([1] * 32)), Visitor(
        "pl_token", bytes([2] * 32)
    )
    assert one.public_key != two.public_key
    assert one.link == two.link


def test_new_visitor_seed_is_32_random_bytes():
    seed = new_visitor_seed()
    assert isinstance(seed, bytes) and len(seed) == 32
    assert seed != new_visitor_seed()


def test_a_seed_that_is_not_32_bytes_is_refused():
    with pytest.raises(ValueError):
        Visitor("pl_token", bytes(31))
    with pytest.raises(ValueError):
        CloudClient("https://127.0.0.1:9", allow_mock=True).use_visitor(
            "pl_token", bytes(31)
        )


def test_use_visitor_answers_the_visitor_in_place_of_a_key():
    client = CloudClient("https://127.0.0.1:9", allow_mock=True)
    client.use_api_key("cal_key")
    assert client._native.has_api_key
    v = client.use_visitor("pl_token", bytes([9] * 32))
    assert (v.link, v.public_key) == (LINK, KEY)
    assert not client._native.has_api_key
    assert client.session() is None
