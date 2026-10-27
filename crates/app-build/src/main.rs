use std::env;
use std::fs;
use std::path::PathBuf;

use calimero_app_build::build_component;
use eyre::Result as EyreResult;

fn main() -> EyreResult<()> {
    let mut args = env::args().skip(1);
    let usage = "usage: mero-app-build <crate-dir> [--out <file>]";
    let crate_dir = PathBuf::from(args.next().ok_or_else(|| eyre::eyre!(usage))?);
    let out = match args.next().as_deref() {
        None => crate_dir.join("app.component.wasm"),
        Some("--out") => PathBuf::from(args.next().ok_or_else(|| eyre::eyre!(usage))?),
        Some(_) => eyre::bail!(usage),
    };
    let built = build_component(&crate_dir, &crate_dir.join("target"))?;
    fs::write(&out, &built.component)?;
    println!("wrote {} ({} bytes)", out.display(), built.component.len());
    Ok(())
}
