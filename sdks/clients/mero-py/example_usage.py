#!/usr/bin/env python3
"""
Example usage of calimero-client-py against a Calimero 1.0 cluster.

1. Install the package: pip install calimero-client-py
   (or from a checkout: maturin develop --release)
2. Start a development cluster (see tests/test_cloud_v2.py for `merod run`)
   and build an app bundle, e.g. core's kv-store fixture.
3. Run: python example_usage.py http://127.0.0.1:2428 path/to/kv_store.wasm

Development clusters serve mock quotes, so this passes allow_mock=True; a
production node is pinned to a signed mero-tee release with tee_profiles=[...].
"""

import sys

from calimero_client_py import Client, CloudError, SoftPasskey


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    url, wasm_path = sys.argv[1], sys.argv[2]

    client = Client(url, allow_mock=True)
    passkey = SoftPasskey()
    try:
        client.sign_up(passkey, "example")
        print("signed in as", client.json.auth.me()["accountId"])

        with open(wasm_path, "rb") as f:
            bundle = client.json.packages.bundle_put(body=f.read())["hash"]
        app = client.json.apps.create(name="example-kv", bundle=bundle)["appId"]
        print("created app", app)

        r = client.json.apps.call(
            app=app, method="set", args={"key": "greeting", "value": "hello"}
        )
        print("committed at index", r["index"])
        print(
            "read back:",
            client.json.apps.view(app=app, method="get", args={"key": "greeting"})[
                "result"
            ],
        )

        for e in client.events(app, start=0, limit=10, timeout=2):
            print("event", e["id"], e["kind"], e["data"])
    except CloudError as e:
        message, status, code, _details = e.args
        print(f"failed: {status} {code}: {message}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
