//! Serializable TDX quote types (mirroring `tdx_quote::Quote`), as attestation
//! documents carry them. Unchanged on the wire from v1's
//! `calimero-server-primitives` (`admin::Quote`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub header: QuoteHeader,
    pub body: QuoteBody,
    pub signature: String,
    pub attestation_key: String,
    pub certification_data: CertificationData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteHeader {
    pub version: u16,
    pub attestation_key_type: u16,
    pub tee_type: u32,
    pub qe_vendor_id: String,
    pub user_data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteBody {
    /// TDX version
    pub tdx_version: String,
    /// TEE Trusted Computing Base Security Version Number (16 bytes)
    pub tee_tcb_svn: String,
    /// Measurement of SEAM module (48 bytes)
    pub mrseam: String,
    /// Measurement of SEAM signer (48 bytes)
    pub mrsignerseam: String,
    /// SEAM attributes (8 bytes)
    pub seamattributes: String,
    /// Trust Domain attributes (8 bytes)
    pub tdattributes: String,
    /// Extended features available mask (8 bytes)
    pub xfam: String,
    /// Measurement Register of Trust Domain (48 bytes) - hash of kernel + initrd + app
    pub mrtd: String,
    /// Measurement of configuration (48 bytes)
    pub mrconfigid: String,
    /// Measurement of owner (48 bytes)
    pub mrowner: String,
    /// Measurement of owner configuration (48 bytes)
    pub mrownerconfig: String,
    /// Runtime Measurement Register 0 (48 bytes)
    pub rtmr0: String,
    /// Runtime Measurement Register 1 (48 bytes)
    pub rtmr1: String,
    /// Runtime Measurement Register 2 (48 bytes)
    pub rtmr2: String,
    /// Runtime Measurement Register 3 (48 bytes)
    pub rtmr3: String,
    /// Report data (64 bytes): nonce[32] || app_hash[32]
    pub reportdata: String,
    /// Optional second TEE TCB SVN (16 bytes) - TDX 1.5+
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tee_tcb_svn_2: Option<String>,
    /// Optional measurement of service TD (48 bytes) - TDX 1.5+
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mrservicetd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QeReportCertificationDataInfo {
    /// QE report (384 bytes hex)
    pub qe_report: String,
    /// ECDSA signature (hex)
    pub signature: String,
    /// QE authentication data (hex)
    pub qe_authentication_data: String,
    /// Inner certification data type
    pub certification_data_type: String,
    /// Inner certification data (hex)
    pub certification_data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum CertificationData {
    #[serde(rename = "pckIdPpidPlainCpusvnPcesvn")]
    PckIdPpidPlainCpusvnPcesvn(String),
    #[serde(rename = "pckIdPpidRSA2048CpusvnPcesvn")]
    PckIdPpidRSA2048CpusvnPcesvn(String),
    #[serde(rename = "pckIdPpidRSA3072CpusvnPcesvn")]
    PckIdPpidRSA3072CpusvnPcesvn(String),
    #[serde(rename = "pckLeafCert")]
    PckLeafCert(String),
    #[serde(rename = "pckCertChain")]
    PckCertChain(String),
    #[serde(rename = "qeReportCertificationData")]
    QeReportCertificationData(QeReportCertificationDataInfo),
    #[serde(rename = "platformManifest")]
    PlatformManifest(String),
}

// Conversion from tdx_quote::Quote to our serializable Quote type
impl TryFrom<tdx_quote::Quote> for Quote {
    type Error = String;

    fn try_from(quote: tdx_quote::Quote) -> Result<Self, Self::Error> {
        use tdx_quote::CertificationData as TdxCert;
        use tdx_quote::CertificationDataInner;

        // Extract method results first to avoid borrow issues
        let mrtd = hex::encode(quote.mrtd());
        let rtmr0 = hex::encode(quote.rtmr0());
        let rtmr1 = hex::encode(quote.rtmr1());
        let rtmr2 = hex::encode(quote.rtmr2());
        let rtmr3 = hex::encode(quote.rtmr3());
        let reportdata = hex::encode(quote.report_input_data());

        Ok(Self {
            header: QuoteHeader {
                version: quote.header.version,
                attestation_key_type: quote.header.attestation_key_type as u16,
                tee_type: quote.header.tee_type as u32,
                qe_vendor_id: hex::encode(quote.header.qe_vendor_id),
                user_data: hex::encode(quote.header.user_data),
            },
            body: QuoteBody {
                tdx_version: match quote.body.tdx_version {
                    tdx_quote::TDXVersion::One => "1.0".to_string(),
                    tdx_quote::TDXVersion::OnePointFive => "1.5".to_string(),
                },
                tee_tcb_svn: hex::encode(quote.body.tee_tcb_svn),
                mrseam: hex::encode(quote.body.mrseam),
                mrsignerseam: hex::encode(quote.body.mrsignerseam),
                seamattributes: hex::encode(quote.body.seamattributes),
                tdattributes: hex::encode(quote.body.tdattributes),
                xfam: hex::encode(quote.body.xfam),
                mrtd,
                mrconfigid: hex::encode(quote.body.mrconfigid),
                mrowner: hex::encode(quote.body.mrowner),
                mrownerconfig: hex::encode(quote.body.mrownerconfig),
                rtmr0,
                rtmr1,
                rtmr2,
                rtmr3,
                reportdata,
                tee_tcb_svn_2: quote.body.tee_tcb_svn_2.map(hex::encode),
                mrservicetd: quote.body.mrservicetd.map(hex::encode),
            },
            signature: hex::encode(quote.signature.to_bytes()),
            attestation_key: hex::encode(quote.attestation_key.to_sec1_bytes()),
            certification_data: match quote.certification_data {
                TdxCert::PckIdPpidPlainCpusvnPcesvn(data) => {
                    CertificationData::PckIdPpidPlainCpusvnPcesvn(hex::encode(&data))
                }
                TdxCert::PckIdPpidRSA2048CpusvnPcesvn(data) => {
                    CertificationData::PckIdPpidRSA2048CpusvnPcesvn(hex::encode(&data))
                }
                TdxCert::PckIdPpidRSA3072CpusvnPcesvn(data) => {
                    CertificationData::PckIdPpidRSA3072CpusvnPcesvn(hex::encode(&data))
                }
                TdxCert::PckLeafCert(data) => CertificationData::PckLeafCert(hex::encode(&data)),
                TdxCert::PckCertChain(data) => CertificationData::PckCertChain(hex::encode(&data)),
                TdxCert::QeReportCertificationData(data) => {
                    // Properly serialize the nested QeReportCertificationData structure
                    let (cert_type, cert_data) = match &data.certification_data {
                        CertificationDataInner::PckIdPpidPlainCpusvnPcesvn(d) => {
                            ("PckIdPpidPlainCpusvnPcesvn", hex::encode(d))
                        }
                        CertificationDataInner::PckIdPpidRSA2048CpusvnPcesvn(d) => {
                            ("PckIdPpidRSA2048CpusvnPcesvn", hex::encode(d))
                        }
                        CertificationDataInner::PckIdPpidRSA3072CpusvnPcesvn(d) => {
                            ("PckIdPpidRSA3072CpusvnPcesvn", hex::encode(d))
                        }
                        CertificationDataInner::PckLeafCert(d) => ("PckLeafCert", hex::encode(d)),
                        CertificationDataInner::PckCertChain(d) => ("PckCertChain", hex::encode(d)),
                        CertificationDataInner::PlatformManifest(d) => {
                            ("PlatformManifest", hex::encode(d))
                        }
                        // Return error for unknown inner certification data variants
                        _ => {
                            return Err(
                                "Unknown CertificationDataInner variant encountered".to_string()
                            );
                        }
                    };

                    CertificationData::QeReportCertificationData(QeReportCertificationDataInfo {
                        qe_report: hex::encode(&data.qe_report),
                        signature: hex::encode(data.signature.to_bytes()),
                        qe_authentication_data: hex::encode(&data.qe_authentication_data),
                        certification_data_type: cert_type.to_string(),
                        certification_data: cert_data,
                    })
                }
                TdxCert::PlatformManifest(data) => {
                    CertificationData::PlatformManifest(hex::encode(&data))
                }
                // Return error for unknown certification data variants
                _ => return Err("Unknown CertificationData variant encountered".to_string()),
            },
        })
    }
}
