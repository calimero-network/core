use std::io::Read;
use std::path::PathBuf;

use clap::Args;
use eyre::{bail, Result as EyreResult, WrapErr};
use mero_auth::provisioning::{
    admin_creds_from_env, admin_password_from_env, strip_trailing_newline, ADMIN_PASSWORD_ENV,
    ADMIN_PASSWORD_FILE_ENV, ADMIN_USER_ENV,
};

/// Admin-account credentials for embedded auth.
///
/// The password deliberately has no bare `--admin-password <VALUE>` flag: a
/// password on the command line leaks through process listings and shell
/// history. It is accepted only via file, stdin, or the environment — and it
/// is consumed on the spot to derive the root key, never stored anywhere.
#[derive(Debug, Args)]
pub struct AdminCredArgs {
    /// Admin username for embedded auth (or set MERO_AUTH_ADMIN_USER)
    #[clap(long, value_name = "NAME")]
    pub admin_user: Option<String>,

    /// Read the admin password from this file (a single trailing newline is
    /// stripped)
    #[clap(long, value_name = "PATH", conflicts_with = "admin_password_stdin")]
    pub admin_password_file: Option<PathBuf>,

    /// Read the admin password from stdin
    #[clap(long)]
    pub admin_password_stdin: bool,
}

impl AdminCredArgs {
    /// True when any credential flag was passed explicitly.
    pub const fn provided(&self) -> bool {
        self.admin_user.is_some() || self.admin_password_file.is_some() || self.admin_password_stdin
    }

    /// Resolve credentials from flags, falling back to the environment
    /// (`MERO_AUTH_ADMIN_USER` + `MERO_AUTH_ADMIN_PASSWORD[_FILE]`).
    ///
    /// Returns `Ok(None)` only when nothing was provided at all; a partial
    /// specification (user without password, or vice versa) is an error.
    pub fn resolve(&self) -> EyreResult<Option<(String, String)>> {
        let username = self.admin_user.clone().or_else(|| {
            std::env::var(ADMIN_USER_ENV)
                .ok()
                .filter(|user| !user.is_empty())
        });

        let password = if let Some(path) = &self.admin_password_file {
            let raw = std::fs::read_to_string(path)
                .wrap_err_with(|| format!("failed to read admin password file {path:?}"))?;
            Some(strip_trailing_newline(raw))
        } else if self.admin_password_stdin {
            let mut raw = String::new();
            let _ = std::io::stdin()
                .read_to_string(&mut raw)
                .wrap_err("failed to read the admin password from stdin")?;
            Some(strip_trailing_newline(raw))
        } else {
            None
        };

        match (username, password) {
            (Some(user), Some(password)) => {
                if password.is_empty() {
                    bail!("the admin password must not be empty");
                }
                Ok(Some((user, password)))
            }
            (Some(user), None) => match admin_password_from_env()? {
                Some(password) => Ok(Some((user, password))),
                None => bail!(
                    "an admin user was provided but no password; pass \
                     --admin-password-file or --admin-password-stdin, or set \
                     {ADMIN_PASSWORD_ENV} or {ADMIN_PASSWORD_FILE_ENV}"
                ),
            },
            (None, Some(_)) => bail!(
                "an admin password was provided but no user; pass --admin-user \
                 or set {ADMIN_USER_ENV}"
            ),
            (None, None) => admin_creds_from_env(),
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::AdminCredArgs;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[clap(flatten)]
        admin: AdminCredArgs,
    }

    #[test]
    fn password_file_and_stdin_are_mutually_exclusive() {
        assert!(TestCli::try_parse_from([
            "merod",
            "--admin-password-file",
            "/dev/null",
            "--admin-password-stdin",
        ])
        .is_err());
    }

    #[test]
    fn no_flags_parse_as_not_provided() {
        let cli = TestCli::try_parse_from(["merod"]).unwrap();
        assert!(!cli.admin.provided());
    }
}
