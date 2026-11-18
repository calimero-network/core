"""The generated client is Python source built from ABI strings, so it is only
safe to import if those strings cannot leave the token they were placed in."""

import copy
import importlib.util
import json
import sys
import types
import uuid
from pathlib import Path

import pytest

from calimero_abi_codegen.generator import ClientGenerator
from calimero_abi_codegen.parser import WASMABIParser

EXAMPLE_ABI = Path(__file__).parent.parent / "examples" / "schemas" / "example_abi.json"

BASE_ABI = {
    "schema_version": "wasm-abi/1",
    "types": {
        "Rec": {"kind": "record", "fields": [{"name": "x", "type": {"kind": "u32"}}]},
        "Var": {"kind": "variant", "variants": [{"name": "On"}]},
    },
    "methods": [
        {"name": "get", "params": [{"name": "key", "type": {"kind": "string"}}]}
    ],
    "events": [],
}


REFUSED = "cannot be used as a name in generated code"


def _payload(marker: Path) -> str:
    return f"__import__('pathlib').Path({str(marker)!r}).touch()"


def _import_generated(out: Path, abi: dict, class_name: str = "ABIClient"):
    """Generate a package from `abi` and import it with a stub `calimero`."""
    files = ClientGenerator(WASMABIParser().parse(abi)).generate_all(out, class_name)
    pkg_dir = out / "gen"
    pkg_dir.mkdir()
    for name, code in files.items():
        (pkg_dir / name).write_text(code)

    stub = types.ModuleType("calimero")
    stub.CloudClient = object
    stub.CloudError = type("CloudError", (Exception,), {})
    name = f"gen_{uuid.uuid4().hex}"
    spec = importlib.util.spec_from_file_location(
        name, pkg_dir / "__init__.py", submodule_search_locations=[str(pkg_dir)]
    )
    module = importlib.util.module_from_spec(spec)
    saved = sys.modules.get("calimero")
    sys.modules["calimero"] = stub
    sys.modules[name] = module
    try:
        # Leaf modules first, so each file runs even if a later one fails to parse.
        importlib.import_module(f"{name}.types")
        importlib.import_module(f"{name}.client")
        spec.loader.exec_module(module)
    finally:
        for key in [k for k in sys.modules if k == name or k.startswith(name + ".")]:
            del sys.modules[key]
        if saved is None:
            sys.modules.pop("calimero", None)
        else:
            sys.modules["calimero"] = saved
    return module


def _rename_type(abi: dict, old: str, new: str) -> None:
    abi["types"] = {(new if k == old else k): v for k, v in abi["types"].items()}


SITES = {
    "record name": lambda abi, n: _rename_type(abi, "Rec", n),
    "variant type name": lambda abi, n: _rename_type(abi, "Var", n),
    "field name": lambda abi, n: abi["types"]["Rec"]["fields"][0].update(name=n),
    "variant name": lambda abi, n: abi["types"]["Var"]["variants"][0].update(name=n),
    "method name": lambda abi, n: abi["methods"][0].update(name=n),
    "param name": lambda abi, n: abi["methods"][0]["params"][0].update(name=n),
}


@pytest.mark.parametrize("site", SITES)
@pytest.mark.parametrize(
    "kind", ["code", "keyword", "empty", "dunder", "underscore", "non-ascii"]
)
def test_unsafe_name_is_refused(tmp_path, site, kind):
    marker = tmp_path / "marker"
    name = {
        # Breaks out of a class header into module-level code.
        "code": f"A:\n    pass\n{_payload(marker)}\nclass B",
        "keyword": "class",
        "empty": "",
        "dunder": "__class__",
        "underscore": "_x",
        # NFKC-normalises to "fix" as an identifier but not inside a literal.
        "non-ascii": "\ufb01x",
    }[kind]
    abi = copy.deepcopy(BASE_ABI)
    SITES[site](abi, name)

    with pytest.raises(ValueError, match=REFUSED):
        _import_generated(tmp_path, abi)
    assert not marker.exists()


@pytest.mark.parametrize(
    "site, name",
    [
        ("record name", "CloudError"),
        ("variant type name", "Enum"),
        ("record name", "json"),
        ("method name", "client"),
        ("param name", "self"),
        ("param name", "json"),
        ("param name", "args"),
    ],
)
def test_name_the_generated_code_uses_is_refused(tmp_path, site, name):
    abi = copy.deepcopy(BASE_ABI)
    SITES[site](abi, name)

    with pytest.raises(ValueError, match=REFUSED):
        _import_generated(tmp_path, abi)


def test_duplicate_parameter_names_are_refused(tmp_path):
    abi = copy.deepcopy(BASE_ABI)
    abi["methods"][0]["params"].append({"name": "key", "type": {"kind": "u32"}})

    with pytest.raises(ValueError, match="duplicate parameter"):
        _import_generated(tmp_path, abi)


def test_record_without_fields_is_importable(tmp_path):
    abi = copy.deepcopy(BASE_ABI)
    abi["types"]["Empty"] = {"kind": "record", "fields": []}

    module = _import_generated(tmp_path, abi)

    assert module.Empty() is not None


def test_unsafe_class_name_is_refused(tmp_path):
    with pytest.raises(ValueError, match=REFUSED):
        _import_generated(tmp_path, copy.deepcopy(BASE_ABI), class_name="A(object)")


def test_schema_version_is_emitted_as_a_literal(tmp_path):
    marker = tmp_path / "marker"
    version = f'1"\n{_payload(marker)}\nx = "'
    abi = copy.deepcopy(BASE_ABI)
    abi["schema_version"] = version

    module = _import_generated(tmp_path, abi)

    assert not marker.exists()
    assert module.__version__ == version


def test_example_abi_generates_an_importable_client(tmp_path):
    module = _import_generated(tmp_path, json.loads(EXAMPLE_ABI.read_text()))

    assert hasattr(module, "ABIClient")
    assert module.Action.Ping.value == "Ping"
