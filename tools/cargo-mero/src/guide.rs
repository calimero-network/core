//! The workflow guide printed by `cargo mero` (bare) and `cargo mero guide`.

/// Long `--help` description for the `mero` subcommand.
pub const ABOUT_LONG: &str = "Calimero application toolchain: scaffold, build, test, and bundle \
WASM apps for Calimero nodes, from `cargo mero new` through a signed .mpk ready for `meroctl app \
install`.";

const GUIDE: &str = "\
The Calimero app workflow

  1. cargo mero new my-app        scaffold an app (state, events, logic, tests)
  2. cargo mero build             compile -> wasm-opt -> embed ABI (res/<name>.wasm)
  3. cargo mero test              run TestHost unit tests + convergence tests (no node needed)
  4. cargo mero bundle --dev      build all services, write manifest.json, sign, tar dist/<package>.mpk
  5. meroctl app install --path dist/<package>.mpk ...   install on a node (merod)

Signing:
  cargo mero key generate -o key.json      create a production Ed25519 key
  cargo mero bundle --key key.json         sign with it (CI: export MERO_SIGN_KEY=...)
  cargo mero bundle --dev                  well-known dev key: fine locally, REFUSED by the registry

Next steps: e2e-test networked flows with merobox (https://github.com/calimero-network/merobox).
Full docs: https://calimero-network.github.io/core/
";

/// Prints the end-to-end workflow guide.
pub fn print() {
    println!("{GUIDE}");
}
