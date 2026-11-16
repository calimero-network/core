import pytest

from check_workflow_hardening import violations

SHA = "a" * 40

WORKFLOW = """
{permissions}
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
    - uses: {uses}
"""


def check(
    tmp_path,
    permissions="permissions:\n  contents: read",
    uses=f"actions/checkout@{SHA}",
):
    workflow = tmp_path / "ci.yml"
    workflow.write_text(WORKFLOW.format(permissions=permissions, uses=uses))
    return violations(workflow)


def test_a_workflow_without_permissions_is_refused(tmp_path):
    assert check(tmp_path, permissions="")


@pytest.mark.parametrize(
    "uses", ["actions/checkout@v7", "dtolnay/rust-toolchain@stable", "a/b@" + SHA[:7]]
)
def test_a_mutable_ref_is_refused(tmp_path, uses):
    assert check(tmp_path, uses=uses)


@pytest.mark.parametrize("uses", [f"actions/checkout@{SHA} # v7", "./local-action"])
def test_a_pinned_or_local_action_passes(tmp_path, uses):
    assert check(tmp_path, uses=uses) == []


def check_job(tmp_path, top, job, name="ci.yml", job_name="build"):
    workflow = tmp_path / name
    workflow.write_text(
        f"{top}\njobs:\n  {job_name}:\n    runs-on: ubuntu-latest\n{job}"
        f"    steps:\n    - uses: actions/checkout@{SHA}\n"
    )
    return violations(workflow)


READ = "permissions:\n  contents: read"


@pytest.mark.parametrize(
    "top", ["permissions: write-all", "permissions:\n  contents: write"]
)
def test_a_write_scope_at_workflow_level_is_refused(tmp_path, top):
    assert check_job(tmp_path, top, "")


def test_write_all_in_a_job_is_refused_even_inside_an_environment(tmp_path):
    job = "    environment: pypi\n    permissions: write-all\n"
    assert check_job(
        tmp_path, READ, job, name="build-wheels.yml", job_name="publish-pypi"
    )


def test_a_write_scope_outside_the_allowlist_is_refused(tmp_path):
    assert check_job(tmp_path, READ, "    permissions:\n      contents: write\n")
    job = "    permissions:\n      contents: read\n      pages: write\n"
    assert check_job(
        tmp_path, READ, job, name="build-wheels.yml", job_name="publish-pypi"
    )


def test_an_allowed_write_scope_passes(tmp_path):
    job = "    permissions:\n      contents: read\n      id-token: write\n"
    assert (
        check_job(tmp_path, READ, job, name="build-wheels.yml", job_name="publish-pypi")
        == []
    )
