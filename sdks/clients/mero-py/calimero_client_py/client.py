"""`Client` (also `CloudClient`): a client for one Calimero 1.0 cluster.

The routes are `calimero_client_py.openapi`, generated from core's OpenAPI
document; `Client.api()` gives the generated calls a client whose transport
seals every request to the node's attested key (`calimero_client_py.sealed`).
What is not a route is the native `SealedClient`'s: the attestation and the
sealed channel, the move between nodes and the refresh of a session, the event
streams, and the keys the client signs with.

The routes are the facade's methods (`client.orgs.invite_create(...)`, one
per operation, generated): 1.0.0rc4 removed the method per route earlier releases
had (CHANGELOG.md maps each to its facade method). What stays here is what is
not a route: the passkey ceremonies, the streams, the generic path calls and
the key helpers.
"""

import base64
import json
import os
from typing import Any, Callable, Dict, List, Optional, Tuple, Union

from . import _native, facade
from ._facade import Group
from ._native import CloudError
from .openapi import AuthenticatedClient
from .sealed import error_from, sealed_client

__all__ = ["Client", "CloudClient"]

_SealedClient = _native.SealedClient

# The fields of a session, as earlier releases answered it (core's `Session`).
_SESSION = ("accountId", "sessionToken", "expiresAt", "refreshToken")


def _confirm(passkey: Any) -> Any:
    """The facade's `confirm=` for a method's `passkey`: none given is none
    used (not the client's own, which these methods never used)."""
    return False if passkey is None else passkey


class _GroupAttribute:
    """`client.<group>`: the facade's operations of one tag on this client."""

    def __init__(self, name: str):
        self.name = name
        self.__doc__ = facade.GROUPS[name].__doc__

    def __get__(self, client: Any, owner: Any = None) -> Any:
        if client is None:
            return facade.GROUPS[self.name]
        return facade.GROUPS[self.name](client)


class _JsonGroups:
    """`client.json`: each facade group, answering JSON."""

    def __init__(self, client: Any):
        self._client = client

    def __getattr__(self, name: str) -> Any:
        group = facade.GROUPS.get(name)
        if group is None:
            raise AttributeError(name)
        return group(self._client, as_json=True)


def _answer(response: Any) -> Any:
    """A generated call's answer as JSON, or the error it stands for."""
    status = int(response.status_code)
    if status >= 400:
        raise error_from(status, response.content)
    if not response.content:
        return None
    try:
        return json.loads(response.content)
    except ValueError:
        return None


def _field(v: Any, name: str) -> str:
    if isinstance(v, dict) and isinstance(v.get(name), str):
        return v[name]
    raise ValueError(f"the node's answer has no {name}")


def _str(v: Any, name: str) -> str:
    """`v[name]` as text, `""` when absent (as core's client reads ids)."""
    found = v.get(name) if isinstance(v, dict) else None
    return found if isinstance(found, str) else ""


def _session(v: Any) -> Dict[str, Any]:
    if not isinstance(v, dict) or any(k not in v for k in _SESSION):
        raise CloudError("the node's answer is not a session", 0, "TRANSPORT", "null")
    return {k: v[k] for k in _SESSION}


def _path(p: Any) -> Optional[str]:
    """`ca_bundle` as the native client takes it: a path as text, or None."""
    return None if p is None else os.fspath(p)


def _json_arg(v: Any) -> Any:
    """An argument the client sends as JSON: `None` is `{}`, as it always was."""
    return {} if v is None else v


class Client:
    """A client for one node of a Calimero 1.0 cluster (and, given `nodes`,
    the others it may move to).

    Connect to `base_url`. The node is trusted only once its attestation
    verifies; `allow_mock` accepts development (mock) quotes, `app_hash`
    (hex) pins the bundle the quote must bind.

    `nodes` are other nodes of the same cluster, in the order to move to
    them when a node answers `421 WRONG_NODE` (its release may not serve
    the account); each is attested under the same policy before anything
    is sent to it. Without them that answer is raised as `WrongNode`.
    A node that cannot be reached, or that keeps answering `503` to a read
    or a call, is left for the next too (each node once per request).
    A `401` to a request made with a session is answered by one refresh
    (any node takes it) and the request sent again; `LoginRequired` is
    raised only when that does not help.

    A real TDX quote is accepted only from an image the policy names:
    `tee_profiles` admits nodes whose signed mero-tee release lists their
    measurements under one of those profiles (`min_release` is a floor).
    Without it, only mock quotes (`allow_mock`) can be accepted. A
    release's debug profiles (whose memory the host can read) are refused
    unless `allow_debug_profiles`.

    `ca_bundle`: the path of a PEM file whose certificates are trusted as
    TLS roots on top of the built-in public ones (a private or staging CA,
    such as Let's Encrypt's staging roots on a non-production cluster),
    for every node, leader and directory the client reaches. None (the
    default) trusts the public roots only. A file that cannot be read
    raises the `OSError` it stands for (`FileNotFoundError`), one with no
    usable certificate `ValueError`, here rather than at the first request.
    The event streams, `attestation()` and `verify_app_cluster()` go
    through core's own client and trust the public roots only.

    The routes: `client.api()` is a client for the generated calls in
    `calimero_client_py.openapi.api`, sealed to the node:

        from calimero_client_py.openapi.api.organizations import org_create
        from calimero_client_py.openapi.models import OrgCreateBody
        org = org_create.sync(client=client.api(), body=OrgCreateBody(name="Acme"))
    """

    def __init__(
        self,
        base_url: str,
        allow_mock: bool = False,
        app_hash: Optional[str] = None,
        tee_profiles: Optional[List[str]] = None,
        min_release: Optional[str] = None,
        allow_debug_profiles: bool = False,
        nodes: Optional[List[str]] = None,
        ca_bundle: Optional[Union[str, "os.PathLike[str]"]] = None,
    ):
        self._init(
            _SealedClient(
                base_url,
                allow_mock,
                app_hash,
                tee_profiles,
                min_release,
                allow_debug_profiles,
                nodes,
                _path(ca_bundle),
            )
        )

    def _init(self, native: Any) -> None:
        self._native = native
        self._passkey: Any = None
        self._operator_signer: Any = None
        self._clients: Dict[Tuple[Any, ...], AuthenticatedClient] = {}

    @classmethod
    def connect(
        cls,
        cluster: str,
        *,
        tee_profiles: Optional[List[str]] = None,
        min_release: Optional[str] = None,
        allow_mock: bool = False,
        app_hash: Optional[str] = None,
        allow_debug_profiles: bool = False,
        directory: Optional[str] = None,
        zone: Optional[str] = None,
        ca_bundle: Optional[Union[str, "os.PathLike[str]"]] = None,
    ) -> "Client":
        """Connect to the cluster named `cluster`, by the nodes the Calimero
        Cloud node directory lists for it (`GET {directory}/{cluster}`).

        The nodes in `zone` come first, then the rest, each group in a
        weighted random order; the first is the node in use and the others
        are `nodes`. The directory is not trusted: every node is attested
        under this policy (the arguments mean what they do for the
        constructor) before anything is sent to it. The listing is fetched
        again once it is due and after the client moved off an unavailable
        node. Raises `CloudError` (code `DIRECTORY`; the status is the
        directory's, `404` for an unknown cluster, 0 when it could not be
        reached) when the directory lists no usable node.
        `ca_bundle` is the constructor's, and covers the directory too.
        """
        client = cls.__new__(cls)
        client._init(
            _SealedClient.connect(
                cluster,
                tee_profiles=tee_profiles,
                min_release=min_release,
                allow_mock=allow_mock,
                app_hash=app_hash,
                allow_debug_profiles=allow_debug_profiles,
                directory=directory,
                zone=zone,
                ca_bundle=_path(ca_bundle),
            )
        )
        return client

    # ------------------------------------------------------------ the generated client

    def api(
        self,
        passkey: Any = None,
        *,
        operator_signed: bool = False,
        operator_key: Optional[str] = None,
        operator_signer: Optional[Callable[[Dict[str, Any]], Any]] = None,
        follow_leader: bool = False,
        repeatable: bool = False,
        retry_unavailable: bool = False,
        once: bool = False,
    ) -> AuthenticatedClient:
        """A client for the generated calls (`calimero_client_py.openapi.api`),
        whose requests go through the sealed channel with this client's
        credential (`use_api_key`, a session).

        `passkey` answers a `412 CONFIRM_REQUIRED` (what an owner approves):
        the request is sent again with its assertion over the node's
        challenge. `operator_signed` answers a `428
        OPERATOR_SIGNATURE_REQUIRED` with the signer `use_operator_signer` set,
        else the key `use_operator_key` set (or `operator_signer`, or
        `operator_key`, a seed in hex). `follow_leader` sends a request a node
        answers `421 NOT_LEADER` to the leader it names (the Ops API's replica
        moves and node removal). `repeatable` marks requests safe to send
        twice (a GET always is), `retry_unavailable` sends them again to the
        same node while it answers `503` (a call with an idempotency key),
        `once` sends them once (a sign-in or sign-up).
        """
        if operator_signed and operator_key is None and operator_signer is None:
            operator_signer = self._operator_signer
        options = dict(
            operator_signed=operator_signed,
            operator_key=operator_key,
            operator_signer=operator_signer,
            follow_leader=follow_leader,
            repeatable=repeatable,
            retry_unavailable=retry_unavailable,
            once=once,
        )
        if passkey is not None:
            return sealed_client(self._native, passkey, **options)
        key = tuple(sorted(options.items(), key=lambda kv: kv[0]))
        client = self._clients.get(key)
        if client is None:
            client = self._clients[key] = sealed_client(self._native, **options)
        return client

    def _op(
        self,
        operation_id: str,
        *path: Any,
        query: Optional[Dict[str, Any]] = None,
        body: Any = None,
        confirm: Any = False,
        operator_proof: Any = None,
    ) -> Any:
        """Run the facade's operation `operation_id` with a JSON body as given,
        answering the node's JSON as it came (what the ceremonies read). A
        passkey answers a step-up only when given (`confirm`), never the
        client's own (`use_passkey`)."""
        op = getattr(facade, "_" + operation_id.upper())
        return Group(self, as_json=True)._run(
            op, path, query or {}, body, confirm, operator_proof
        )

    def use_operator_signer(
        self, signer: Optional[Callable[[Dict[str, Any]], Any]]
    ) -> None:
        """Answer the node's `428 OPERATOR_SIGNATURE_REQUIRED` with
        `signer(details)`, the `operatorProof` (`{nonce, signature}`) for the
        statement `details` names (`{subject, method, args, nonce, cluster,
        message}`): for a key the client never holds, or a signer that checks
        the statement before it signs. It takes precedence over
        `use_operator_key`. `None` forgets it."""
        self._operator_signer = signer

    @property
    def json(self) -> Any:
        """The facade's groups answering the node's JSON as it came (dicts and
        lists) rather than the generated models: `client.json.ops.status()`.
        For a node older than the document, whose answers may lack a field the
        document requires."""
        return _JsonGroups(self)

    def use_passkey(self, passkey: Any) -> None:
        """Answer the node's `412 CONFIRM_REQUIRED` in the facade's methods
        (`client.orgs.invite_create(...)`) with `passkey` (anything with
        `create(options)` and `get(challenge)`, as `SoftPasskey`), unless a
        call passes its own `confirm=`. `None` forgets it."""
        self._passkey = passkey

    def _path(
        self,
        method: str,
        path: str,
        body: Any = None,
        *,
        json_body: bool = True,
        **options: Any,
    ) -> Any:
        """`method` on any API `path` through the sealed transport."""
        client = self.api(**options).get_httpx_client()
        kwargs: Dict[str, Any] = {}
        if json_body:
            kwargs["json"] = body
        response = client.request(method, "/" + path.lstrip("/"), **kwargs)
        return _answer(response)

    def _text(self, response: Any) -> str:
        """A text answer (`ops/metrics`); an error's text is its message."""
        status = int(response.status_code)
        text = response.content.decode("utf-8", errors="replace")
        if status >= 400:
            raise CloudError(f"{status} ERROR: {text}", status, "ERROR", "null")
        return text

    # ------------------------------------------------------------ the connection

    @property
    def node_url(self) -> str:
        """The URL of the node in use: `base_url`, until a `421 WRONG_NODE` or
        an unavailable node moved the client to the next of `nodes`."""
        return self._native.node_url

    @property
    def node_urls(self) -> List[str]:
        """Every node this client may use, in the order it moves through them
        (the node in use among them)."""
        return self._native.node_urls

    def attestation(self) -> Any:
        """The node's attestation document (unverified, for display)."""
        return self._native.attestation()

    def verify_app_cluster(self, app: str) -> Any:
        """Verify every replica of `app`: each one's quote over a fresh nonce,
        held to this client's policy, binding its receipt key and the
        cluster's governance. Returns `{"keys": {node_id: receipt_key_hex},
        "governance": {"releases", "appUpgrades", "outbound"}}`; a call's or
        view's `receipt` is to be trusted only from one of those keys."""
        return self._native.verify_app_cluster(app)

    # ------------------------------------------------------------ credentials

    def use_api_key(self, key: str) -> None:
        """Authenticate every request with an API key (`cal_...`)."""
        self._native.use_api_key(key)

    def use_session(self, session: Any) -> None:
        """Authenticate with a session returned by `sign_up`/`sign_in`."""
        self._native.use_session(session)

    def session(self) -> Any:
        """The session in use (`sign_up`, `sign_in`, `refresh`, `use_session`),
        or `None` (none yet, or an API key instead)."""
        return self._native.session()

    def refresh(self) -> Any:
        """Exchange the refresh token for a new session, on the node in use (any
        node of the cluster takes it). One at a time with the refreshes the
        client makes on its own after a `401`: a refresh token is used once."""
        return self._native.refresh()

    def use_operator_key(self, seed: Optional[str]) -> None:
        """Sign the Ops API's decisions with the operator's Ed25519 key (its
        32-byte seed, hex; the nodes were started with `--operator-key` set to
        its verify key): a `428 OPERATOR_SIGNATURE_REQUIRED` from `ops_*` is
        then answered with the signature. `None` forgets it."""
        self._native.use_operator_key(seed)

    @staticmethod
    def operator_public_key(seed: str) -> str:
        """The operator key's verify half (hex), what the nodes take as
        `--operator-key`, from its 32-byte seed (hex; `secrets.token_hex(32)`
        makes one)."""
        return _SealedClient.operator_public_key(seed)

    @staticmethod
    def api_key_public(key: str) -> str:
        """The Ed25519 public key (hex) an API key stands for, byte-for-byte
        core's `proof::api_key_public`: the seed is `SHA-256("calimero.api-key.v1\\0"
        || key)`. An upgrade key's record (`list_api_keys`) keeps it as
        `upgrades.proofKey`, and the node signs each upgrade the key makes
        with its private half, so the directory's log proves the key holder
        asked for it. Compare the two to check which key a record is."""
        return _SealedClient.api_key_public(key)

    @staticmethod
    def normalize_recovery_code(code: str) -> str:
        """The canonical form of a recovery code as written down: dashes and
        spaces dropped, upper case, `O` read as `0` and `I`/`L` as `1`.
        `ValueError` for anything that cannot be a code."""
        return _SealedClient.normalize_recovery_code(code)

    @staticmethod
    def recovery_code_key(code: str) -> str:
        """The Ed25519 public key (hex) a recovery code stands for: the seed is
        `SHA-256("calimero.account-recovery-code.v1\\0" || canonical code)`,
        core's `proof::recovery_key`. The node records only this; it signs a
        recovery's start with the private half. `ValueError` for anything that
        cannot be a code."""
        return _SealedClient.recovery_code_key(code)

    @staticmethod
    def new_recovery_seed() -> str:
        """A new recovery seed, as the owner writes it down (64 hex digits in
        groups of eight). Keep it offline; give the organization only
        `recovery_public_key(seed)`."""
        return _SealedClient.new_recovery_seed()

    @staticmethod
    def recovery_public_key(seed: str) -> str:
        """The public key (base64url) of a recovery seed: what the organization
        stores (`create_org(.., recovery_key=)`, `set_recovery_key`)."""
        return _SealedClient.recovery_public_key(seed)

    @staticmethod
    def sign_recovery(seed: str, org: str, account: str, nonce: str) -> str:
        """The seed's signature (base64url) handing `org` to `account` under
        `nonce`, as `start_recovery` sends it."""
        return _SealedClient.sign_recovery(seed, org, account, nonce)

    # ------------------------------------------------------------ passkey ceremonies

    def _signed_in(self, answer: Any) -> Dict[str, Any]:
        s = _session(answer)
        self._native.use_session(s)
        return s

    def sign_up(self, passkey: Any, name: str, origin: Optional[str] = None) -> Any:
        """Create an account with a new passkey and sign in. Returns the session.
        The node then asks the new passkey to sign the account it creates
        (`412 CONFIRM_REQUIRED`), which `passkey` answers; a node from before
        that answers the registration at once. `origin`: the web origin the
        passkey is made for (one a package registered, `add_package_origin`);
        without it, the node's own."""
        begin_body: Dict[str, Any] = {"name": name}
        if origin is not None:
            begin_body["origin"] = origin
        begin = self._op("register_begin", body=begin_body)
        registration = passkey.create(begin["options"])
        done = self._op(
            "register_finish",
            body={"challengeId": begin["challengeId"], "registration": registration},
            confirm=_confirm(passkey),
        )
        return self._signed_in(done)

    def sign_in(self, passkey: Any, origin: Optional[str] = None) -> Any:
        """Sign in with an existing passkey. Returns the session. `origin` as
        `sign_up`: the session is then the app's that registered it."""
        begin_body = {} if origin is None else {"origin": origin}
        begin = self._op("login_begin", body=begin_body)
        challenge = begin["options"]["challenge"]
        assertion = passkey.get(
            base64.urlsafe_b64decode(challenge + "=" * (-len(challenge) % 4))
        )
        done = self._op(
            "login_finish",
            body={"challengeId": begin["challengeId"], "assertion": assertion},
        )
        return self._signed_in(done)

    def add_passkey(self, passkey: Any, approver: Any) -> str:
        """Add `passkey` (another device) to the signed-in account, approved
        with `approver`, a passkey the account already has (the node's `412
        CONFIRM_REQUIRED`). Returns the new credential's id."""
        if passkey is approver:
            raise ValueError(
                "approve a new passkey with one the account already has, not itself"
            )
        begin = self._op("passkey_add_begin")
        registration = passkey.create(begin["options"])
        done = self._op(
            "passkey_add_finish",
            body={"challengeId": begin["challengeId"], "registration": registration},
            confirm=_confirm(approver),
        )
        return _str(done, "credentialId")

    def start_account_recovery(self, code: str, passkey: Any) -> Any:
        """Start recovering the account `code` names, signed out, with `passkey`,
        a new one made here for the node's options (`/auth/recovery/begin`,
        then `/start`). Nothing is added and no session opens yet: answers the
        pending recovery, `{accountId, credentialId, startedAt, readyAt}`;
        `finish_account_recovery` with the same passkey once `readyAt` has
        passed (72 hours). The code is spent. A wrong or spent code raises
        `LoginRequired` (`401`), an account with a recovery under way
        `CloudError` `409 RECOVERY_PENDING`; a code that cannot be one,
        `ValueError` before anything is sent."""
        code = _SealedClient.normalize_recovery_code(code)
        begin = self._op(
            "account_recovery_begin",
            body={"code": code},
        )
        registration = passkey.create(begin["options"])
        return self._op(
            "account_recovery_start",
            body={
                "challengeId": begin["challengeId"],
                "code": code,
                "registration": registration,
            },
        )

    def finish_account_recovery(
        self, passkey: Any, account_id: Optional[str] = None
    ) -> Any:
        """Finish a recovery once its delay passed, signed out, with `passkey`,
        the one `start_account_recovery` registered: the node adds it, ends
        every session the account had, and signs in. Returns the session and
        uses it. `account_id` (the start's `accountId`) lets the node refuse
        early; without it the passkey names the recovery. Before `readyAt`:
        `CloudError` `409 RECOVERY_NOT_READY` (`details.readyAt`)."""
        body = {} if account_id is None else {"accountId": account_id}
        done = self._op(
            "account_recovery_finish",
            body=body,
            confirm=_confirm(passkey),
        )
        return self._signed_in(done)

    def start_recovery(self, org: str, seed: str) -> Any:
        """Start handing `org` to the signed-in account with the recovery
        `seed`, which signs here and is never sent. Answers `{ startedAt,
        readyAt }`; `finish_recovery` once `readyAt` has passed. A wrong seed
        raises `CloudError` (403 `WRONG_RECOVERY_KEY`)."""
        _SealedClient.recovery_public_key(seed)  # a seed, before anything is sent
        account = _field(self._op("me"), "accountId")
        nonce = base64.urlsafe_b64encode(os.urandom(16)).rstrip(b"=").decode()
        signature = _SealedClient.sign_recovery(seed, org, account, nonce)
        return self._op(
            "recovery_start",
            org,
            body={"nonce": nonce, "signature": signature},
        )

    # ------------------------------------------------------------ streams

    def events(
        self, app: str, start: int = 0, limit: int = 100, timeout: float = 5.0
    ) -> Any:
        """Committed events from index `start`: up to `limit` of them, or as many
        as arrive within `timeout` seconds. Each is `{"id", "kind", "data"}`."""
        return self._native.events(app, start, limit, timeout)

    def presence_stream(self, app: str, limit: int = 100, timeout: float = 5.0) -> Any:
        """Presence as a stream: the snapshot first (`{"kind": "snapshot",
        "data": [...]}`), then one `{"kind": "presence", "data": {...}}` per
        change; up to `limit` of them, or as many as arrive within `timeout`
        seconds."""
        return self._native.presence_stream(app, limit, timeout)

    # ------------------------------------------------------------ any path

    def get(self, path: str) -> Any:
        """GET any API path."""
        return self._path("GET", path, json_body=False)

    def get_text(self, path: str) -> str:
        """GET any API path whose answer is text (`ops/metrics`)."""
        client = self.api().get_httpx_client()
        return self._text(client.request("GET", "/" + path.lstrip("/")))

    def post(self, path: str, body: Any = None) -> Any:
        """POST any API path (organizations, teams, invites, ...)."""
        return self._path("POST", path, _json_arg(body))

    def put(self, path: str, body: Any = None) -> Any:
        """PUT any API path (an alias: `auth/aliases/{alias}` with `{"app": ...}`)."""
        return self._path("PUT", path, _json_arg(body))

    def delete(self, path: str, body: Any = None) -> Any:
        """DELETE any API path; `body`, when given, goes as JSON (a proof the
        caller made itself: `{"confirm": ...}` for a passkey, a member or a
        team row, `{"operatorProof": ...}` for `ops/nodes/{id}`). Without one
        the DELETE carries no body, as before."""
        if body is None:
            return self._path("DELETE", path, json_body=False)
        return self._path("DELETE", path, body)

    def post_confirmed(self, path: str, body: Any, passkey: Any) -> Any:
        """POST `body` to `path`, answering a `412 CONFIRM_REQUIRED` with
        `passkey`'s assertion over the node's challenge: what an owner
        approves (upgrades, organization upgrades, release and outbound
        approvals, deleting an organization's...)."""
        return self._path_confirmed("POST", path, _json_arg(body), passkey)

    def delete_confirmed(self, path: str, passkey: Any, body: Any = None) -> Any:
        """DELETE `path`, answering `412 CONFIRM_REQUIRED` (deleting an
        organization) with `passkey`, as `post_confirmed` does. `body`, when
        given, goes as JSON, and the approval is added to it (`{"origin": ...}`
        on `package-origins/{package}`)."""
        return self._path_confirmed("DELETE", path, _json_arg(body), passkey)

    def _path_confirmed(self, method: str, path: str, body: Any, passkey: Any) -> Any:
        client = self.api(passkey).get_httpx_client()
        return _answer(client.request(method, "/" + path.lstrip("/"), json=body))

    def post_operator_signed(
        self, path: str, body: Any = None, operator_key: Optional[str] = None
    ) -> Any:
        """POST an operator decision to `path`, answering `428
        OPERATOR_SIGNATURE_REQUIRED` with `operator_key` (a seed, hex), else
        the key `use_operator_key` set."""
        return self._path(
            "POST",
            path,
            _json_arg(body),
            operator_signed=True,
            operator_key=operator_key,
        )

    def delete_operator_signed(
        self, path: str, operator_key: Optional[str] = None
    ) -> Any:
        """DELETE an operator decision (`ops/nodes/{id}`), signed as
        `post_operator_signed`."""
        return self._path(
            "DELETE", path, {}, operator_signed=True, operator_key=operator_key
        )


# The facade's groups: `client.orgs.get(org=...)`.
for _name in facade.GROUPS:
    setattr(Client, _name, _GroupAttribute(_name))
del _name

# The name earlier releases used; one class, two names.
CloudClient = Client
