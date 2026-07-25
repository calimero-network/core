//! `cargo mero new`: scaffold a standalone Calimero app from embedded templates.
//! Replaces the "hand-copy apps/kv-store" onboarding path: the generated app
//! builds, tests, and bundles cleanly through the rest of the toolchain.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use eyre::{bail, Context, Result};

use crate::{templates, NewArgs};

pub fn run(args: &NewArgs) -> Result<()> {
    validate_name(&args.name)?;

    let dir = args
        .path
        .clone()
        .unwrap_or_else(|| Utf8PathBuf::from(&args.name));

    if dir.exists() && dir.read_dir_utf8()?.next().is_some() {
        bail!("target directory `{dir}` already exists and is not empty");
    }

    let name_snake = args.name.replace('-', "_");
    let name_pascal = to_pascal(&args.name);
    let subst = |tmpl: &str| {
        tmpl.replace("{{name}}", &args.name)
            .replace("{{name_snake}}", &name_snake)
            .replace("{{name_pascal}}", &name_pascal)
            .replace(templates::TAG_PLACEHOLDER, &args.sdk_version)
    };

    for (out, tmpl) in templates::FILES {
        write_file(&dir.join(out), &subst(tmpl))?;
    }

    println!("Scaffolded `{}` at {dir}", args.name);
    println!();
    println!("Next steps:");
    println!("  cd {dir}");
    println!("  cargo mero build      # compile -> wasm-opt -> embed ABI");
    println!("  cargo mero test       # TestHost + convergence tests (no node needed)");
    println!(
        "  cargo mero bundle --dev   # signed dist/com.example.{}.mpk",
        args.name
    );

    Ok(())
}

/// Crate names must match `[a-z][a-z0-9-]*`: a valid Rust package name that is
/// also a clean reverse-DNS suffix for the `com.example.<name>` app id.
fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let ok = matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !ok {
        bail!(
            "invalid app name `{name}`: use lowercase letters, digits, and dashes, \
             starting with a letter (e.g. `my-app`)"
        );
    }
    Ok(())
}

/// `my-app` -> `MyApp`, for the state struct name.
fn to_pascal(name: &str) -> String {
    name.split('-')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn write_file(path: &Utf8Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).wrap_err_with(|| format!("failed to create {parent}"))?;
    }
    fs::write(path, contents).wrap_err_with(|| format!("failed to write {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffold(name: &str, dir: &Utf8Path) {
        let args = NewArgs {
            name: name.to_owned(),
            sdk_version: "0.11.0-rc.17".to_owned(),
            path: Some(dir.to_owned()),
        };
        run(&args).unwrap();
    }

    #[test]
    fn scaffold_creates_expected_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().join("my-app")).unwrap();
        scaffold("my-app", &dir);

        for rel in [
            "Cargo.toml",
            "build.rs",
            "src/lib.rs",
            "tests/converge.rs",
            "README.md",
            ".gitignore",
        ] {
            assert!(dir.join(rel).exists(), "missing {rel}");
        }
    }

    #[test]
    fn scaffold_substitutes_names() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().join("my-app")).unwrap();
        scaffold("my-app", &dir);

        let lib = fs::read_to_string(dir.join("src/lib.rs")).unwrap();
        assert!(lib.contains("pub struct MyApp"), "PascalCase struct name");
        assert!(!lib.contains("{{"), "no placeholders left in lib.rs");

        let cargo = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("name = \"my-app\""), "kebab crate name");
        assert!(cargo.contains("tag = \"0.11.0-rc.17\""), "sdk git tag");
        assert!(
            cargo.contains("calimero-wasm-abi = \"=0.11.0-rc.17\""),
            "wasm-abi exact pin tracks the sdk tag"
        );
        assert!(cargo.contains("com.example.my-app"), "app id");
        assert!(!cargo.contains("{{"), "no placeholders left in Cargo.toml");

        let converge = fs::read_to_string(dir.join("tests/converge.rs")).unwrap();
        assert!(converge.contains("use my_app::MyApp;"), "snake + pascal");
    }

    #[test]
    fn scaffold_is_valid_cargo_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().join("my-app")).unwrap();
        scaffold("my-app", &dir);

        // --no-deps + --offline: parse and validate the manifest without
        // resolving the git SDK deps (which would hit the network).
        let meta = cargo_metadata::MetadataCommand::new()
            .manifest_path(dir.join("Cargo.toml"))
            .no_deps()
            .other_options(["--offline".to_owned()])
            .exec()
            .expect("scaffolded Cargo.toml must be valid cargo metadata");

        let pkg = meta
            .packages
            .iter()
            .find(|p| p.name.as_str() == "my-app")
            .expect("package present in metadata");

        // The [package.metadata.calimero] table must load through meta::load.
        let dir_ref = pkg.manifest_path.parent().unwrap();
        let bundle = crate::meta::load(&meta, dir_ref).unwrap();
        assert_eq!(bundle.package, "com.example.my-app");
    }

    #[test]
    fn rejects_invalid_crate_names() {
        for bad in ["My App!", "1app", "-app", "app_name", "App", ""] {
            assert!(validate_name(bad).is_err(), "`{bad}` should be rejected");
        }
        for good in ["my-app", "app", "a1", "kv-store-2"] {
            assert!(validate_name(good).is_ok(), "`{good}` should be accepted");
        }
    }

    #[test]
    fn refuses_existing_non_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8PathBuf::from_path_buf(tmp.path().to_owned()).unwrap();
        fs::write(dir.join("existing.txt"), "x").unwrap();

        let args = NewArgs {
            name: "my-app".to_owned(),
            sdk_version: "0.11.0-rc.17".to_owned(),
            path: Some(dir),
        };
        assert!(run(&args).is_err());
    }
}
