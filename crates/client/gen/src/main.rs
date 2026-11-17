//! `cargo run -p calimero-client-gen`: regenerate `crates/client/src/generated/`
//! from `docs/public/openapi.json`. `--check` only says whether it is current.

use eyre::{bail, Result as EyreResult};

use calimero_client_gen::{differing, generate_at, output_dir, repo_root, write};

fn main() -> EyreResult<()> {
    let check = std::env::args().skip(1).any(|a| a == "--check");
    let root = repo_root();
    let generated = generate_at(&root)?;
    let out = output_dir(&root);
    if check {
        let fresh =
            std::env::temp_dir().join(format!("calimero-client-gen-{}", std::process::id()));
        write(&generated, &fresh)?;
        let names: Vec<&str> = generated.files.iter().map(|(n, _)| *n).collect();
        let stale = differing(&fresh, &out, &names);
        let _ = std::fs::remove_dir_all(&fresh);
        if !stale.is_empty() {
            bail!(
                "{} differ from what the document generates: run `cargo run -p calimero-client-gen`",
                stale.join(", ")
            );
        }
        println!("crates/client/src/generated is current");
        return Ok(());
    }
    write(&generated, &out)?;
    println!(
        "crates/client/src/generated: {} methods; left out (event streams): {}",
        generated.methods,
        generated.streams.join(", ")
    );
    Ok(())
}
