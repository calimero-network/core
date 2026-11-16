import pytest

from check_oidc_jobs import violations

PUBLISH = """
permissions:
  contents: read
jobs:
  publish:
    environment: {environment}
    permissions:
      id-token: write
    runs-on: ubuntu-latest
    steps: []
"""


@pytest.mark.parametrize(
    "environment",
    ["''", "${{ github.event.inputs.env }}", "{name: '${{ vars.ENV }}'}"],
)
def test_an_environment_that_may_be_empty_is_refused(tmp_path, environment):
    workflow = tmp_path / "publish.yml"
    workflow.write_text(PUBLISH.format(environment=environment))

    assert violations(workflow)


@pytest.mark.parametrize("environment", ["pypi", "{name: pypi}"])
def test_a_fixed_environment_passes(tmp_path, environment):
    workflow = tmp_path / "publish.yml"
    workflow.write_text(PUBLISH.format(environment=environment))

    assert violations(workflow) == []
