"""The generated Python clients (`tests/golden/*.py`) against the published calimero-client-py.

Each module is imported with the real `calimero` package, then every method of its client
class runs against a stand-in whose `json.apps.call` / `json.apps.view` / `events` take
exactly the arguments the real `Client`'s do (`inspect.Signature.bind` against the facade),
so a generated call that names a method or keyword the client does not have fails here.

    pip install -r requirements.txt
    python smoke.py [golden dir] && mypy --strict --follow-untyped-imports ../golden/*.py
"""

import importlib.util
import inspect
import sys
from pathlib import Path
from typing import Any, Dict, List

import calimero
from calimero_client_py import facade

GOLDEN = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parent.parent / "golden"


class Apps:
    """`client.json.apps`, recording what is sent; binds each call to the real signature."""

    def __init__(self, sent: List[Dict[str, Any]]):
        self._sent = sent

    def call(self, **kwargs: Any) -> Dict[str, Any]:
        inspect.signature(facade.Apps.call).bind(None, **kwargs)
        self._sent.append({"kind": "call", **kwargs})
        return {"index": 1, "result": None, "events": [], "receipt": {}}

    def view(self, **kwargs: Any) -> Dict[str, Any]:
        inspect.signature(facade.Apps.view).bind(None, **kwargs)
        self._sent.append({"kind": "view", **kwargs})
        return {"index": 1, "result": None, "receipt": {}}


class Json:
    def __init__(self, sent: List[Dict[str, Any]]):
        self.apps = Apps(sent)


class Client:
    """What the generated class uses of `calimero.Client`."""

    def __init__(self) -> None:
        self.sent: List[Dict[str, Any]] = []
        self.json = Json(self.sent)

    def events(self, *args: Any) -> List[Dict[str, Any]]:
        inspect.signature(calimero.Client.events).bind(None, *args)
        self.sent.append({"kind": "events"})
        return []


def load(path: Path) -> Any:
    spec = importlib.util.spec_from_file_location(path.stem.replace("-", "_"), path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def main() -> None:
    # The JSON groups and the methods the generated classes send through exist.
    assert isinstance(inspect.getattr_static(calimero.Client, "json"), property)
    assert callable(facade.GROUPS["apps"].call) and callable(facade.GROUPS["apps"].view)
    checked = 0
    for path in sorted(GOLDEN.glob("*.py")):
        module = load(path)
        (cls,) = [
            c
            for name, c in vars(module).items()
            if name.endswith("Client") and inspect.isclass(c) and c.__module__ == module.__name__
        ]
        for name, method in inspect.getmembers(cls, inspect.isfunction):
            if name.startswith("_"):
                continue
            client = Client()
            sig = inspect.signature(method)
            args = [
                "x"
                for p in list(sig.parameters.values())[1:]
                if p.kind == p.POSITIONAL_OR_KEYWORD and p.default is p.empty
            ]
            extra: Dict[str, Any] = {}
            if "passkey" in sig.parameters:
                extra["passkey"] = object()
            if "idempotency_key" in sig.parameters:
                extra["idempotency_key"] = "k-1"
            if "consistency" in sig.parameters:
                extra.update(consistency="stale", min_index=3)
            getattr(cls(client, "app-1"), name)(*args, **extra)
            (sent,) = client.sent
            if sent["kind"] != "events":
                assert sent["app"] == "app-1", sent
                assert isinstance(sent["method"], str) and isinstance(sent["args"], dict), sent
            if "passkey" in extra:
                assert sent["confirm"] is extra["passkey"], sent
            if "idempotency_key" in extra:
                assert sent["idempotency_key"] == "k-1", sent
            if "consistency" in extra:
                assert sent["consistency"] == "stale" and sent["min_index"] == 3, sent
            checked += 1
    print(f"generated Python clients: {checked} methods ok")


if __name__ == "__main__":
    main()
