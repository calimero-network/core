"""scripts/set_version.py: the release's version, written for the build."""

import importlib.util
import os
import subprocess
import shutil
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location(
    "set_version", ROOT / "scripts" / "set_version.py"
)
set_version = importlib.util.module_from_spec(spec)
spec.loader.exec_module(set_version)


@pytest.mark.parametrize(
    "semver, pep440",
    [
        ("1.0.0", "1.0.0"),
        ("1.0.0-rc.5", "1.0.0rc5"),
        ("2.1.3-beta.2", "2.1.3b2"),
        ("1.2.0-alpha.1", "1.2.0a1"),
    ],
)
def test_pep440(semver, pep440):
    assert set_version.pep440(semver) == pep440


@pytest.mark.parametrize("bad", ["v1.0.0", "1.0", "1.0.0-rc5", "1.0.0-next.1", ""])
def test_refuses_what_is_not_a_release(bad):
    with pytest.raises(ValueError):
        set_version.pep440(bad)


def test_writes_the_crate_version_only(tmp_path):
    shutil.copy(ROOT / "Cargo.toml", tmp_path / "Cargo.toml")
    shutil.copy(ROOT / "Cargo.lock", tmp_path / "Cargo.lock")
    before_toml = (tmp_path / "Cargo.toml").read_text()
    before_lock = (tmp_path / "Cargo.lock").read_text()
    assert set_version.set_version(tmp_path, "9.8.7-rc.6") == "9.8.7rc6"
    toml = (tmp_path / "Cargo.toml").read_text()
    lock = (tmp_path / "Cargo.lock").read_text()
    assert '\nversion = "9.8.7-rc.6"\n' in toml
    # One line changed in each file: the crate's own version.
    changed = [
        (a, b) for a, b in zip(before_toml.splitlines(), toml.splitlines()) if a != b
    ]
    assert len(changed) == 1 and changed[0][1] == 'version = "9.8.7-rc.6"'
    changed = [
        (a, b) for a, b in zip(before_lock.splitlines(), lock.splitlines()) if a != b
    ]
    assert len(changed) == 1 and changed[0][1] == 'version = "9.8.7-rc.6"'
    assert lock.count('name = "calimero-client-py"\nversion = "9.8.7-rc.6"') == 1


def _read_the_tag_script() -> str:
    """build-wheels.yml's `Read the tag` step, as the shell runs it."""
    lines = (ROOT / ".github" / "workflows" / "build-wheels.yml").read_text()
    lines = lines.splitlines()
    start = lines.index("    - name: Read the tag")
    run = next(i for i in range(start, len(lines)) if lines[i] == "      run: |")
    body = []
    for line in lines[run + 1 :]:
        if line and not line.startswith("        "):
            break
        body.append(line[8:])
    return "\n".join(body)


def _release_version(tag: str, tmp_path) -> subprocess.CompletedProcess:
    out = tmp_path / "github_output"
    out.write_text("")
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "REF": f"refs/tags/{tag}",
        "TAG": tag,
        "GITHUB_OUTPUT": str(out),
    }
    result = subprocess.run(
        ["bash", "-c", _read_the_tag_script()],
        env=env,
        capture_output=True,
        text=True,
    )
    result.version = out.read_text()
    return result


@pytest.mark.skipif(shutil.which("bash") is None, reason="needs bash")
@pytest.mark.parametrize(
    "tag, version, pep440",
    [
        # What semantic-release tags from master: a plain MAJOR.MINOR.PATCH.
        ("v1.0.0", "1.0.0", "1.0.0"),
        ("v2.3.4", "2.3.4", "2.3.4"),
        # A pre-release tag pushed by hand still builds.
        ("v1.0.0-rc.5", "1.0.0-rc.5", "1.0.0rc5"),
    ],
)
def test_a_release_tag_builds_its_version(tag, version, pep440, tmp_path):
    result = _release_version(tag, tmp_path)
    assert result.returncode == 0, result.stdout + result.stderr
    assert result.version == f"version={version}\n"
    shutil.copy(ROOT / "Cargo.toml", tmp_path / "Cargo.toml")
    shutil.copy(ROOT / "Cargo.lock", tmp_path / "Cargo.lock")
    assert set_version.set_version(tmp_path, version) == pep440
    assert f'\nversion = "{version}"\n' in (tmp_path / "Cargo.toml").read_text()


@pytest.mark.skipif(shutil.which("bash") is None, reason="needs bash")
@pytest.mark.parametrize("tag", ["v1.0", "v1.0.0-rc5", "v1.0.0-next.1"])
def test_a_tag_that_is_not_a_release_does_not_build(tag, tmp_path):
    result = _release_version(tag, tmp_path)
    assert result.returncode != 0
    assert result.version == ""
