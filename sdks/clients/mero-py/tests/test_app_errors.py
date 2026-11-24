"""
A `422 APP_ERROR` the client raises reads as a sentence for the refusals every
app shares (a KV's `NotGranted`, a `uses` link's `NotLinked`), built from the
structured `details`; any other refusal keeps the node's message.

Runs on the stand-in native client of test_openapi_client.py: no node.
"""

import json

import pytest

from calimero_client_py import CloudError
from calimero_client_py.products.kv.client import KvClient

from test_openapi_client import Native, client_on, refusal

STAND_IN = "app_error (stand-in)"


@pytest.mark.parametrize(
    "details, sentence",
    [
        (
            {"code": "NotGranted", "fields": {"bucket": "default", "need": "read"}},
            "You can't view bucket default.",
        ),
        (
            {"code": "NotGranted", "fields": {"bucket": "default", "need": "write"}},
            "You can't edit bucket default.",
        ),
        (
            {"code": "NotGranted", "fields": {"bucket": "b", "need": "admin"}},
            "You can't manage bucket b.",
        ),
        (
            {"code": "NotLinked", "fields": {"name": "kv"}},
            "This app isn't connected to a KV store yet.",
        ),
        (
            {"code": "NotLinked", "fields": {"name": "calendar"}},
            "This app isn't connected to its calendar yet.",
        ),
        # An older node nests the app's error under `details.error`.
        (
            {
                "error": {
                    "code": "NotGranted",
                    "fields": {"bucket": "b", "need": "read"},
                }
            },
            "You can't view bucket b.",
        ),
        ({"code": "NotGranted", "fields": {"bucket": "b", "need": "purge"}}, STAND_IN),
        ({"code": "NotGranted", "fields": {"bucket": "b", "need": ["read"]}}, STAND_IN),
        ({"code": "NotGranted", "fields": {"need": "read"}}, STAND_IN),
        ({"code": "NotLinked", "fields": {}}, STAND_IN),
        ({"code": "NotLinked", "fields": ["kv"]}, STAND_IN),
        ({"code": "Conflict", "fields": {"current": "3"}}, STAND_IN),
        (None, STAND_IN),
    ],
)
def test_an_app_error_reads_as_a_sentence_where_its_fields_make_one(details, sentence):
    native = Native(refusal(422, "APP_ERROR", details, message=STAND_IN))
    kv = KvClient(client_on(native), "kv-1")
    with pytest.raises(CloudError) as e:
        kv.set("default", "flags/beta", "{}")
    assert e.value.args[:3] == (f"422 APP_ERROR: {sentence}", 422, "APP_ERROR")
    assert json.loads(e.value.args[3]) == details


def test_another_status_keeps_the_nodes_message():
    details = {"code": "NotGranted", "fields": {"bucket": "b", "need": "read"}}
    native = Native(refusal(403, "FORBIDDEN", details, message=STAND_IN))
    with pytest.raises(CloudError) as e:
        KvClient(client_on(native), "kv-1").whoami()
    assert e.value.args[0] == f"403 FORBIDDEN: {STAND_IN}"
