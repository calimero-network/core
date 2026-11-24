"""
`Calimero.from_env()` and its `kv()`: which environment connects how (the
policy handed to the native client), and what each mistake says. The native
client is a stand-in recording how it was made (test_openapi_client.py's), so
no node is needed; test_sealed_refusals.py's stand-in node shows a mock quote
refused without `CALIMERO_ALLOW_UNATTESTED=1`.
"""

import pytest

import calimero_client_py.client as client_module
from calimero_client_py import Calimero, Client, CloudError
from calimero_client_py.products.kv.client import KvClient

from test_openapi_client import Native, answer
from test_sealed_refusals import node  # noqa: F401 (a fixture)

DEV = {
    "CALIMERO_NODE_URL": "http://127.0.0.1:1",
    "CALIMERO_ALLOW_UNATTESTED": "1",
    "CALIMERO_API_KEY": "cal_test",
    "CALIMERO_KV_STORE": "kv-1",
}
CLUSTER = {
    "CALIMERO_CLUSTER": "eu-shared-1",
    "CALIMERO_MIN_RELEASE": "2.3.109",
    "CALIMERO_API_KEY": "cal_test",
}


class Keyed(Native):
    """A stand-in native client that also records the API key it was given."""

    def use_api_key(self, key):
        self.api_key = key
        self.has_api_key = True


@pytest.fixture
def made(monkeypatch):
    """How the native client was made: `[(how, args, kwargs, native)]`."""
    made = []

    class Sealed:
        def __new__(cls, *args):
            native = Keyed(answer(200, {"index": 3, "result": "7", "events": []}))
            made.append(("new", args, {}, native))
            return native

        @staticmethod
        def connect(*args, **kwargs):
            native = Keyed(answer(200, {"index": 3, "result": None}))
            made.append(("connect", args, kwargs, native))
            return native

    monkeypatch.setattr(client_module, "_SealedClient", Sealed)
    return made


def test_a_dev_node_takes_a_mock_quote_and_the_api_key(made):
    kv = Calimero.from_env(DEV).kv()
    assert isinstance(kv, KvClient)
    assert kv.set("default", "flags/beta", '{"enabled":true}') == "7"
    ((how, args, _, native),) = made
    assert (how, args[:5]) == ("new", ("http://127.0.0.1:1", True, None, None, None))
    assert native.api_key == "cal_test"
    (sent,) = native.sent
    assert sent["path"] == "/apps/kv-1/intents"
    assert sent["body"] == {
        "method": "set",
        "args": {"bucket": "default", "key": "flags/beta", "value": '{"enabled":true}'},
    }


def test_an_attested_node_is_held_to_the_release_floor(made):
    env = {**DEV, "CALIMERO_ALLOW_UNATTESTED": "", "CALIMERO_MIN_RELEASE": "2.3.109"}
    Calimero.from_env(env)
    ((_, args, _, _),) = made
    assert args[:5] == (
        "http://127.0.0.1:1",
        False,
        None,
        ["locked-read-only"],
        "2.3.109",
    )


def test_a_cluster_is_connected_by_name_under_the_release_floor(made):
    calimero = Calimero.from_env(CLUSTER)
    ((how, args, kwargs, native),) = made
    assert (how, args) == ("connect", ("eu-shared-1",))
    assert kwargs["allow_mock"] is False
    assert kwargs["tee_profiles"] == ["locked-read-only"]
    assert kwargs["min_release"] == "2.3.109"
    assert kwargs["allow_debug_profiles"] is False
    assert native.api_key == "cal_test"
    assert isinstance(calimero.client, Client)
    assert calimero.kv("kv-1").get("b", "k") is None
    assert native.sent[0]["path"] == "/apps/kv-1/views"


@pytest.mark.parametrize(
    "profile, debug",
    [
        ("debug", True),
        ("debug-read-only", True),
        ("locked-read-only", False),
        ("staging", False),
    ],
)
def test_calimero_tee_profile_names_the_trusted_image_profile(made, profile, debug):
    Calimero.from_env({**CLUSTER, "CALIMERO_TEE_PROFILE": profile})
    ((_, _, kwargs, _),) = made
    assert kwargs["tee_profiles"] == [profile]
    assert kwargs["allow_debug_profiles"] is debug


def test_kv_binds_another_store(made):
    Calimero.from_env(DEV).kv("kv-2").set("b", "k", "v")
    assert made[0][3].sent[0]["path"] == "/apps/kv-2/intents"


def test_kv_without_a_store_names_the_variable(made):
    calimero = Calimero.from_env({**DEV, "CALIMERO_KV_STORE": ""})
    with pytest.raises(ValueError, match="CALIMERO_KV_STORE"):
        calimero.kv()


def test_reads_the_process_environment_by_default(made, monkeypatch):
    for name, value in DEV.items():
        monkeypatch.setenv(name, value)
    Calimero.from_env().kv().set("b", "k", "v")
    assert made[0][3].sent[0]["path"] == "/apps/kv-1/intents"


@pytest.mark.parametrize(
    "env, message",
    [
        ({"CALIMERO_API_KEY": "cal_test"}, "CALIMERO_CLUSTER"),
        (
            {**CLUSTER, "CALIMERO_NODE_URL": "http://127.0.0.1:1"},
            "CALIMERO_CLUSTER or CALIMERO_NODE_URL, not both",
        ),
        (
            {**CLUSTER, "CALIMERO_ALLOW_UNATTESTED": "1"},
            "CALIMERO_ALLOW_UNATTESTED.*CALIMERO_NODE_URL",
        ),
        (
            {**DEV, "CALIMERO_ALLOW_UNATTESTED": "true"},
            "CALIMERO_ALLOW_UNATTESTED must be 1",
        ),
        ({**CLUSTER, "CALIMERO_API_KEY": ""}, "CALIMERO_API_KEY"),
        ({**CLUSTER, "CALIMERO_MIN_RELEASE": ""}, "CALIMERO_MIN_RELEASE"),
        (
            {"CALIMERO_NODE_URL": "http://127.0.0.1:1", "CALIMERO_API_KEY": "k"},
            "CALIMERO_MIN_RELEASE",
        ),
        (
            {**DEV, "CALIMERO_TEE_PROFILE": "debug"},
            "CALIMERO_TEE_PROFILE needs CALIMERO_MIN_RELEASE",
        ),
    ],
)
def test_each_mistake_names_its_variable_before_connecting(made, env, message):
    with pytest.raises(ValueError, match=message):
        Calimero.from_env(env)
    assert made == []


def test_a_mock_quote_is_refused_without_allow_unattested(node):  # noqa: F811
    # The stand-in node attests with a mock quote, then refuses every request.
    env = {
        "CALIMERO_NODE_URL": node,
        "CALIMERO_MIN_RELEASE": "2.3.109",
        "CALIMERO_API_KEY": "cal_test",
        "CALIMERO_KV_STORE": "kv-1",
    }
    with pytest.raises(CloudError, match="mock quote refused"):
        Calimero.from_env(env).kv().whoami()
    with pytest.raises(CloudError) as e:
        Calimero.from_env({**env, "CALIMERO_ALLOW_UNATTESTED": "1"}).kv().whoami()
    assert e.value.args[1:3] == (413, "TOO_LARGE")
