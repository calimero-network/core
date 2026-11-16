"""
The generated client's path: `Client.api()` runs the calls generated from
core's OpenAPI document (`calimero_client_py.openapi`) on the sealed
transport (`calimero_client_py.sealed`), which answers the node's step-ups.

A stand-in for the native `SealedClient` records what the transport hands
it and answers what each test scripts, so these run without a node; the
channel itself is exercised against a stand-in node in
test_sealed_refusals.py and against merod in the cluster tests.
"""

import asyncio
import base64
import hashlib
import json
from pathlib import Path


from calimero_client_py import Client, CloudError, LoginRequired, SoftPasskey, WrongNode
from calimero_client_py.openapi.api.apps import intents
from calimero_client_py.openapi.api.organizations import org_create, org_set_member
from calimero_client_py.openapi.api.ops import ops_set_replicas
from calimero_client_py.openapi.models import (
    IntentsBody,
    OrgCreateBody,
    OrgSetMemberBody,
    OpsSetReplicasBody,
)
from calimero_client_py.sealed import error_from

ROOT = Path(__file__).resolve().parents[1]


def _decoded(body):
    try:
        return json.loads(body) if body else None
    except ValueError:
        return None


def answer(status, body=None):
    content = b"" if body is None else json.dumps(body).encode()
    return status, [("content-type", "application/json")], content


def refusal(status, code, details=None, message="refused"):
    return answer(
        status,
        {"status": status, "error": code, "message": message, "details": details},
    )


class Native:
    """Records each request and answers the next scripted answer."""

    def __init__(self, *answers, api_key=False, operator_seed=None):
        self.answers = list(answers)
        self.sent = []
        self.has_api_key = api_key
        self.operator_seed = operator_seed
        self.node_url = "http://node-1"
        self.node_urls = ["http://node-1"]
        self.sessions = []

    def _next(self, record):
        self.sent.append(record)
        return self.answers.pop(0)

    def request(self, method, path, headers, body, mode="run", retry=False):
        return self._next(
            {
                "at": None,
                "method": method,
                "path": path,
                "headers": dict((k.lower(), v) for k, v in headers),
                "body": _decoded(body),
                "raw": body,
                "mode": mode,
                "retry": retry,
            }
        )

    def request_at(self, url, method, path, headers, body):
        return self._next(
            {
                "at": url,
                "method": method,
                "path": path,
                "headers": dict((k.lower(), v) for k, v in headers),
                "body": _decoded(body),
                "raw": body,
                "mode": None,
                "retry": False,
            }
        )

    def operator_sign(self, message, seed=None):
        key = seed or self.operator_seed
        if key is None:
            return None
        return hashlib.sha256(key.encode() + message).hexdigest()

    def use_session(self, session):
        self.sessions.append(dict(session))


def client_on(native):
    client = Client.__new__(Client)
    client._init(native)
    return client


def challenge_details(challenge=b"the node's challenge"):
    return {
        "challengeId": "ch-1",
        "challenge": base64.urlsafe_b64encode(challenge).rstrip(b"=").decode(),
        "rpId": "localhost",
        "allowCredentials": [],
    }


# ------------------------------------------------------------ the transport


def test_a_generated_call_goes_through_the_native_channel():
    native = Native(answer(200, {"orgId": "o1"}))
    api = client_on(native).api()
    r = org_create.sync_detailed(client=api, body=OrgCreateBody(name="Acme"))
    assert r.status_code == 200
    assert json.loads(r.content) == {"orgId": "o1"}
    (sent,) = native.sent
    assert (sent["method"], sent["path"], sent["body"]) == (
        "POST",
        "/orgs",
        {"name": "Acme"},
    )
    assert sent["mode"] == "run"
    # The credential is the native client's: nothing the generated client
    # sets reaches the node as one.
    assert "authorization" not in sent["headers"]


def test_a_read_is_repeatable():
    native = Native(answer(200, {"node": 1}))
    client_on(native).api().get_httpx_client().get("/ops/status")
    assert native.sent[0]["method"] == "GET"
    assert native.sent[0]["mode"] == "repeatable"


def test_a_path_segment_cannot_reach_another_route():
    native = Native(answer(200, {}))
    client_on(native).api().get_httpx_client()
    client_on(native)._op("app_get", "a/../ops/status")
    assert native.sent[0]["path"] == "/apps/a%2F..%2Fops%2Fstatus"


def test_a_confirm_required_is_answered_with_the_passkey_and_the_body_kept():
    passkey = SoftPasskey()
    native = Native(
        refusal(412, "CONFIRM_REQUIRED", challenge_details()),
        answer(200, {}),
    )
    body = OrgSetMemberBody.from_dict({"account": "acc", "role": "admin"})
    r = org_set_member.sync_detailed(
        "o1", client=client_on(native).api(passkey), body=body
    )
    assert r.status_code == 200
    first, second = native.sent
    assert first["body"] == {"account": "acc", "role": "admin"}
    assert second["path"] == first["path"]
    assert second["body"]["account"] == "acc" and second["body"]["role"] == "admin"
    assert second["body"]["confirm"]["challengeId"] == "ch-1"
    # The assertion signs the node's challenge, as core's step-up does.
    expected = passkey.get(b"the node's challenge")
    assertion = second["body"]["confirm"]["assertion"]
    assert assertion["credentialId"] == expected["credentialId"]
    client_data = json.loads(
        base64.urlsafe_b64decode(assertion["clientDataJson"] + "==")
    )
    assert client_data["challenge"] == challenge_details()["challenge"]


def test_a_confirm_required_without_a_passkey_is_the_answer():
    native = Native(refusal(412, "CONFIRM_REQUIRED", challenge_details()))
    body = OrgSetMemberBody.from_dict({"account": "acc", "role": "admin"})
    r = org_set_member.sync_detailed("o1", client=client_on(native).api(), body=body)
    assert r.status_code == 412
    assert len(native.sent) == 1


def test_any_other_answer_is_returned_as_it_came():
    native = Native(refusal(404, "NOT_FOUND"))
    body = OrgSetMemberBody.from_dict({"account": "acc", "role": "admin"})
    r = org_set_member.sync_detailed(
        "o1", client=client_on(native).api(SoftPasskey()), body=body
    )
    assert r.status_code == 404
    assert len(native.sent) == 1


# A log's status, as `POST /ops/apps/{app}/replicas` answers it.
STATUS = {
    "group": 7,
    "leader": 1,
    "is_leader": True,
    "term": 2,
    "last_log": 10,
    "applied": 10,
    "voters": [1, 2, 3],
    "learners": [],
}


def test_an_operator_signature_is_answered_with_the_operator_key():
    message = b"calimero.ops.v1 statement"
    details = {
        "subject": "ops",
        "method": "set_replicas",
        "args": {},
        "nonce": "n-1",
        "cluster": "c",
        "message": base64.b64encode(message).decode(),
    }
    native = Native(
        refusal(428, "OPERATOR_SIGNATURE_REQUIRED", details),
        answer(200, {"status": STATUS, "unchanged": False}),
        operator_seed="seed",
    )
    api = client_on(native).api(operator_signed=True)
    r = ops_set_replicas.sync_detailed(
        "app", client=api, body=OpsSetReplicasBody(replicas=[1, 2, 3])
    )
    assert r.status_code == 200
    proof = native.sent[1]["body"]["operatorProof"]
    assert proof == {"nonce": "n-1", "signature": native.operator_sign(message)}
    assert native.sent[1]["body"]["replicas"] == [1, 2, 3]


def test_without_an_operator_key_the_428_is_the_answer():
    details = {"nonce": "n", "message": base64.b64encode(b"m").decode()}
    native = Native(refusal(428, "OPERATOR_SIGNATURE_REQUIRED", details))
    api = client_on(native).api(operator_signed=True)
    r = ops_set_replicas.sync_detailed(
        "app", client=api, body=OpsSetReplicasBody(replicas=[1, 2, 3])
    )
    assert r.status_code == 428
    assert len(native.sent) == 1


def test_not_leader_is_followed_and_the_step_up_goes_to_the_leader():
    details = {"nonce": "n-1", "message": base64.b64encode(b"m").decode()}
    native = Native(
        refusal(421, "NOT_LEADER", {"leaderUrl": "http://node-2"}),
        refusal(428, "OPERATOR_SIGNATURE_REQUIRED", details),
        answer(200, {"status": STATUS, "unchanged": False}),
        api_key=True,
        operator_seed="seed",
    )
    api = client_on(native).api(operator_signed=True, follow_leader=True)
    r = ops_set_replicas.sync_detailed(
        "app", client=api, body=OpsSetReplicasBody(replicas=[1, 2, 4])
    )
    assert r.status_code == 200
    assert [s["at"] for s in native.sent] == [None, "http://node-2", "http://node-2"]
    assert "operatorProof" in native.sent[2]["body"]


def test_not_leader_without_an_api_key_is_the_answer():
    native = Native(refusal(421, "NOT_LEADER", {"leaderUrl": "http://node-2"}))
    api = client_on(native).api(follow_leader=True)
    r = ops_set_replicas.sync_detailed(
        "app", client=api, body=OpsSetReplicasBody(replicas=[1, 2, 4])
    )
    assert r.status_code == 421
    assert len(native.sent) == 1


def test_a_call_is_retried_and_repeatable():
    native = Native(
        answer(
            200,
            {
                "index": 1,
                "result": None,
                "events": [],
                "receipt": {
                    "app": "app",
                    "index": 1,
                    "resultHash": "h",
                    "node": 1,
                    "key": "k",
                    "signature": "s",
                },
            },
        )
    )
    api = client_on(native).api(repeatable=True, retry_unavailable=True)
    intents.sync_detailed(
        "app", client=api, body=IntentsBody(method="get", idempotency_key="k")
    )
    assert native.sent[0]["mode"] == "repeatable"
    assert native.sent[0]["retry"] is True
    assert native.sent[0]["body"]["idempotencyKey"] == "k"


def test_the_async_calls_run_on_the_same_channel():
    native = Native(answer(200, {"orgId": "o1"}))
    api = client_on(native).api()

    async def go():
        return await org_create.asyncio_detailed(
            client=api, body=OrgCreateBody(name="Acme")
        )

    r = asyncio.run(go())
    assert r.status_code == 200
    assert native.sent[0]["body"] == {"name": "Acme"}


def test_a_typed_answer_is_parsed_by_the_generated_models():
    native = Native(answer(200, {"orgId": "o1"}))
    parsed = org_create.sync(
        client=client_on(native).api(), body=OrgCreateBody(name="A")
    )
    assert parsed.org_id == "o1"


# ------------------------------------------------------------ errors


def test_errors_keep_the_arguments_the_client_raised():
    e = error_from(403, json.dumps({"error": "FORBIDDEN", "message": "no"}).encode())
    assert type(e) is CloudError
    assert e.args == ("403 FORBIDDEN: no", 403, "FORBIDDEN", "null")
    e = error_from(
        412,
        json.dumps(
            {"error": "CONFIRM_REQUIRED", "message": "m", "details": {"a": [1, "é"]}}
        ).encode(),
    )
    assert e.args[3] == '{"a":[1,"é"]}'
    assert isinstance(error_from(401, b'{"error":"UNAUTHENTICATED"}'), LoginRequired)
    assert isinstance(error_from(421, b'{"error":"WRONG_NODE"}'), WrongNode)
    # Not the error shape: `ERROR`, as core's client reads it.
    assert error_from(502, b"bad gateway").args == ("502 ERROR: ", 502, "ERROR", "null")


# ------------------------------------------------------------ the vendored document


def test_the_vendored_document_is_the_one_recorded():
    source = json.loads((ROOT / "openapi" / "source.json").read_text())
    data = (ROOT / "openapi" / "openapi.json").read_bytes()
    assert hashlib.sha256(data).hexdigest() == source["sha256"]
    assert len(source["commit"]) == 40


def test_the_generated_package_names_its_source():
    import calimero_client_py.openapi as generated

    source = json.loads((ROOT / "openapi" / "source.json").read_text())
    assert source["commit"] in (generated.__doc__ or "")


# ------------------------------------------------------------ rc4: the routes are the facade's


def test_the_route_methods_are_gone():
    # 1.0.0rc4 removed rc3's method per route; the facade has each route.
    for name in ("create_org", "call", "view", "ops_status", "packages"):
        assert not callable(getattr(Client, name, None)) or isinstance(
            getattr(Client, name), type
        ), name


def test_what_is_not_a_route_is_kept():
    for name in (
        "sign_up",
        "sign_in",
        "add_passkey",
        "events",
        "presence_stream",
        "get",
        "post",
        "post_confirmed",
        "use_api_key",
        "verify_app_cluster",
    ):
        assert callable(getattr(Client, name)), name
