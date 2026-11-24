//! `meroctl app codegen` without meroctl's build: the SDKs generate their app clients with it
//! (`scripts/app-codegen.sh`).

fn main() -> eyre::Result<()> {
    calimero_app_build::abi_tools::codegen(std::env::args().skip(1))
}
