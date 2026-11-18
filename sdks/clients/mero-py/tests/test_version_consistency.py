"""The version is declared once, in `Cargo.toml`, and nowhere else.

It used to be stated in three files kept in step by a test. That test caught
drift only after it happened, and twice it did not: 0.6.20 bumped `Cargo.toml`
and `calimero/__init__.py` but not `pyproject.toml`, so the publish gate saw no
change and shipped nothing while every check stayed green; 0.6.19 shipped a
`__version__` reading 0.3.0. Both were possible only because the number was
written down more than once.

`pyproject.toml` now declares `dynamic = ["version"]`, so maturin takes it from
`Cargo.toml`, and `calimero/__init__.py` reads it back from the installed
distribution. This guards the shape rather than the values: there is nothing
left to disagree.
"""

import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent.parent


def test_cargo_declares_the_version():
    text = (ROOT / "Cargo.toml").read_text()
    assert re.search(
        r'^version = "[^"]+"', text, re.M
    ), "no top-level version in Cargo.toml"


def test_pyproject_takes_the_version_from_cargo():
    text = (ROOT / "pyproject.toml").read_text()
    assert re.search(r'^dynamic = \[.*"version".*\]', text, re.M), (
        "pyproject.toml must declare version as dynamic so maturin reads it from "
        "Cargo.toml; a literal here is a second copy that can drift"
    )
    assert not re.search(r'^version = "', text, re.M), (
        "pyproject.toml states a version of its own - the publish gate and the "
        "wheel would then disagree with Cargo.toml"
    )


def test_the_package_reads_its_version_back():
    text = (ROOT / "calimero" / "__init__.py").read_text()
    assert not re.search(r'^__version__ = "', text, re.M), (
        "calimero/__init__.py hardcodes a version - it must read the installed "
        "distribution's, or it can report one the wheel does not carry"
    )
