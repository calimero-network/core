"""scripts/facade_changes.py: what openapi-sync.yml reports in its pull request."""

import importlib.util
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "facade_changes.py"
spec = importlib.util.spec_from_file_location("facade_changes", SCRIPT)
facade_changes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(facade_changes)

OLD = """
class Orgs(Group):
    def get(self, *, org):
        pass

    def remove(self, *, org):
        pass

    def list(self):
        pass


GROUPS = {"orgs": Orgs}
"""

NEW = """
class Orgs(Group):
    def get(self, *, org, detail=UNSET):
        pass

    def list(self):
        pass

    def rename(self, *, org, name):
        pass


GROUPS = {"orgs": Orgs}
"""


def test_it_reports_added_removed_and_changed_methods():
    old = facade_changes.methods(OLD)
    new = facade_changes.methods(NEW)
    assert new["client.orgs.get"] == "*, org, detail=UNSET"
    out = facade_changes.report(old, new)
    assert "**Added**\n\n- `client.orgs.rename(*, org, name)`" in out
    assert "**Removed**\n\n- `client.orgs.remove(*, org)`" in out
    assert (
        "- `client.orgs.get`\n  - was `(*, org)`\n  - now `(*, org, detail=UNSET)`"
        in out
    )
    assert "client.orgs.list" not in out


def test_no_change_says_so():
    same = facade_changes.methods(OLD)
    assert facade_changes.report(same, same) == "No facade method changed.\n"


def test_it_reads_the_checked_in_facade():
    root = Path(__file__).resolve().parents[1]
    found = facade_changes.methods((root / "calimero_client_py/facade.py").read_text())
    assert "client.orgs.get" in found and "client.apps.call" in found
