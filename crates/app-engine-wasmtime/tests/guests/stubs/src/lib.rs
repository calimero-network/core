//! Test guest: exercises the nine D44 stubs (stdin, exit, terminals, filesystem).

use wasi::cli::{environment, exit, stdin, terminal_stderr, terminal_stdin, terminal_stdout};
use wasi::filesystem::preopens;

wit_bindgen::generate!({
    inline: r#"
        package test:stubs;

        world stubs {
            include calimero:app/app@0.5.1;
            import wasi:cli/stdin@0.2.12;
            import wasi:cli/exit@0.2.12;
            import wasi:cli/terminal-stdin@0.2.12;
            import wasi:cli/terminal-stdout@0.2.12;
            import wasi:cli/terminal-stderr@0.2.12;
            import wasi:filesystem/preopens@0.2.12;
        }
    "#,
    path: "../../../../app-wit/wit",
    world: "test:stubs/stubs",
    generate_all,
});

const ABI_JSON: &[u8] = br#"{"abiVersion":1,"world":"calimero:app@0.5.1","methods":{"preopens":{"kind":"call"},"open":{"kind":"call"},"stdin":{"kind":"call"},"terminals":{"kind":"call"},"env":{"kind":"call"},"exit":{"kind":"call"}}}"#;

#[link_section = "calimero:abi"]
#[used]
static ABI: [u8; ABI_JSON.len()] = {
    let mut out = [0u8; ABI_JSON.len()];
    let mut i = 0;
    while i < ABI_JSON.len() {
        out[i] = ABI_JSON[i];
        i += 1;
    }
    out
};

struct App;

impl Guest for App {
    fn handle(_kind: InvocationKind, method: String, _args: Vec<u8>) -> Result<Vec<u8>, AppError> {
        match method.as_str() {
            "preopens" => Ok((preopens::get_directories().len() as u32).to_le_bytes().to_vec()),
            "open" => Ok(vec![u8::from(std::fs::File::open("/etc/hosts").is_err())]),
            "stdin" => {
                let read = stdin::get_stdin().blocking_read(1_024).unwrap_or_default();
                Ok((read.len() as u32).to_le_bytes().to_vec())
            }
            "terminals" => Ok(vec![
                u8::from(terminal_stdin::get_terminal_stdin().is_none()),
                u8::from(terminal_stdout::get_terminal_stdout().is_none()),
                u8::from(terminal_stderr::get_terminal_stderr().is_none()),
            ]),
            "env" => Ok(vec![
                environment::get_environment().len() as u8,
                environment::get_arguments().len() as u8,
                u8::from(environment::initial_cwd().is_none()),
            ]),
            "exit" => {
                exit::exit(Err(()));
                Err(AppError { code: "ExitReturned".into(), message: String::new() })
            }
            _ => Err(AppError { code: "NoMethod".into(), message: String::new() }),
        }
    }
}

export!(App);
