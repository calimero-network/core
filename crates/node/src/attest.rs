//! Quote generation for this node; the document and its verification live in
//! `calimero-client`, shared with every client.

pub use calimero_client::attest::*;

/// Where quotes come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Development: a mock quote with the same report-data layout.
    Mock,
    /// A real TDX quote from the TDX module (configfs-tsm).
    Tdx,
}

/// Produce a quote over `report`.
pub fn quote(mode: Mode, report: [u8; 64]) -> eyre::Result<Vec<u8>> {
    match mode {
        Mode::Mock => {
            let mut q = MOCK_QUOTE_HEADER.to_vec();
            q.extend_from_slice(&report);
            q.resize(256, 0);
            Ok(q)
        }
        Mode::Tdx => calimero_tee_attestation::generate_attestation(report)
            .map(|r| r.quote_bytes)
            .map_err(|e| eyre::eyre!("TDX quote: {e:?}")),
    }
}
