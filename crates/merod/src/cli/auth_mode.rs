use calimero_server::config::AuthMode;
use clap::ValueEnum;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum AuthModeArg {
    Proxy,
    Embedded,
}

impl From<AuthModeArg> for AuthMode {
    fn from(value: AuthModeArg) -> Self {
        match value {
            AuthModeArg::Proxy => AuthMode::Proxy,
            AuthModeArg::Embedded => AuthMode::Embedded,
        }
    }
}
