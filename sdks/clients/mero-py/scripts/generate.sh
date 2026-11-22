#!/usr/bin/env bash
# Regenerates everything mero-py generates (sdk.json `generate`; run from
# sdks/clients/mero-py, or as `python3 scripts/sdks.py generate mero-py` from
# core's root). Reads core's document from this tree; fetches no document.
#
#   ../../../docs/public/openapi.json -> calimero_client_py/openapi/   (openapi-python-client)
#                                     -> calimero_client_py/facade.py  (scripts/generate_facade.py)
#                                     -> ../../../docs/src/content/docs/clients/mero-py/reference/methods.mdx
#   abi-codegen/examples/schemas/example_abi.json -> abi-codegen/examples/generated_client/
#
# The generators and Black run, at the versions CI checks with, in their own
# venv under target/ (ignored), so this needs only python3 and, the first
# time, PyPI for those tools.
set -euo pipefail
cd "$(dirname "$0")/.."

venv=target/generate-venv
tools=(openapi-python-client==0.26.2 black==26.10.1 ./abi-codegen)
stamp="$venv/.tools"
if [ ! -x "$venv/bin/python" ] || [ "$(cat "$stamp" 2>/dev/null)" != "${tools[*]}" ]; then
  rm -rf "$venv"
  python3 -m venv "$venv"
  "$venv/bin/python" -m pip install --quiet --disable-pip-version-check "${tools[@]}"
  echo "${tools[*]}" > "$stamp"
fi

"$venv/bin/python" scripts/generate_openapi_client.py

out=abi-codegen/examples/generated_client
rm -rf "$out"
"$venv/bin/calimero-abi-codegen" --input abi-codegen/examples/schemas/example_abi.json --output "$out" >/dev/null
"$venv/bin/black" --quiet --config pyproject.toml "$out"
echo "regenerated $out"
