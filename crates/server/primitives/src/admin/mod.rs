use std::collections::BTreeMap;

use calimero_context_config::types::SignedGroupOpenInvitation;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId, GroupMemberRole};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{AccountId, DeviceId, MemberIdentity, PublicKey};
use calimero_primitives::metadata::MetadataRecord;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Empty;

// -------------------------------------------- Application API --------------------------------------------
/// Install by coordinates: no URL, so the node can only fetch from its own
/// `[registry]`. `deny_unknown_fields` refuses a stale body carrying `url`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallApplicationRequest {
    pub package: String,
    pub version: String,
}

impl InstallApplicationRequest {
    pub const fn new(package: String, version: String) -> Self {
        Self { package, version }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationInstallResponseData {
    pub application_id: ApplicationId,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallApplicationResponse {
    pub data: ApplicationInstallResponseData,
}

impl InstallApplicationResponse {
    pub const fn new(application_id: ApplicationId) -> Self {
        Self {
            data: ApplicationInstallResponseData { application_id },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallDevApplicationRequest {
    pub path: Utf8PathBuf,
}

impl InstallDevApplicationRequest {
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallApplicationResponseData {
    pub application_id: ApplicationId,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallApplicationResponse {
    pub data: UninstallApplicationResponseData,
}

impl UninstallApplicationResponse {
    pub const fn new(application_id: ApplicationId) -> Self {
        Self {
            data: UninstallApplicationResponseData { application_id },
        }
    }
}

// -------------------------------------------- Package Management API --------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPackagesResponse {
    pub packages: Vec<String>,
}

impl ListPackagesResponse {
    pub const fn new(packages: Vec<String>) -> Self {
        Self { packages }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListVersionsResponse {
    pub versions: Vec<String>,
}

impl ListVersionsResponse {
    pub const fn new(versions: Vec<String>) -> Self {
        Self { versions }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetLatestVersionResponse {
    pub application_id: Option<ApplicationId>,
    /// Version string of the latest release (e.g. "1.0.0")
    pub version: Option<String>,
}

impl GetLatestVersionResponse {
    pub const fn new(application_id: Option<ApplicationId>, version: Option<String>) -> Self {
        Self {
            application_id,
            version,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListApplicationResponseData {
    pub apps: Vec<Application>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListApplicationsResponse {
    pub data: ListApplicationResponseData,
}

impl ListApplicationsResponse {
    pub const fn new(apps: Vec<Application>) -> Self {
        Self {
            data: ListApplicationResponseData { apps },
        }
    }
}

/// One locally-retained bytecode version of an application's package, as
/// returned by `GET /admin-api/applications/:application_id/versions`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationVersionEntry {
    pub version: String,
    pub blob_id: String,
    pub size: u64,
    pub package: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListApplicationVersionsResponse {
    pub data: Vec<ApplicationVersionEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetApplicationResponseData {
    pub application: Option<Application>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetApplicationResponse {
    pub data: GetApplicationResponseData,
}

impl GetApplicationResponse {
    pub const fn new(application: Option<Application>) -> Self {
        Self {
            data: GetApplicationResponseData { application },
        }
    }
}

// No `rename_all`: the query key on the wire is `service_name`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GetApplicationAbiQuery {
    pub service_name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetApplicationAbiResponse {
    /// The application's `wasm-abi/1` manifest, verbatim - read from the app
    /// row's latest-fetched bytecode, not any context's pinned version.
    pub data: serde_json::Value,
}

impl GetApplicationAbiResponse {
    #[must_use]
    pub const fn new(data: serde_json::Value) -> Self {
        Self { data }
    }
}
// -------------------------------------------- Context API --------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateContextRequest {
    pub application_id: ApplicationId,
    /// Which service from the application bundle to run. Optional for single-service apps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    pub context_seed: Option<Hash>,
    pub initialization_params: Vec<u8>,
    pub group_id: String,
    pub identity_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl CreateContextRequest {
    pub const fn new(
        application_id: ApplicationId,
        context_seed: Option<Hash>,
        initialization_params: Vec<u8>,
        group_id: String,
        identity_secret: Option<String>,
    ) -> Self {
        Self {
            application_id,
            service_name: None,
            context_seed,
            initialization_params,
            group_id,
            identity_secret,
            name: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextResponseData {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default)]
    pub group_created: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextResponse {
    pub data: CreateContextResponseData,
}

impl CreateContextResponse {
    pub fn new(context_id: ContextId, member_public_key: PublicKey) -> Self {
        Self {
            data: CreateContextResponseData {
                context_id,
                member_public_key,
                group_id: None,
                group_created: false,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteContextApiRequest {}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedContextResponseData {
    pub is_deleted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteContextResponse {
    pub data: DeletedContextResponseData,
}

impl DeleteContextResponse {
    pub const fn new(is_deleted: bool) -> Self {
        Self {
            data: DeletedContextResponseData { is_deleted },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextResponse {
    pub data: Context,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextStorageResponseData {
    pub size_in_bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextStorageResponse {
    pub data: GetContextStorageResponseData,
}

impl GetContextStorageResponse {
    pub const fn new(size_in_bytes: u64) -> Self {
        Self {
            data: GetContextStorageResponseData { size_in_bytes },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextIdentitiesResponseData {
    pub identities: Vec<PublicKey>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextIdentitiesResponse {
    pub data: ContextIdentitiesResponseData,
}

impl GetContextIdentitiesResponse {
    pub const fn new(identities: Vec<PublicKey>) -> Self {
        Self {
            data: ContextIdentitiesResponseData { identities },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAliasesResponse<T> {
    #[serde(bound(deserialize = "T: Ord + Deserialize<'de>"))]
    pub data: BTreeMap<Alias<T>, T>,
}

impl<T> ListAliasesResponse<T> {
    pub fn new(data: BTreeMap<Alias<T>, T>) -> Self {
        Self { data }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextWithGroup {
    #[serde(flatten)]
    pub context: Context,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextsResponseData {
    pub contexts: Vec<ContextWithGroup>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextsResponse {
    pub data: GetContextsResponseData,
}

impl GetContextsResponse {
    pub const fn new(contexts: Vec<ContextWithGroup>) -> Self {
        Self {
            data: GetContextsResponseData { contexts },
        }
    }
}

/// Per-context application switch. Code-only: migrations are declared in the
/// app's embedded ABI and resolved by the node during a group upgrade — the
/// caller never names a migrate method.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateContextApplicationRequest {
    pub application_id: ApplicationId,
    pub executor_public_key: PublicKey,
}

impl UpdateContextApplicationRequest {
    pub const fn new(application_id: ApplicationId, executor_public_key: PublicKey) -> Self {
        Self {
            application_id,
            executor_public_key,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateContextApplicationResponse {
    pub data: Empty,
}

impl Default for UpdateContextApplicationResponse {
    fn default() -> Self {
        Self::new()
    }
}

impl UpdateContextApplicationResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}

// -------------------------------------------- Identity API ----------------------------------------
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContextIdentityResponseData {
    pub public_key: PublicKey,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContextIdentityResponse {
    pub data: GenerateContextIdentityResponseData,
}

impl GenerateContextIdentityResponse {
    pub const fn new(public_key: PublicKey) -> Self {
        Self {
            data: GenerateContextIdentityResponseData { public_key },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateAliasRequest<T: AliasKind> {
    pub alias: Alias<T>,
    #[serde(flatten)]
    pub value: T::Value,
}

pub trait AliasKind {
    type Value;

    fn from_value(data: Self::Value) -> Self;
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextIdAlias {
    pub context_id: ContextId,
}

impl AliasKind for ContextId {
    type Value = CreateContextIdAlias;

    fn from_value(data: Self::Value) -> Self {
        data.context_id
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct CreateContextIdentityAlias {
    pub identity: PublicKey,
}

impl AliasKind for PublicKey {
    type Value = CreateContextIdentityAlias;

    fn from_value(data: Self::Value) -> Self {
        data.identity
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApplicationIdAlias {
    pub application_id: ApplicationId,
}

impl AliasKind for ApplicationId {
    type Value = CreateApplicationIdAlias;

    fn from_value(data: Self::Value) -> Self {
        data.application_id
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateDeviceIdAlias {
    pub device_id: DeviceId,
}

impl AliasKind for DeviceId {
    type Value = CreateDeviceIdAlias;

    fn from_value(data: Self::Value) -> Self {
        data.device_id
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAliasResponse {
    pub data: Empty,
}

impl Default for CreateAliasResponse {
    fn default() -> Self {
        Self::new()
    }
}

impl CreateAliasResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAliasResponse {
    pub data: Empty,
}

impl Default for DeleteAliasResponse {
    fn default() -> Self {
        Self::new()
    }
}

impl DeleteAliasResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupAliasResponse<T> {
    pub data: LookupAliasResponseData<T>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupAliasResponseData<T> {
    pub value: Option<T>,
}

impl<T> LookupAliasResponseData<T> {
    pub const fn new(value: Option<T>) -> Self {
        Self { value }
    }
}

// -------------------------------------------- Misc API --------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GetPeersCountResponse {
    pub count: usize,
}

impl GetPeersCountResponse {
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self { count }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncContextResponse {
    pub data: Empty,
}

impl Default for SyncContextResponse {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncContextResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}

// -------------------------------------------- TEE API --------------------------------------------

// Serializable TDX Quote Types (mirrors tdx_quote::Quote structure)

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeeAttestRequest {
    /// Client-provided nonce for freshness (32 bytes as hex string)
    pub nonce: String,
    /// Optional application ID to include in attestation
    /// If provided, the application's bytecode BlobId (hash) will be included in report_data
    pub application_id: Option<ApplicationId>,
}

impl TeeAttestRequest {
    pub fn new(nonce: String, application_id: Option<ApplicationId>) -> Self {
        Self {
            nonce,
            application_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FleetJoinRequest {
    pub group_id: String,
}

impl Validate for FleetJoinRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.group_id.len() != 64 {
            errors.push(ValidationError::InvalidLength {
                field: "group_id",
                expected: 64,
                actual: self.group_id.len(),
            });
        } else if hex::decode(&self.group_id).is_err() {
            errors.push(ValidationError::InvalidHexEncoding {
                field: "group_id",
                reason: "not valid hex".to_owned(),
            });
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FleetJoinResponse {
    pub status: String,
    pub group_id: String,
    pub namespace_id: String,
    pub public_key: String,
    /// Hex-encoded `AccountId` this replica speaks for.
    ///
    /// `public_key` is its signing key, which is NOT what a member listing
    /// reports — membership is recorded against the account. Without this a
    /// caller cannot check that the replica was admitted, because it has no way
    /// to turn the key it was given into the id the listing uses.
    ///
    /// `serde(default)` so a response from a node predating the field still
    /// decodes here rather than failing outright.
    #[serde(default)]
    pub account: String,
    pub admitted: bool,
    /// `true` if the node successfully published `MemberSetAutoFollow` for
    /// itself after admission. `false` means admission succeeded but the
    /// node will NOT auto-join future contexts until the op is retried.
    #[serde(default)]
    pub auto_follow_enabled: bool,
    pub contexts_joined: Vec<String>,
}

/// Per-column on-disk byte estimates for a namespace.
///
/// Values are RocksDB approximations (`get_approximate_sizes_cf`) — sampled
/// from SST metadata, not exact. Sufficient for quota enforcement, not for
/// audit-level accounting. `total` is the sum of the individual column
/// fields; callers can use it directly rather than re-summing.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceUsageBytes {
    pub state: u64,
    pub private_state: u64,
    pub delta: u64,
    pub governance: u64,
    pub total: u64,
}

/// Per-namespace resource usage on this node.
/// Returned by `GET /admin-api/usage` in the `namespaces` list.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceUsage {
    pub namespace_id: String,
    pub context_count: u32,
    pub member_count: u32,
    pub subgroup_count: u32,
    pub bytes: NamespaceUsageBytes,
}

/// Response for `GET /admin-api/usage`. Reports per-namespace counts + byte
/// breakdown for every namespace this node participates in. Used by MDMA
/// to enforce plan limits (e.g. 1 GB free tier) and for operator dashboards.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageResponse {
    pub namespaces: Vec<NamespaceUsage>,
}

/// Response for `GET /admin-api/network/status`. Wire-format snapshot of
/// the local node's libp2p connectivity state — what relays we hold
/// reservations with, which rendezvous registrations are live, the
/// outcome of the latest DCUtR hole-punch per peer, and the most recent
/// AutoNAT v2 probe. Surfaced verbatim by `meroctl network status`.
///
/// All multiaddrs / peer ids are stringified, all timestamps are RFC3339
/// UTC, all status fields are flat strings (lowercase enum names). This
/// keeps the wire shape stable across libp2p upgrades and friendly to
/// consumers in any language.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatusResponse {
    pub local_peer_id: String,
    pub listen_addrs: Vec<String>,
    pub external_addrs: Vec<String>,
    pub relays: Vec<RelayStatusEntry>,
    pub rendezvous: Vec<RendezvousStatusEntry>,
    pub direct_upgrades: Vec<DirectUpgradeStatusEntry>,
    pub autonat: AutonatStatusEntry,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatusEntry {
    pub peer_id: String,
    /// One of: `discovered`, `requested`, `accepted`, `expired`.
    pub reservation_status: String,
    pub last_state_change: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RendezvousStatusEntry {
    pub peer_id: String,
    /// One of: `discovered`, `requested`, `registered`, `expired`.
    pub registration_status: String,
    pub last_state_change: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectUpgradeStatusEntry {
    pub peer_id: String,
    /// `succeeded` or `failed`. When `failed`, `reason` is populated.
    pub status: String,
    pub reason: Option<String>,
    pub connection_id: Option<String>,
    pub last_attempt: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutonatStatusEntry {
    /// One of: `unknown`, `public`, `private`.
    pub reachability: String,
    pub last_test_addr: Option<String>,
    /// `reachable`, `failed`, or `null` if no probe has landed.
    pub last_test_result: Option<String>,
    pub last_test_reason: Option<String>,
    pub last_test_observed_addr: Option<String>,
    pub last_test_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeInfoResponseData {
    /// Cloud provider (e.g., "gcp", "azure", "unknown")
    pub cloud_provider: String,
    /// OS image name (e.g., "ubuntu-2404-tdx-v20250115")
    pub os_image: String,
    /// MRTD extracted from TD report (48 bytes hex)
    pub mrtd: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeInfoResponse {
    pub data: TeeInfoResponseData,
}

impl TeeInfoResponse {
    pub fn new(cloud_provider: String, os_image: String, mrtd: String) -> Self {
        Self {
            data: TeeInfoResponseData {
                cloud_provider,
                os_image,
                mrtd,
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeAttestResponseData {
    /// Base64-encoded TDX quote
    /// The quote contains the report_data which the client must verify
    pub quote_b64: String,
    /// Parsed TDX quote structure
    pub quote: Quote,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeAttestResponse {
    pub data: TeeAttestResponseData,
}

impl TeeAttestResponse {
    pub fn new(quote_b64: String, quote: Quote) -> Self {
        Self {
            data: TeeAttestResponseData { quote_b64, quote },
        }
    }
}

// -------------------------------------------- Validation Implementations --------------------------------------------
//
// Validation Strategy:
// ====================
// These implementations focus on validating user-controlled string fields and size limits.
//
// Types like `ContextId`, `PublicKey`, and `ApplicationId` are validated during
// serde deserialization - they implement `FromStr` which performs format validation (hex
// decoding, length checks). If deserialization succeeds, the type is guaranteed valid.
//
// For request types containing only these strongly-typed fields, the `Validate` impl returns
// an empty Vec since no additional runtime validation is needed beyond what serde already does.
//
// This approach provides:
// 1. Type-safe validation at the deserialization boundary
// 2. Additional size/format checks for user-provided strings (method names, URLs, etc.)
// 3. Protection against oversized payloads that could cause resource exhaustion

use crate::validation::{
    helpers::{
        validate_bytes_size, validate_hex_string, validate_non_empty, validate_safe_path,
        validate_string_length,
    },
    Validate, ValidationError, MAX_INIT_PARAMS_SIZE, MAX_PACKAGE_NAME_LENGTH, MAX_VERSION_LENGTH,
};

impl Validate for InstallApplicationRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // Only the shape. What a coordinate may contain is decided where it
        // becomes a path segment, in `RegistryCoords`.
        [
            validate_non_empty(&self.package, "package"),
            validate_string_length(&self.package, "package", MAX_PACKAGE_NAME_LENGTH),
            validate_non_empty(&self.version, "version"),
            validate_string_length(&self.version, "version", MAX_VERSION_LENGTH),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

impl Validate for InstallDevApplicationRequest {
    fn validate(&self) -> Vec<ValidationError> {
        validate_safe_path(self.path.as_str(), "path")
            .into_iter()
            .collect()
    }
}

impl Validate for CreateContextRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if let Some(e) = validate_bytes_size(
            &self.initialization_params,
            "initialization_params",
            MAX_INIT_PARAMS_SIZE,
        ) {
            errors.push(e);
        }

        errors
    }
}

impl Validate for UpdateContextApplicationRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // All fields are typed (ApplicationId, PublicKey) with their own validation
        Vec::new()
    }
}

impl Validate for TeeAttestRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // Nonce must be exactly 64 hex characters (32 bytes)
        if let Some(e) = validate_hex_string(&self.nonce, "nonce", 32) {
            errors.push(e);
        }

        errors
    }
}

// -------------------------------------------- Group API --------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateGroupApiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    // `appKey` on the wire in BOTH directions: an old server knows only that
    // name, and the alias keeps taking `bytecodeId` from clients that send it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "appKey",
        alias = "bytecodeId"
    )]
    pub bytecode_id: Option<String>,
    pub application_id: ApplicationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_group_id: Option<String>,
}

impl Validate for CreateGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if let Some(ref bytecode_id) = self.bytecode_id {
            if bytecode_id.is_empty() {
                errors.push(ValidationError::EmptyField { field: "appKey" });
            }
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupApiResponse {
    pub data: CreateGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupApiResponseData {
    pub group_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateNamespaceApiRequest {
    pub application_id: ApplicationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Hex-encoded 32-byte bytecode blob id to pin the namespace to a
    /// specific installed version. Default: the application row's blob
    /// (latest fetched).
    // `appKey` on the wire in BOTH directions: an old server knows only that
    // name, and the alias keeps taking `bytecodeId` from clients that send it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "appKey",
        alias = "bytecodeId"
    )]
    pub bytecode_id: Option<String>,
}

impl Validate for CreateNamespaceApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNamespaceApiResponseData {
    pub namespace_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNamespaceApiResponse {
    pub data: CreateNamespaceApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteNamespaceApiRequest {}

impl Validate for DeleteNamespaceApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteNamespaceApiResponseData {
    pub is_deleted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteNamespaceApiResponse {
    pub data: DeleteNamespaceApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteGroupApiRequest {}

impl Validate for DeleteGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteGroupApiResponse {
    pub data: DeleteGroupApiResponseData,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteGroupApiResponseData {
    pub is_deleted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfoApiResponse {
    pub data: GroupInfoApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfoApiResponseData {
    pub group_id: String,
    // `appKey` on the wire: the rename is internal, the JSON is a client contract.
    #[serde(rename = "appKey")]
    pub bytecode_id: String,
    pub target_application_id: ApplicationId,
    pub member_count: u64,
    pub context_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_upgrade: Option<GroupUpgradeStatusApiData>,
    pub default_capabilities: u32,
    pub subgroup_visibility: String,
    /// Full metadata record for the group (name + opaque `data` map), or
    /// omitted if none has ever been set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataRecord>,
    /// Hex-encoded SHA-256 hash of the group's authorization-relevant
    /// state. Mirrors `contextStateHash` on context responses; lets
    /// clients poll for governance convergence across nodes.
    // Explicit rename pins the JSON name even if the Rust field is
    // refactored, matching the same pattern as `contextStateHash`.
    #[serde(rename = "groupStateHash")]
    pub group_state_hash: String,
}

/// A member's request that this node perform one intent on their behalf.
///
/// The intent travels in the clear to THIS node, which is fine and is the whole
/// point of the hop: the relay has to read the method and arguments to run them.
/// What must not travel in the clear is the intent on the *gossip* wire, and it
/// does not — the warrant commits to `H(method ‖ args)` and the detail is sealed
/// beside the operations.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PerformIntentApiRequest {
    /// The method to run.
    pub method: String,
    /// Its arguments, as the JSON the guest will receive.
    pub args_json: serde_json::Value,
    /// The author's consent: hex-encoded borsh of a `calimero_account::Warrant`.
    ///
    /// Opaque on this surface deliberately. It is a signed statement whose
    /// canonical form is its borsh encoding, and re-describing its fields as
    /// JSON would create a second spelling that could disagree with the bytes
    /// the signature covers.
    pub warrant: String,
    /// Hex-encoded borsh of the author's `AccountProof<DeviceCert>`, proving the
    /// key that signed the warrant is a device of the account it names.
    ///
    /// The author supplies only its OWN half. The executor's proof and signing
    /// key are attached by the node from its own credentials, because the
    /// warrant authorizes an operator ACCOUNT and the author has no business
    /// knowing which of that operator's processes will run it — that is the
    /// whole reason `Warrant::executor` is an account.
    pub author_proof: String,
}

impl Validate for PerformIntentApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.method.is_empty() {
            errors.push(ValidationError::EmptyField { field: "method" });
        }
        if self.warrant.is_empty() {
            errors.push(ValidationError::EmptyField { field: "warrant" });
        }
        if self.author_proof.is_empty() {
            errors.push(ValidationError::EmptyField {
                field: "authorProof",
            });
        }
        errors
    }
}

/// Where the accepted intent landed, so a client can wait for it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformIntentApiResponseData {
    /// The context's scope root after the run.
    ///
    /// This was a `deltaId` that the handler had no way to populate, so it was
    /// always `null` — a field that reports nothing is worse than no field,
    /// because a caller reasonably reads it as "no delta was produced". The
    /// execute path does not hand a delta id back (`ExecuteResponse` carries
    /// none), but it does return the new root, which answers the question a
    /// caller actually has: did this change anything?
    pub root_hash: String,
    /// The method's own return value.
    pub returns: Option<serde_json::Value>,
}

/// Wrapped in `data` like every neighbouring response, and
/// `NodeIdentityApiResponse` in particular — the closest sibling to this route,
/// added in the same account work.
///
/// The wrapper is not decoration. `mero-js`'s admin client types every call as
/// `post<{ data: T }>` and runs the result through one `unwrap`, so a flat
/// response is the one shape its generated method cannot consume without a
/// special case — and 51 of the 79 response structs here already wrap. Cheaper
/// to match the convention than to explain the exception in three client
/// libraries.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PerformIntentApiResponse {
    pub data: PerformIntentApiResponseData,
}

/// What a keyholder needs to know before it mints a warrant for this node.
///
/// A warrant binds `executor` to an **account**, and that account is the one
/// thing a client cannot derive: it is this node's, not the caller's, and it is a
/// content address rather than a key on any wire the client already reads. Making
/// the client ask `/admin-api/identity` separately and then guess whether the
/// grant exists is two round trips to answer one question — "can this relay run
/// my intent, and whose name do I put in the warrant?"
///
/// Answering both together is also what lets a client fail *before* signing.
/// A warrant naming the wrong executor, or a relay with no grant, is refused at
/// `POST .../intents` — after the author has spent a nonce from its monotonic
/// sequence on a warrant no relay will ever accept.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentRelayApiResponseData {
    /// The account a warrant for this node must name as its `executor`, hex.
    ///
    /// An account, not this node's signing key: one of the relay's processes
    /// re-keying must not void warrants already issued to it.
    pub executor_account: String,
    /// Whether this node may execute a delegated write in the group owning this
    /// context — the same question `POST .../intents` and every peer asks.
    ///
    /// `false` is not an error and not permanent: it is the state of a context no
    /// admin has opened to delegated execution yet, which is every context by
    /// default, since `CAN_AUTHOR_ON_BEHALF` is implied by neither membership nor
    /// admin and is never granted implicitly. Read it together with
    /// `grantedOnGroupId` below, which says which group a grant would have to be
    /// revoked on — or asked for.
    pub can_author_on_behalf: bool,
    /// The group owning this context, hex.
    pub group_id: String,
    /// The group whose capability row carries the grant, hex — or absent when no
    /// group reachable from here carries it.
    ///
    /// It says *where*, while `can_author_on_behalf` says *whether*, and the two
    /// are computed from one source so they cannot contradict each other:
    ///
    /// * **absent** — nobody has granted this node authorship anywhere it can
    ///   reach, and it is refused. Someone must grant it: on `groupId`, or once
    ///   on an ancestor this node inherits membership through.
    /// * **equal to `groupId`** — granted on this context's own group. Paired
    ///   with `canAuthorOnBehalf: true`.
    /// * **different from `groupId`** — granted on an ancestor and honoured
    ///   here, so also paired with `canAuthorOnBehalf: true`. The distinction is
    ///   what a caller needs in order to *change* it: a revoke or a narrowing
    ///   has to edit the group named here, not `groupId`, and one root grant of
    ///   this kind is typically covering an entire relay fleet.
    ///
    /// It still reports where a grant lives, never what is permitted:
    /// `canAuthorOnBehalf` remains the single authorization answer, and a client
    /// must not infer permission from this field alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_on_group_id: Option<String>,
}

/// Wrapped in `data` like every neighbouring response.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentRelayApiResponse {
    pub data: IntentRelayApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddGroupMembersApiRequest {
    pub members: Vec<GroupMemberApiInput>,
}

impl Validate for AddGroupMembersApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.members.is_empty() {
            errors.push(ValidationError::EmptyField { field: "members" });
        }
        for member in &self.members {
            if member.role == GroupMemberRole::ReadOnlyTee {
                errors.push(ValidationError::InvalidFormat {
                    field: "members[].role",
                    reason: "ReadOnlyTee role can only be assigned via TEE attestation".to_owned(),
                });
            }
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupMemberApiInput {
    /// The member's ACCOUNT - what every other verb on this resource names and
    /// what the listing returns.
    ///
    /// A KEY is accepted too, spelled identically — both are 32 bytes and both
    /// render as 64 hex, so this field cannot say which it carries and does not
    /// try. The node resolves it on apply: bytes matching a signing key bound in
    /// this namespace name that key's account; anything else is taken as an
    /// account as given.
    ///
    /// So a key that is not bound here reads as an account and adds a member
    /// nothing will match. That is the cost of one encoding, and it is the same
    /// shape as the existing rule below — an account is never checked for
    /// existence, because naming one this node has not converged on yet is the
    /// point.
    ///
    /// An ACCOUNT is taken as given - no local existence check. That asymmetry
    /// is the point: an account this node has not converged on yet is exactly
    /// what a key could never name, and refusing it would put back the case the
    /// key typing claimed to serve and could not. The cost is that a mistyped
    /// account writes a membership row nobody holds, which an admin removes the
    /// same way it was added.
    pub identity: MemberIdentity,
    pub role: GroupMemberRole,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveGroupMembersApiRequest {
    /// The members to remove, named by ACCOUNT — the principal the membership
    /// rows are keyed by. `GET .../members` returns these same ids.
    pub members: Vec<AccountId>,
}

impl Validate for RemoveGroupMembersApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.members.is_empty() {
            errors.push(ValidationError::EmptyField { field: "members" });
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListGroupMembersApiResponse {
    pub members: Vec<GroupMemberApiEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMemberApiEntry {
    /// The member's ACCOUNT — a member is a person here, and a person may hold
    /// several keys. Renders as 64 hex characters, as every id does; on the
    /// request side a key is distinguished by a `key:` tag, not by its encoding.
    pub identity: AccountId,
    pub role: GroupMemberRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ListGroupMembersQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberDeviceApiEntry {
    /// Renders as 64 hex characters - the form
    /// `POST /namespaces/:namespace_id/account/revoke` takes.
    pub device_id: DeviceId,
    /// The key this device's signatures carry, 64 hex. This is the join column
    /// against `GET /contexts/:id/identities`.
    pub signing_key: PublicKey,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberDevicesApiEntry {
    pub account: AccountId,
    pub devices: Vec<MemberDeviceApiEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMemberDevicesApiResponse {
    pub members: Vec<MemberDevicesApiEntry>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ListMemberDevicesQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupContextEntryResponse {
    pub context_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListGroupContextsApiResponse {
    pub data: Vec<GroupContextEntryResponse>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ListGroupContextsQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// A group upgrade names only the target application — whether and what to
/// migrate is resolved by the node from the apps' embedded ABIs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpgradeGroupApiRequest {
    pub target_application_id: ApplicationId,
    /// When `true`, emit one atomic `GroupOp::CascadeUpgrade` fanning out to
    /// every descendant subgroup whose `bytecode_id` matches the signed group's;
    /// when `false` (default), stay on the single-group path.
    #[serde(default)]
    pub cascade: bool,
    /// When `true`, a target build with no embedded ABI proceeds code-only
    /// (the operator asserts layout-compatibility) instead of being refused;
    /// when `false` (default), a missing target ABI refuses the upgrade.
    #[serde(default)]
    pub force_code_only: bool,
}

impl Validate for UpgradeGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // All fields are typed (ApplicationId, Option<PublicKey>, bool) with
        // their own validation
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeGroupApiResponse {
    pub data: UpgradeGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeGroupApiResponseData {
    pub group_id: String,
    pub status: String,
    /// Contexts this node enumerated for the upgrade. A per-node count, not a
    /// fleet one - fleet progress is the `migration-status` rollup.
    pub local_contexts_total: Option<u32>,
    /// Contexts this node has swapped to the target application.
    pub local_contexts_swapped: Option<u32>,
    /// Contexts whose swap failed on this node; a non-zero value is what
    /// `retry_group_upgrade` picks up.
    pub local_contexts_failed: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetGroupUpgradeStatusApiResponse {
    pub data: Option<GroupUpgradeStatusApiData>,
}

/// Per-group cascade migration status entry returned by `get_cascade_status`.
///
/// Mirrors [`GroupUpgradeStatusApiData`] for the upgrade snapshot, augmented
/// with `group_id` and the sticky `cascade_hlc` fence from the atomic
/// `CascadeUpgrade` op (opaque display string; `None` for non-cascade upgrades).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CascadeStatusApiEntry {
    /// Hex-encoded 32-byte group id.
    pub group_id: String,
    /// Upgrade snapshot for this group.
    pub upgrade: GroupUpgradeStatusApiData,
    /// HLC fence string from the atomic `CascadeUpgrade` op, or `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cascade_hlc: Option<String>,
}

/// Response returned by `GET .../groups/:namespace_id/cascade-status`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCascadeStatusApiResponse {
    pub data: Vec<CascadeStatusApiEntry>,
}

/// The freshest reported facts for a pinned-cohort member, surfaced by
/// `get_migration_status` (Task 6c.10). `null` for a member with no fresh
/// heartbeat (its `state` is then `"unknown"`).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberMigrationReportApiData {
    pub schema_version: u32,
    pub residue_auto: u64,
    /// Always 0. Held on the wire only because the released calimero-client-py
    /// the e2e suite runs still requires it; drop once that floor moves.
    #[serde(default)]
    pub residue_identity: u64,
    pub synced_up_to_hlc: u64,
    pub reported_at: u64,
    /// Member's self-reported pending-authored count (best-effort; 6f).
    #[serde(default)]
    pub authored_remaining: u64,
    /// Why this member's migration did not complete: `"check_aborted"`,
    /// `"apply_failed"`, or `"no_migration_path"` (the stranded-context reason).
    /// Absent when the member has no failure on record (its `state` is then
    /// `"migrated"`/`"in_progress"`/`"unknown"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration_failed: Option<String>,
}

/// One per-member row in the migration-status rollup: a pinned-cohort member,
/// its reported facts (if any), and the derived migration state.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberMigrationStatusApiEntry {
    /// The reporting device key, 64 hex. A member with two devices appears twice.
    pub peer: PublicKey,
    /// The account `peer` speaks for, 64 hex. Joins these rows to the
    /// account-keyed `GET /groups/:id/members`. `None` only from a node
    /// predating the field - a default would name a principal that exists nowhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountId>,
    /// The member's freshest reported facts, or `null` when it has no fresh
    /// heartbeat (in which case `state == "unknown"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<MemberMigrationReportApiData>,
    /// Derived state discriminant: `"migrated"`, `"in_progress"`, `"unknown"`,
    /// or `"failed"`.
    pub state: String,
}

/// Rollup counters across the pinned cohort (observability only — never a gate).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationStatusRollupApiData {
    pub migrated: usize,
    pub in_progress: usize,
    pub unknown: usize,
    /// Members whose migrate aborted (migration-check failed or apply errored).
    #[serde(default)]
    pub failed: usize,
    pub total: usize,
    /// `true` iff every pinned-cohort member reported a converged schema with
    /// zero residue. Any `unknown`, `failed`, or in-progress member keeps this
    /// `false`.
    pub all_migrated: bool,
    /// Count of members reporting `authored_remaining > 0` (owners with
    /// identity-gated entries still to re-sign; 6f, skew #1). Best-effort.
    #[serde(default)]
    pub members_pending_signature: usize,
}

/// Migration-status answer returned by `GET .../groups/:namespace_id/migration-status`.
///
/// The operator-facing "have all peers migrated?" rollup (Task 6c.10): the
/// pinned-cohort size, the per-member rows, and the `all_migrated` flag.
/// Observability only — this endpoint never gates a write or apply.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMigrationStatusApiResponse {
    pub target_version: u32,
    /// Size of the pinned cohort (the inherited-membership closure, minus any
    /// member excluded by the expand-entry HLC pin).
    pub expected_members: usize,
    /// The governance HLC the cohort was pinned at, as an opaque display string;
    /// `null` when there is no migration record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cohort_pinned_at_hlc: Option<String>,
    /// Unix timestamp when this node watched the cohort converge, or `null`
    /// while it has not. Durable, unlike `rollup.allMigrated`, which is
    /// recomputed from in-TTL heartbeats and lapses when a member goes quiet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet_completed_at: Option<u64>,
    pub rollup: MigrationStatusRollupApiData,
    pub members: Vec<MemberMigrationStatusApiEntry>,
}

/// Response returned by `POST .../groups/:namespace_id/migration/abort`.
///
/// Reports whether a pending migration was found and logically aborted. The
/// abort flips the group's migration target back to the pre-migration
/// application and drops the pending migration marker so not-yet-applied lazy
/// contexts stop migrating. It does not recall an already-committed v2 context.
/// Idempotent: aborting with nothing pending returns `aborted: false`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbortMigrationApiResponse {
    /// Hex-encoded 32-byte namespace id the abort targeted.
    pub namespace_id: String,
    /// `true` when a pending migration was flipped back; `false` for the
    /// idempotent no-op (nothing was pending).
    pub aborted: bool,
}

/// Request body for resyncing a stranded context from a peer. A resync
/// overwrites local state with a peer's, discarding any local DAG heads, so
/// `force` must be set when the context still holds them.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResyncContextApiRequest {
    #[serde(default)]
    pub force: bool,
}

impl Validate for ResyncContextApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResyncContextApiResponse {
    /// Hex-encoded 32-byte context id the resync targeted.
    pub context_id: String,
    /// `true` when the resync marker was set and a sync was triggered.
    pub resync_started: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupUpgradeStatusApiData {
    pub from_version: String,
    pub to_version: String,
    pub initiated_at: u64,
    pub initiated_by: PublicKey,
    pub status: String,
    /// Contexts this node enumerated for the upgrade. A per-node count, not a
    /// fleet one - fleet progress is the `migration-status` rollup.
    pub local_contexts_total: Option<u32>,
    /// Contexts this node has swapped to the target application.
    pub local_contexts_swapped: Option<u32>,
    /// Contexts whose swap failed on this node; a non-zero value is what
    /// `retry_group_upgrade` picks up.
    pub local_contexts_failed: Option<u32>,
    pub completed_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryGroupUpgradeApiRequest {}

impl Validate for RetryGroupUpgradeApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

/// What the pairing device minted, for the account holder to certify.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
/// Every field here is hex, including the two public keys, and that is
/// deliberate rather than an oversight against the usual `PublicKey` rendering.
///
/// This payload is a set of round-trip tokens: a caller copies them verbatim
/// from `pair-init` to `pair-complete`, which parses them back as hex. Nothing
/// compares them against a key from anywhere else — the signing key here is the
/// NEW device's, not one that appears in a member listing — so the convention
/// that matters is the one inside the payload, and it is uniform. Rendering one
/// field bs58 to match `PublicKey` elsewhere would make this payload mixed to
/// make the wider surface consistent, which is the worse trade.
///
/// That trade no longer has to be made: `PublicKey` renders hex everywhere now,
/// so the uniformity this payload chose locally is the rule globally.
pub struct PairDeviceInitApiResponseData {
    /// Hex-encoded `AccountId` this device will speak for once linked.
    pub account_id: String,
    /// Hex-encoded `DeviceId` — hand this to the account holder.
    pub device_id: String,
    /// Hex-encoded X25519 agreement key a scope key must be wrapped under to
    /// reach this device. Hand this to the account holder alongside the id.
    pub kem_public_key: String,
    /// Hex-encoded Ed25519 key this device signs its ops with.
    ///
    /// The account holder cannot derive this — it is minted here — and the
    /// certificate names it, so it has to travel with the other two.
    pub sign_public_key: String,
    /// Hex-encoded Ed25519 signature (64 bytes) by `signPublicKey` over the
    /// account, the device id and both keys above.
    ///
    /// Travels with them and `pair-complete` refuses without it, so the three
    /// values arrive as a statement by the device that minted them rather than
    /// as assertions by whoever relayed them.
    pub statement: String,
    /// The value to read out to the account holder, who compares it with what
    /// their `pair-complete` reports.
    ///
    /// A substituting attacker can re-sign its own statement, so this is the
    /// part it cannot fake: it would have to make its own keys derive the code
    /// the other side is reading.
    pub confirmation_code: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairDeviceInitApiResponse {
    pub data: PairDeviceInitApiResponseData,
}

/// What pairing established.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairDeviceCompleteApiResponseData {
    /// Hex-encoded `AccountId` the device now speaks for.
    pub account_id: String,
    /// Hex-encoded `DeviceId` that was linked.
    pub device_id: String,
    /// Whether the current scope key was wrapped and published for the device.
    ///
    /// `false` does not mean pairing failed — the link is what confers
    /// authority, and the device's own sync pull re-requests the key. It does
    /// mean the device cannot read until that lands.
    pub key_delivered: bool,
    /// The confirmation code for the key material this certified — the same value
    /// the request carried, echoed so the operator can see what the certificate
    /// names.
    pub confirmation_code: String,
    /// Hex-encoded borsh of the `AccountProof<DeviceCert>` this pairing minted, so
    /// the device can present itself without reading the DAG it is not a member of.
    /// Not a secret: it proves nothing without the device key it names.
    pub credential: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairDeviceCompleteApiResponse {
    pub data: PairDeviceCompleteApiResponseData,
}

/// Adopt an existing account on this node and mint one device for it, across a
/// set of namespaces. One device for the whole set, so the response carries one
/// id, one key pair and one code however many namespaces it covers.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountPairInitApiRequest {
    /// Hex-encoded epoch-0 root **public** key (32 bytes). Named for the half it
    /// carries: private and public are both 32 hex bytes, and the private root
    /// leaves the node only via `merod account export`.
    pub account_root_public_key: String,
    /// Hex-encoded namespace ids to enroll into (32 bytes each). The caller must
    /// name them: this node is a member of nothing, so it can neither read the
    /// account's namespace set off a DAG nor derive it.
    pub namespaces: Vec<String>,
}

impl Validate for AccountPairInitApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if let Some(e) =
            validate_hex_string(&self.account_root_public_key, "accountRootPublicKey", 32)
        {
            errors.push(e);
        }
        // Refused here rather than deeper, where "enroll into nothing" is a
        // device that is certified and then listens on no topic at all.
        if self.namespaces.is_empty() {
            errors.push(ValidationError::EmptyField {
                field: "namespaces",
            });
        }
        errors.extend(
            self.namespaces
                .iter()
                .filter_map(|id| validate_hex_string(id, "namespaces[]", 32)),
        );

        errors
    }
}

/// Certify a device another node minted, link it, and deliver the scope keys —
/// scoped by application rather than by namespace.
///
/// Every field but `applications` is what that node's `pair-init` returned, and
/// the response is [`PairDeviceCompleteApiResponse`] unchanged.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountPairCompleteApiRequest {
    /// Hex-encoded `DeviceId` the other node minted (32 bytes).
    pub device_id: String,
    /// Hex-encoded X25519 agreement key to wrap the scope keys under (32 bytes).
    pub kem_public_key: String,
    /// Hex-encoded Ed25519 key that device signs its ops with (32 bytes).
    pub sign_public_key: String,
    /// Hex-encoded Ed25519 signature (64 bytes) from that node's pair-init.
    ///
    /// Not optional: without it the three values above are only claims by the
    /// sender, and certifying them would make attacker-supplied keys a trusted
    /// device of this account.
    pub statement: String,
    /// The confirmation code the account holder was read from the pairing
    /// device, e.g. `7BC0-DAAC-CCB4-84A4`. Grouping and case are ignored.
    ///
    /// One code covers the whole pairing, because one device was minted for it.
    pub confirmation_code: String,
    /// Which applications this device may speak for. Absent or empty means all.
    ///
    /// A person can answer "which apps may this device use"; they cannot answer
    /// "which namespaces", because a namespace is an implementation unit they
    /// never named. So the scope is chosen here and resolved to namespaces on the
    /// node, through the same lookup `GET /namespaces/for-application/:id` reads.
    #[serde(default)]
    pub applications: Vec<String>,
}

impl Validate for AccountPairCompleteApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        // The confirmation code is free-form here (grouping and case are
        // normalized at the point of comparison, which is the only place that can
        // say whether it is *right*); an empty one is refused up front.
        //
        // `applications` gets no check: the handler's parse is the only thing that
        // can say whether one names an application at all. An empty list is the
        // valid "all of them".
        //
        // It could now be length-checked like the fields below — an application id
        // is 64 hex like every other id — but a shape check still could not answer
        // the question that matters, so this stays where it is.
        if self.confirmation_code.trim().is_empty() {
            errors.push(ValidationError::EmptyField {
                field: "confirmationCode",
            });
        }
        for (field, value, expected) in [
            ("deviceId", &self.device_id, 32),
            ("kemPublicKey", &self.kem_public_key, 32),
            ("signPublicKey", &self.sign_public_key, 32),
            ("statement", &self.statement, 64),
        ] {
            if let Some(e) = validate_hex_string(value, field, expected) {
                errors.push(e);
            }
        }
        errors
    }
}

/// Withdraw a device from an account, terminally.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevokeDeviceApiRequest {
    /// Hex-encoded `DeviceId` to withdraw (32 bytes).
    pub device_id: String,
    /// A hex-encoded, borsh-serialized `SignedDeviceRevocation` minted elsewhere.
    ///
    /// Present when the account root that owns the device is not on this node —
    /// the lost-device case, where the proof is signed offline (`merod account
    /// revoke-proof`) and this node only publishes it. Omit it to keep the
    /// original behaviour: the node mints its own proof if it owns the account,
    /// and otherwise revokes as an admin.
    ///
    /// Not a credential, and not a secret: it authorises this one revocation of
    /// this one device, and only alongside a stored binding that already names the
    /// same account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<String>,
}

impl Validate for RevokeDeviceApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.device_id.len() != 64 {
            errors.push(ValidationError::InvalidLength {
                field: "deviceId",
                expected: 64,
                actual: self.device_id.len(),
            });
        } else if hex::decode(&self.device_id).is_err() {
            errors.push(ValidationError::InvalidHexEncoding {
                field: "deviceId",
                reason: "not valid hex".to_owned(),
            });
        }
        // Hex only. Whether the bytes decode as a proof, and whether that proof
        // verifies, are checked where the account is known — validation here has no
        // access to it, and a "valid" verdict that only meant "decodes" would be
        // more misleading than no check at all. An empty string is rejected because
        // it is always a caller mistake: omit the field instead.
        if let Some(proof) = &self.proof {
            if proof.is_empty() {
                errors.push(ValidationError::InvalidHexEncoding {
                    field: "proof",
                    reason: "empty; omit the field entirely if you have no proof".to_owned(),
                });
            } else if hex::decode(proof).is_err() {
                errors.push(ValidationError::InvalidHexEncoding {
                    field: "proof",
                    reason: "not valid hex".to_owned(),
                });
            }
        }
        errors
    }
}

/// What the revocation withdrew.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeDeviceApiResponseData {
    /// Hex-encoded `AccountId` the device spoke for.
    pub account_id: String,
    /// Hex-encoded `DeviceId` that was withdrawn.
    pub device_id: String,
    /// Whether the scope key rotated in the namespace **named in the request**.
    ///
    /// `revoked_in` is the full picture; this reports the one namespace the
    /// caller asked about, which is what it meant before a revocation reached
    /// more than one.
    ///
    /// Kept rather than folded into `revoked_in` because `calimero-client-py` is
    /// a Rust binding that deserializes into this struct, compiled into the
    /// released wheel while this field was required — dropping it makes every
    /// response fail to parse there, which merobox reports only as the useless
    /// "account revoke failed". The same mistake, with the same symptom, is
    /// the same mistake was recorded on `create_account`'s `accountNonce`, a
    /// field kept as 32 zeros for exactly this reason until that endpoint was
    /// deleted.
    ///
    /// Removable once a wheel built against `revoked_in` is released and the
    /// merobox `account_revoke` step reads it instead of `keyRotated`.
    pub key_rotated: bool,
    /// Every namespace the revocation was published into.
    ///
    /// A device belongs to an account, not to a scope, so revoking it withdraws
    /// it from every namespace holding a binding for it — not only the one named
    /// in the request. Reported per namespace because publication is per-DAG: a
    /// namespace absent here did not receive the op, and a partially propagated
    /// revocation is a state the caller has to be able to see.
    pub revoked_in: Vec<RevocationOutcomeApiEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevocationOutcomeApiEntry {
    /// Hex-encoded namespace id.
    pub namespace_id: String,
    /// Whether the scope key rotated in the same op, in THIS namespace.
    ///
    /// `false` means the device stopped writing there at once but still holds the
    /// key it had, so it can read until an admin rotates. Only an admin may
    /// rotate, and the account holder revoking their own device usually is not
    /// one, so this is commonly owed.
    pub key_rotated: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokeDeviceApiResponse {
    pub data: RevokeDeviceApiResponseData,
}

/// Repair or widen the reach of a device this account already certified, by
/// re-running pairing's fan-out against the namespaces this node takes part in
/// now. The device is named in the path and need not be online.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelinkDeviceApiRequest {
    /// Applications to add to the stored scope, hex-encoded. Empty repairs
    /// without widening; it is not overloaded to mean "every application" so the
    /// accidental request is not the widest one.
    #[serde(default)]
    pub applications: Vec<String>,
}

impl Validate for RelinkDeviceApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // `applications` gets no check here, exactly as on `pair-complete`: the
        // handler's parse is the only thing that can say whether one names an
        // application at all.
        Vec::new()
    }
}

/// What the relink repaired, and what it left alone.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelinkDeviceApiResponseData {
    /// Hex-encoded `AccountId` the device speaks for.
    pub account_id: String,
    /// Hex-encoded `DeviceId` that was repaired.
    pub device_id: String,
    /// The device's scope after the request, hex-encoded. Empty means every
    /// application, which is what a pairing that named none asked for.
    pub applications: Vec<String>,
    /// Namespaces the link was published into by this call.
    ///
    /// Reported per namespace for the same reason `revokedIn` is: publication is
    /// per-DAG, so which namespaces a device actually reached is a state the
    /// caller has to be able to see.
    pub linked_in: Vec<RelinkOutcomeApiEntry>,
    /// Namespaces nothing was published into, and why.
    pub skipped: Vec<RelinkSkipApiEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelinkOutcomeApiEntry {
    /// Hex-encoded namespace id.
    pub namespace_id: String,
    /// Whether the scope key was wrapped and published for the device here.
    ///
    /// `false` means the link landed and the delivery did not - the link is what
    /// confers authority, and the device's own sync pull re-requests the key.
    pub key_delivered: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelinkSkipApiEntry {
    /// Hex-encoded namespace id.
    pub namespace_id: String,
    /// Why nothing was published there. One of `outOfScope`, `alreadyBound`,
    /// `noScopeKey`, `revoked`, `ownDevice`, `failed`.
    pub reason: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelinkDeviceApiResponse {
    pub data: RelinkDeviceApiResponseData,
}

/// One device of this account, joined from the node-local certificate cache and
/// the live bindings of every namespace this node takes part in.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDeviceApiEntry {
    pub device_id: DeviceId,
    pub signing_key: PublicKey,
    /// Set only on the device this node itself presents.
    pub is_self: bool,
    pub revoked: bool,
    /// Applications this device may speak for. **Empty means every
    /// application** - the same convention `KnownDeviceCert` stores. Absent for
    /// a device this node has no cached certificate for (bound before the cache
    /// existed, or certified by another holder).
    pub applications: Vec<ApplicationId>,
    /// Hex-encoded ids of the namespaces currently holding a live binding for
    /// this device. Empty for a certified device not yet bound anywhere.
    pub namespaces: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDevicesApiResponse {
    pub devices: Vec<AccountDeviceApiEntry>,
}

/// One application this account speaks in, derived from the namespaces this
/// node takes part in that target it.
///
/// **Known limitation:** an application installed with no namespace yet is
/// invisible here - it has no cross-device meaning until a namespace exists.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountApplicationApiEntry {
    pub application_id: ApplicationId,
    /// Hex-encoded ids of the namespaces targeting this application.
    pub namespaces: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountApplicationsApiResponse {
    pub applications: Vec<AccountApplicationApiEntry>,
}

/// A join the joiner signed but cannot publish, handed to a node the inviter
/// named as an admitter.
///
/// The joiner may hold no node at all — an account, a key, an offline device
/// certificate, and nowhere to publish from. This is how such a joiner gets its
/// membership op onto the DAG.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmitJoinApiRequest {
    /// The invitation being claimed. Must name this node in its `admitters`,
    /// and must otherwise verify — being designated is permission to carry a
    /// valid claim, not to skip checking it.
    pub invitation: calimero_context_config::types::SignedGroupOpenInvitation,
    /// The joiner's `SignedNamespaceOp`, borsh-encoded and hex.
    ///
    /// Already signed, and signed by the joiner's **device key** — every peer
    /// checks `signer == credential.statement.sign_pk` when applying a join. An
    /// admitter therefore cannot author this, only carry it, and cannot alter
    /// who joined, which group, or with what role.
    pub signed_op: String,
}

impl Validate for AdmitJoinApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.signed_op.trim().is_empty() {
            errors.push(ValidationError::EmptyField { field: "signedOp" });
        }
        errors
    }
}

/// What the admitter did with it.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmitJoinApiResponseData {
    /// Whether the op reached the namespace topic.
    ///
    /// Not "joined": membership lands when peers apply the op, which this node
    /// neither performs nor waits for. Reporting a join here would report
    /// something this endpoint cannot observe.
    pub published: bool,
}

/// The response as it goes over the wire, envelope included.
///
/// `ApiResponse` serialises the payload under `data`, so a client that
/// deserialises into the payload struct alone looks for `published` at the top
/// level and fails. Matches `JoinNamespaceApiResponse`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmitJoinApiResponse {
    pub data: AdmitJoinApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateGroupInvitationApiRequest {
    /// Duration in seconds for the invitation validity.
    /// Defaults to 1 year when not provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration_timestamp: Option<u64>,
    #[serde(default)]
    pub recursive: Option<bool>,
    /// Accounts permitted to admit a claim of this invitation, 64 hex each.
    ///
    /// Empty or absent is filled in at mint from the group's admins and TEE
    /// nodes. Naming them explicitly narrows that set; it cannot widen it past
    /// what the caller is entitled to name, because the list is signed and
    /// checked against the account an admitter proves it holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admitters: Vec<String>,
    /// libp2p addresses for those admitters, each a full multiaddr including
    /// the `/p2p/<peer-id>` suffix.
    ///
    /// Empty or absent asks the node to fill them in from addresses it already
    /// has on file. Supplied values are used as given rather than merged.
    ///
    /// Unsigned: a wrong address misdirects where a joiner knocks, never who may
    /// answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub admitter_addrs: Vec<String>,
}

impl Validate for CreateGroupInvitationApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInvitationApiResponse {
    pub data: CreateGroupInvitationApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInvitationApiResponseData {
    pub invitation: SignedGroupOpenInvitation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecursiveInvitationEntry {
    pub group_id: String,
    pub invitation: SignedGroupOpenInvitation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRecursiveInvitationApiResponseData {
    pub invitations: Vec<RecursiveInvitationEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRecursiveInvitationApiResponse {
    pub data: CreateRecursiveInvitationApiResponseData,
}

/// Atomically move a group to a new parent. Replaces the old
/// nest/unnest pair — orphan state is no longer reachable.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReparentGroupApiRequest {
    pub new_parent_id: String,
}

impl Validate for ReparentGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.new_parent_id.len() != 64 {
            errors.push(ValidationError::InvalidLength {
                field: "new_parent_id",
                expected: 64,
                actual: self.new_parent_id.len(),
            });
        } else if hex::decode(&self.new_parent_id).is_err() {
            errors.push(ValidationError::InvalidHexEncoding {
                field: "new_parent_id",
                reason: "not valid hex".to_owned(),
            });
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReparentGroupApiResponse {
    pub reparented: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubgroupEntryApiResponse {
    pub group_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSubgroupsApiResponse {
    pub subgroups: Vec<SubgroupEntryApiResponse>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceGroupEntryApiResponse {
    pub group_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNamespaceGroupsApiResponse {
    pub data: Vec<NamespaceGroupEntryApiResponse>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JoinGroupApiRequest {
    pub invitation: SignedGroupOpenInvitation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
}

impl Validate for JoinGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinGroupApiResponse {
    pub data: JoinGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinGroupApiResponseData {
    pub group_id: String,
    /// The key the joiner signs with, 64 hex.
    pub member_identity: PublicKey,
    /// The account that key joined as, 64 hex characters — the id every
    /// member-addressing endpoint expects. See `NodeIdentityApiResponseData`.
    pub member_account: String,
}

/// Response for `POST namespaces/:id/join`. Names the id `namespaceId`,
/// matching `CreateNamespaceApiResponseData` — a namespace is a root group
/// internally, and only the namespace endpoints translate that on the way out.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinNamespaceApiResponseData {
    pub namespace_id: String,
    /// The key the joiner signs with, 64 hex.
    pub member_identity: PublicKey,
    /// The account that key joined as, 64 hex characters — the id every
    /// member-addressing endpoint expects. See `NodeIdentityApiResponseData`.
    pub member_account: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinNamespaceApiResponse {
    pub data: JoinNamespaceApiResponseData,
}

// ---- List All Groups ----

// ---- Update Member Role ----

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct UpdateMemberRoleApiResponse {}

// ---- Add Group Members (empty response) ----

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct AddGroupMembersApiResponse {}

// ---- Remove Group Members (empty response) ----

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct RemoveGroupMembersApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateMemberRoleApiRequest {
    pub role: GroupMemberRole,
}

impl Validate for UpdateMemberRoleApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.role == GroupMemberRole::ReadOnlyTee {
            errors.push(ValidationError::InvalidFormat {
                field: "role",
                reason: "ReadOnlyTee role can only be assigned via TEE attestation".to_owned(),
            });
        }
        errors
    }
}

// ---- Detach Context From Group ----

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct DetachContextFromGroupApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DetachContextFromGroupApiRequest {}

impl Validate for DetachContextFromGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

// ---- Sync Group ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncGroupApiRequest {}

impl Validate for SyncGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncGroupApiResponse {
    pub data: SyncGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncGroupApiResponseData {
    pub group_id: String,
    // `appKey` on the wire: the rename is internal, the JSON is a client contract.
    #[serde(rename = "appKey")]
    pub bytecode_id: String,
    pub target_application_id: ApplicationId,
    pub member_count: u64,
    pub context_count: u64,
}

// ---- Join Context ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinContextApiResponse {
    pub data: JoinContextApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinContextApiResponseData {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

// ---- Join Subgroup via Inheritance ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinSubgroupInheritanceApiResponse {
    pub data: JoinSubgroupInheritanceApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinSubgroupInheritanceApiResponseData {
    pub group_id: String,
    pub member_public_key: PublicKey,
    pub was_inherited: bool,
}

// ---- Leave Context (local-only opt-out) ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveContextApiResponse {
    pub data: LeaveContextApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveContextApiResponseData {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

// ---- Leave Group (distributed self-leave op) ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveGroupApiResponse {
    pub data: LeaveGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveGroupApiResponseData {
    pub group_id: String,
    pub member_public_key: PublicKey,
}

// ---- Issue Ownership Proof ----
//
// Wire format is locked: see github.com/calimero-network/tauri-app#73.
// mdma and tauri-app are implemented separately against this exact shape.
//
// Request: { audience, context_id, subject, nonce, expires_at_ms }
// Response: { signer_public_key, signed_payload, signature }
//
// `signed_payload` is opaque base64-encoded UTF-8 JSON bytes — the verifier
// re-parses them. The signature input is
//   `OWNERSHIP_PROOF_DOMAIN || signed_payload_bytes`
// where `OWNERSHIP_PROOF_DOMAIN` is the 28-byte literal
// `b"calimero.ownership-claim.v1\x00"` (defined in calimero-context).

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssueOwnershipProofApiRequest {
    pub audience: String,
    /// Hex-encoded 32-byte context id. Parsed server-side via `parse_context_id`,
    /// which took base58 too until every id became hex.
    pub context_id: String,
    pub subject: String,
    /// Hex string, 32–128 chars inclusive (16–64 raw bytes).
    pub nonce: String,
    /// Caller-requested expiry in unix milliseconds. Server clamps to
    /// `min(expires_at_ms, issued_at_ms + 5*60*1000)`.
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueOwnershipProofApiResponse {
    /// Hex-encoded 32-byte ed25519 public key of the signer.
    pub signer_public_key: String,
    /// Base64-encoded opaque UTF-8 JSON bytes of the canonical claim payload.
    /// Verifiers MUST re-parse this exact byte slice and re-derive the
    /// signature input as `OWNERSHIP_PROOF_DOMAIN || signed_payload_bytes`.
    pub signed_payload: String,
    /// Base64-encoded 64-byte ed25519 signature over the signature input.
    pub signature: String,
}

impl Validate for IssueOwnershipProofApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // audience: non-empty, <= 256 chars.
        if self.audience.is_empty() {
            errors.push(ValidationError::EmptyField { field: "audience" });
        } else if let Some(e) = validate_string_length(&self.audience, "audience", 256) {
            errors.push(e);
        }

        // subject: non-empty, <= 512 chars.
        if self.subject.is_empty() {
            errors.push(ValidationError::EmptyField { field: "subject" });
        } else if let Some(e) = validate_string_length(&self.subject, "subject", 512) {
            errors.push(e);
        }

        // nonce: hex string, 32..=128 chars inclusive (16..=64 raw bytes).
        let n = self.nonce.len();
        if !(32..=128).contains(&n) {
            errors.push(ValidationError::InvalidFormat {
                field: "nonce",
                reason: "nonce must be hex-encoded, 32..=128 characters".into(),
            });
        } else if !self.nonce.chars().all(|c| c.is_ascii_hexdigit()) {
            errors.push(ValidationError::InvalidHexEncoding {
                field: "nonce",
                reason: "nonce must be valid hex".into(),
            });
        } else if !n.is_multiple_of(2) {
            // An odd-length hex string can't decode to whole bytes, which is
            // inconsistent with the documented "16..=64 raw bytes" contract.
            errors.push(ValidationError::InvalidFormat {
                field: "nonce",
                reason: "nonce hex string must have even length".into(),
            });
        }

        // context_id and expires_at_ms are validated in the handler (the former
        // because parsing is shared with `parse_context_id`, the latter because
        // it requires comparing against the current wall-clock).

        errors
    }
}

/// Namespace-scoped sibling of [`IssueOwnershipProofApiRequest`].
///
/// Wire-identical MINUS the `contextId` field: the proof is scoped to the
/// namespace-root group only. The response reuses
/// [`IssueOwnershipProofApiResponse`] verbatim. Purely additive to
/// `calimero-server-primitives`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssueNamespaceOwnershipProofApiRequest {
    pub audience: String,
    pub subject: String,
    /// Hex string, 32–128 chars inclusive (16–64 raw bytes).
    pub nonce: String,
    /// Caller-requested expiry in unix milliseconds. Server clamps to
    /// `min(expires_at_ms, issued_at_ms + 5*60*1000)`.
    pub expires_at_ms: u64,
}

impl Validate for IssueNamespaceOwnershipProofApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // audience: non-empty, <= 256 chars.
        if self.audience.is_empty() {
            errors.push(ValidationError::EmptyField { field: "audience" });
        } else if let Some(e) = validate_string_length(&self.audience, "audience", 256) {
            errors.push(e);
        }

        // subject: non-empty, <= 512 chars.
        if self.subject.is_empty() {
            errors.push(ValidationError::EmptyField { field: "subject" });
        } else if let Some(e) = validate_string_length(&self.subject, "subject", 512) {
            errors.push(e);
        }

        // nonce: hex string, 32..=128 chars inclusive (16..=64 raw bytes).
        let n = self.nonce.len();
        if !(32..=128).contains(&n) {
            errors.push(ValidationError::InvalidFormat {
                field: "nonce",
                reason: "nonce must be hex-encoded, 32..=128 characters".into(),
            });
        } else if !self.nonce.chars().all(|c| c.is_ascii_hexdigit()) {
            errors.push(ValidationError::InvalidHexEncoding {
                field: "nonce",
                reason: "nonce must be valid hex".into(),
            });
        } else if !n.is_multiple_of(2) {
            // An odd-length hex string can't decode to whole bytes, which is
            // inconsistent with the documented "16..=64 raw bytes" contract.
            errors.push(ValidationError::InvalidFormat {
                field: "nonce",
                reason: "nonce hex string must have even length".into(),
            });
        }

        // expires_at_ms is validated in the handler (it requires comparing
        // against the current wall-clock).

        errors
    }
}

// ---- Leave Namespace (cascading self-leave) ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveNamespaceApiResponse {
    pub data: LeaveNamespaceApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveNamespaceApiResponseData {
    pub namespace_id: String,
    pub member_public_key: PublicKey,
}

// ---- Get Context Group ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextGroupApiResponse {
    pub data: Option<String>,
}

// ---- Group Permissions API ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetMemberCapabilitiesApiRequest {
    pub capabilities: u32,
}

impl Validate for SetMemberCapabilitiesApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetMemberCapabilitiesApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetMemberAutoFollowApiRequest {
    /// When true, the target auto-joins new contexts registered in this group.
    pub auto_follow_contexts: bool,
    /// When true, the target self-admits into subgroups nested under this group.
    pub auto_follow_subgroups: bool,
}

impl Validate for SetMemberAutoFollowApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetMemberAutoFollowApiResponse {}

// ---- Set Metadata (group / member / context) ----

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetMetadataApiRequest {
    /// New display name. Absent field keeps the current name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Replacement opaque `data` map; stored verbatim by core.
    #[serde(default)]
    pub data: BTreeMap<String, String>,
}

impl Validate for SetMetadataApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // Delegate to the single source of truth — the exact same checks the
        // `*MetadataSet` op-apply path enforces (size limits, non-empty name,
        // non-empty data keys) — so an HTTP request that would later fail at
        // apply time is rejected here with a clean 400 instead.
        match calimero_primitives::metadata::validate_metadata_payload(
            self.name.as_deref(),
            &self.data,
        ) {
            Ok(()) => Vec::new(),
            Err(reason) => vec![ValidationError::InvalidFormat {
                field: "metadata",
                reason,
            }],
        }
    }
}

pub type SetMemberMetadataApiRequest = SetMetadataApiRequest;
pub type SetGroupMetadataApiRequest = SetMetadataApiRequest;
pub type SetContextMetadataApiRequest = SetMetadataApiRequest;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetMetadataApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMetadataApiResponse {
    /// The metadata record, or `null` if none has ever been set for the
    /// target (group / member / context).
    pub data: Option<MetadataRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMemberCapabilitiesApiResponse {
    pub data: GetMemberCapabilitiesApiData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMemberCapabilitiesApiData {
    pub capabilities: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetDefaultCapabilitiesApiRequest {
    pub default_capabilities: u32,
}

impl Validate for SetDefaultCapabilitiesApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetDefaultCapabilitiesApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetTeeAdmissionPolicyApiRequest {
    #[serde(default)]
    pub allowed_mrtd: Vec<String>,
    #[serde(default)]
    pub allowed_rtmr0: Vec<String>,
    #[serde(default)]
    pub allowed_rtmr1: Vec<String>,
    #[serde(default)]
    pub allowed_rtmr2: Vec<String>,
    #[serde(default)]
    pub allowed_rtmr3: Vec<String>,
    #[serde(default)]
    pub allowed_tcb_statuses: Vec<String>,
    #[serde(default)]
    pub accept_mock: bool,
}

impl Validate for SetTeeAdmissionPolicyApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.allowed_mrtd.is_empty() && !self.accept_mock {
            errors.push(ValidationError::InvalidFormat {
                field: "allowed_mrtd",
                reason: "at least one MRTD must be specified when accept_mock is false".to_owned(),
            });
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetTeeAdmissionPolicyApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTeeAdmissionPolicyApiResponse {
    pub enabled: bool,
    pub allowed_mrtd: Vec<String>,
    pub allowed_rtmr0: Vec<String>,
    pub allowed_rtmr1: Vec<String>,
    pub allowed_rtmr2: Vec<String>,
    pub allowed_rtmr3: Vec<String>,
    pub allowed_tcb_statuses: Vec<String>,
    pub accept_mock: bool,
}

impl GetTeeAdmissionPolicyApiResponse {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            allowed_mrtd: vec![],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec![],
            accept_mock: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetSubgroupVisibilityApiRequest {
    pub subgroup_visibility: String,
}

impl Validate for SetSubgroupVisibilityApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.subgroup_visibility != "open" && self.subgroup_visibility != "restricted" {
            errors.push(ValidationError::InvalidFormat {
                field: "subgroup_visibility",
                reason: "must be 'open' or 'restricted'".into(),
            });
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetSubgroupVisibilityApiResponse {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_device_id_alias_request_round_trips_through_json() {
        let device_id = DeviceId::from([0x11; 32]);
        let json = serde_json::json!({
            "alias": "laptop",
            "deviceId": device_id.to_string(),
        });

        let req: CreateAliasRequest<DeviceId> =
            serde_json::from_value(json).expect("valid device id must deserialize");
        assert_eq!(req.alias.as_str(), "laptop");
        assert_eq!(DeviceId::from_value(req.value), device_id);
    }

    #[test]
    fn create_device_id_alias_request_rejects_invalid_device_id() {
        // Wrong width: valid hex, but not 32 bytes.
        let short = serde_json::json!({"alias": "laptop", "deviceId": "aa"});
        assert!(serde_json::from_value::<CreateAliasRequest<DeviceId>>(short).is_err());

        // Right width, non-hex characters.
        let non_hex = serde_json::json!({"alias": "laptop", "deviceId": "g".repeat(64)});
        assert!(serde_json::from_value::<CreateAliasRequest<DeviceId>>(non_hex).is_err());
    }

    #[test]
    fn join_response_ignores_a_governance_op_from_an_older_node() {
        // The mirror of the tolerance this replaces. `governanceOp` is gone from
        // the struct, so the risk is no longer a client that misses the field but
        // a client that meets it: a node predating the removal still sends it, and
        // this type must read such a response rather than reject it as unknown.
        let json = serde_json::json!({
            "groupId": hex::encode([0xCC; 32]),
            "memberIdentity": PublicKey::from([0xBB; 32]),
            "memberAccount": hex::encode([0xDD; 32]),
            "governanceOp": "",
        });

        let resp: JoinGroupApiResponseData = serde_json::from_value(json).unwrap();
        assert_eq!(resp.member_account, hex::encode([0xDD; 32]));
    }

    #[test]
    fn create_context_response_serializes_with_group_info() {
        let context_id = ContextId::from([0xAA; 32]);
        let member_pk = PublicKey::from([0xBB; 32]);
        let group_id_hex = hex::encode([0xCC; 32]);

        let resp = CreateContextResponseData {
            context_id,
            member_public_key: member_pk,
            group_id: Some(group_id_hex.clone()),
            group_created: true,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["groupId"], group_id_hex);
        assert_eq!(json["groupCreated"], true);
        assert!(json["contextId"].is_string());
        assert!(json["memberPublicKey"].is_string());
    }

    #[test]
    fn create_context_response_omits_group_id_when_none() {
        let context_id = ContextId::from([0xAA; 32]);
        let member_pk = PublicKey::from([0xBB; 32]);

        let resp = CreateContextResponseData {
            context_id,
            member_public_key: member_pk,
            group_id: None,
            group_created: false,
        };

        let json = serde_json::to_value(&resp).unwrap();
        // groupId should be omitted (skip_serializing_if = "Option::is_none")
        assert!(json.get("groupId").is_none());
        assert_eq!(json["groupCreated"], false);
    }

    #[test]
    fn create_context_response_deserializes_without_group_fields() {
        // Backwards compatibility: old responses without groupId/groupCreated
        // Use valid hex IDs (every id serializes as 64 hex)
        let context_id = ContextId::from([0xAA; 32]);
        let member_pk = PublicKey::from([0xBB; 32]);
        let json = serde_json::json!({
            "contextId": serde_json::to_value(context_id).unwrap(),
            "memberPublicKey": serde_json::to_value(member_pk).unwrap()
        });

        let resp: CreateContextResponseData = serde_json::from_value(json).unwrap();
        assert!(resp.group_id.is_none());
        assert!(!resp.group_created);
    }

    /// A node predating `account` omits it and must still parse. Absent stays
    /// absent: a zero account would name a principal that exists nowhere.
    #[test]
    fn a_member_entry_without_an_account_still_deserializes() {
        let json = serde_json::json!({
            "targetVersion": 2,
            "expectedMembers": 1,
            "rollup": {
                "migrated": 1, "inProgress": 0, "unknown": 0, "failed": 0,
                "total": 1, "allMigrated": true, "membersPendingSignature": 0
            },
            "members": [{ "peer": PublicKey::from([0x11; 32]), "state": "migrated" }]
        });

        let resp: GetMigrationStatusApiResponse =
            serde_json::from_value(json).expect("an older node's response must still parse");

        assert_eq!(
            resp.members[0].account, None,
            "absent must stay absent - a zero account would name nobody"
        );
    }

    #[test]
    fn migration_status_response_serializes_rollup_and_members() {
        // The `get_migration_status` admin route (Task 6c.10) returns this
        // shape. Pin the JSON contract: camelCase keys, the per-member `state`
        // discriminant, the `allMigrated` rollup flag, and a `null`-report
        // member surfacing as `unknown` with its `report` field omitted.
        let migrated_peer = PublicKey::from([0x11; 32]);
        let unknown_peer = PublicKey::from([0x22; 32]);
        let failed_peer = PublicKey::from([0x33; 32]);

        let resp = GetMigrationStatusApiResponse {
            target_version: 2,
            expected_members: 3,
            cohort_pinned_at_hlc: Some("hlc-abc".into()),
            fleet_completed_at: None,
            rollup: MigrationStatusRollupApiData {
                migrated: 1,
                in_progress: 0,
                unknown: 1,
                failed: 1,
                total: 3,
                all_migrated: false,
                members_pending_signature: 1,
            },
            members: vec![
                MemberMigrationStatusApiEntry {
                    peer: migrated_peer,
                    account: Some(AccountId::from(*migrated_peer)),
                    report: Some(MemberMigrationReportApiData {
                        schema_version: 2,
                        residue_auto: 0,
                        residue_identity: 0,
                        synced_up_to_hlc: 7,
                        reported_at: 1_700_000_000,
                        authored_remaining: 3,
                        migration_failed: None,
                    }),
                    state: "migrated".into(),
                },
                MemberMigrationStatusApiEntry {
                    peer: unknown_peer,
                    account: Some(AccountId::from(*unknown_peer)),
                    report: None,
                    state: "unknown".into(),
                },
                MemberMigrationStatusApiEntry {
                    peer: failed_peer,
                    account: Some(AccountId::from(*failed_peer)),
                    report: Some(MemberMigrationReportApiData {
                        schema_version: 1,
                        residue_auto: 1,
                        residue_identity: 0,
                        synced_up_to_hlc: 5,
                        reported_at: 1_700_000_001,
                        authored_remaining: 0,
                        migration_failed: Some("check_aborted".into()),
                    }),
                    state: "failed".into(),
                },
            ],
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["targetVersion"], 2);
        assert_eq!(json["expectedMembers"], 3);
        assert_eq!(json["cohortPinnedAtHlc"], "hlc-abc");
        // A cohort that has not converged has no fleet timestamp to carry.
        assert!(json.get("fleetCompletedAt").is_none());
        assert_eq!(json["rollup"]["allMigrated"], false);
        assert_eq!(json["rollup"]["migrated"], 1);
        assert_eq!(json["rollup"]["unknown"], 1);
        assert_eq!(json["rollup"]["failed"], 1);
        assert_eq!(json["rollup"]["membersPendingSignature"], 1);

        let members = json["members"].as_array().unwrap();
        assert_eq!(members.len(), 3);
        assert_eq!(members[0]["state"], "migrated");
        assert_eq!(members[0]["report"]["schemaVersion"], 2);
        assert_eq!(members[0]["report"]["syncedUpToHlc"], 7);
        assert_eq!(members[0]["report"]["authoredRemaining"], 3);
        // Released calimero-client-py (which the e2e suite runs) still requires
        // this key; omitting it fails its deserialize before any assert runs.
        assert_eq!(members[0]["report"]["residueIdentity"], 0);
        // A migrated member carries no failure reason — `migrationFailed` omitted.
        assert!(members[0]["report"].get("migrationFailed").is_none());
        // The unknown member has no fresh report — `report` is omitted.
        assert_eq!(members[1]["state"], "unknown");
        assert!(members[1].get("report").is_none());
        // The failed member surfaces its categorized reason.
        assert_eq!(members[2]["state"], "failed");
        assert_eq!(members[2]["report"]["migrationFailed"], "check_aborted");
    }

    #[test]
    fn migration_status_response_omits_hlc_when_absent() {
        // No migration record → `cohortPinnedAtHlc` is omitted.
        let resp = GetMigrationStatusApiResponse {
            target_version: 0,
            expected_members: 0,
            cohort_pinned_at_hlc: None,
            fleet_completed_at: Some(1_700_002_000),
            rollup: MigrationStatusRollupApiData {
                migrated: 0,
                in_progress: 0,
                unknown: 0,
                failed: 0,
                total: 0,
                all_migrated: false,
                members_pending_signature: 0,
            },
            members: vec![],
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("cohortPinnedAtHlc").is_none());
        assert_eq!(json["rollup"]["allMigrated"], false);
        // The durable answer outlives the live rollup: a member that converged
        // and then went quiet turns `allMigrated` back off, and this does not
        // follow it.
        assert_eq!(json["fleetCompletedAt"], 1_700_002_000u64);
    }

    fn ownership_req(nonce: &str) -> IssueOwnershipProofApiRequest {
        IssueOwnershipProofApiRequest {
            audience: "mdma.cloud".into(),
            context_id: "0000000000000000000000000000000000000000000000000000000000000000".into(),
            subject: "subject-xyz".into(),
            nonce: nonce.into(),
            expires_at_ms: 1,
        }
    }

    #[test]
    fn ownership_proof_even_length_hex_nonce_passes() {
        // 32 hex chars (16 bytes) — minimum valid, even length.
        let errors = ownership_req("deadbeefcafebabe1122334455667788").validate();
        assert!(
            errors.is_empty(),
            "even-length hex nonce must validate cleanly, got {errors:?}"
        );
    }

    #[test]
    fn ownership_proof_odd_length_hex_nonce_rejected() {
        // 33 hex chars: in range, all hex digits, but odd length.
        let errors = ownership_req("deadbeefcafebabe1122334455667788a").validate();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::InvalidFormat { field: "nonce", reason }
                    if reason == "nonce hex string must have even length"
            )),
            "odd-length hex nonce must be rejected, got {errors:?}"
        );
    }

    fn pair_init_req(namespaces: Vec<String>) -> AccountPairInitApiRequest {
        AccountPairInitApiRequest {
            account_root_public_key: hex::encode([0x11; 32]),
            namespaces,
        }
    }

    fn pair_complete_req() -> AccountPairCompleteApiRequest {
        AccountPairCompleteApiRequest {
            device_id: hex::encode([0x44; 32]),
            kem_public_key: hex::encode([0x55; 32]),
            sign_public_key: hex::encode([0x66; 32]),
            statement: hex::encode([0x77; 64]),
            confirmation_code: "7BC0-DAAC-CCB4-84A4".to_owned(),
            applications: Vec::new(),
        }
    }

    #[test]
    fn account_pair_init_accepts_a_set_of_namespaces() {
        let errors =
            pair_init_req(vec![hex::encode([0x22; 32]), hex::encode([0x33; 32])]).validate();
        assert!(
            errors.is_empty(),
            "a well-formed set must validate, got {errors:?}"
        );
    }

    /// Naming none is the one case nothing downstream can recover from: the device
    /// is certified and then listens on no topic at all.
    #[test]
    fn account_pair_init_refuses_an_empty_namespace_set() {
        let errors = pair_init_req(Vec::new()).validate();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::EmptyField {
                    field: "namespaces"
                }
            )),
            "an empty namespace set must be refused, got {errors:?}"
        );
    }

    /// Every entry is checked, not just the first. The handler decodes all of
    /// them, so a set that validates on its head and fails on its tail would be
    /// refused half way through minting.
    #[test]
    fn account_pair_init_checks_every_namespace_in_the_set() {
        let errors = pair_init_req(vec![
            hex::encode([0x22; 32]),
            hex::encode([0x33; 16]),
            "zz".repeat(32),
        ])
        .validate();

        assert_eq!(
            errors.len(),
            2,
            "both malformed entries must be reported, got {errors:?}"
        );
        assert!(errors.iter().all(|e| matches!(
            e,
            ValidationError::InvalidLength {
                field: "namespaces[]",
                ..
            } | ValidationError::InvalidHexEncoding {
                field: "namespaces[]",
                ..
            }
        )));
    }

    #[test]
    fn account_pair_init_refuses_a_root_key_of_the_wrong_width() {
        let errors = AccountPairInitApiRequest {
            account_root_public_key: hex::encode([0x11; 31]),
            namespaces: vec![hex::encode([0x22; 32])],
        }
        .validate();

        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::InvalidLength {
                    field: "accountRootPublicKey",
                    expected: 64,
                    ..
                }
            )),
            "a 31-byte root key must be refused, got {errors:?}"
        );
    }

    #[test]
    fn account_pair_complete_accepts_what_pair_init_returned() {
        let errors = pair_complete_req().validate();
        assert!(
            errors.is_empty(),
            "the minted payload must validate, got {errors:?}"
        );
    }

    /// The statement is 64 bytes and the three keys 32, and the width is the only
    /// thing that tells them apart - so a value put in the wrong field has to be
    /// refused here rather than decoded into something the certificate names.
    #[test]
    fn account_pair_complete_pins_each_field_to_its_own_width() {
        // Every field gets the other's width at once, which also pins that the
        // errors accumulate rather than stop at the first.
        let key = hex::encode([0x88; 32]);
        let statement = hex::encode([0x88; 64]);
        let mut req = pair_complete_req();
        req.device_id = statement.clone();
        req.kem_public_key = statement.clone();
        req.sign_public_key = statement;
        req.statement = key;

        let errors = req.validate();
        for (field, expected) in [
            ("deviceId", 64),
            ("kemPublicKey", 64),
            ("signPublicKey", 64),
            ("statement", 128),
        ] {
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    ValidationError::InvalidLength { field: f, expected: x, .. }
                        if *f == field && *x == expected
                )),
                "{field} at the wrong width must be refused, got {errors:?}"
            );
        }
    }

    #[test]
    fn account_pair_complete_refuses_a_blank_confirmation_code() {
        let mut req = pair_complete_req();
        req.confirmation_code = "   ".to_owned();

        let errors = req.validate();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::EmptyField {
                    field: "confirmationCode"
                }
            )),
            "a code of nothing but whitespace must be refused, got {errors:?}"
        );
    }

    /// Absent means all, so the field has to decode to an empty list rather than
    /// fail. The node reads that empty list as every namespace it takes part in -
    /// see `resolve_scope` beside the pairing handler.
    #[test]
    fn account_pair_complete_omitting_applications_means_all_of_them() {
        let json = serde_json::json!({
            "deviceId": hex::encode([0x44; 32]),
            "kemPublicKey": hex::encode([0x55; 32]),
            "signPublicKey": hex::encode([0x66; 32]),
            "statement": hex::encode([0x77; 64]),
            "confirmationCode": "7BC0-DAAC-CCB4-84A4",
        });

        let req: AccountPairCompleteApiRequest =
            serde_json::from_value(json).expect("a request naming no application must parse");

        assert!(req.applications.is_empty());
        assert!(req.validate().is_empty());
    }

    /// A named application narrows the fan-out, and its id is hex rather than
    /// hex - so validation has to let it through for the handler's parse to be the
    /// thing that judges it.
    #[test]
    fn account_pair_complete_carries_a_named_application_through_validation() {
        let application = ApplicationId::from([0x99; 32]).to_string();
        let mut req = pair_complete_req();
        req.applications = vec![application.clone()];

        let errors = req.validate();
        assert!(
            errors.is_empty(),
            "an application id is not this layer's to judge, got {errors:?}"
        );
        let json = serde_json::to_value(&req).expect("serialize");
        assert_eq!(json["applications"][0], application);
    }
}

// ---------------------------------------------------------------------------
// Namespace API types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceApiResponse {
    pub namespace_id: String,
    // `appKey` on the wire: the rename is internal, the JSON is a client contract.
    #[serde(rename = "appKey")]
    pub bytecode_id: String,
    pub target_application_id: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub member_count: usize,
    pub context_count: usize,
    pub subgroup_count: usize,
    /// Bundle-manifest version of this namespace's `bytecode_id` blob — the
    /// per-namespace truth (the shared application row only says "latest
    /// fetched"). `None` when unresolvable (raw-wasm app, legacy key,
    /// blob not retained locally).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNamespaceApiResponse {
    pub data: NamespaceApiResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNamespacesApiResponse {
    pub data: Vec<NamespaceApiResponse>,
}

/// Who this node is, with no namespace involved.
///
/// Each field is node-level: one root key is one account everywhere, a node is
/// one device, and it signs with one key. The namespaced endpoints this replaces
/// took a namespace and returned the same answer regardless, which read as
/// though the answer varied by scope.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeIdentityApiResponseData {
    /// Hex-encoded `AccountId` this node writes as.
    pub account_id: String,
    /// Hex-encoded `DeviceId`, or `None` when the node has not enrolled yet.
    pub device_id: Option<String>,
    /// The key this node signs ops with, 64 hex.
    ///
    /// The device's signing key, not the account root — the root signs
    /// certificates and handoffs and never an op, so a signature on the wire
    /// verifies against this one.
    pub public_key: String,
    /// Hex-encoded epoch-0 root **public** key of this node's account.
    ///
    /// This is what a second device needs to pair into this account, and it is
    /// public by construction: it is hashed into the `AccountId` and travels in
    /// every genesis. Not optional, because the route 404s without an account
    /// root — there is no account to report rather than an empty one.
    ///
    /// The private root is not reachable from any HTTP route. It leaves the
    /// node only via `merod account export`, as a mnemonic.
    pub account_root_public_key: String,
    /// Hex-encoded X25519 **public** agreement key of this node's device, or
    /// `None` when it has no device row yet.
    ///
    /// The third input `merod account sign-cert` needs, alongside `device_id` and
    /// `public_key`. Without it an operator holding an offline account root cannot
    /// certify this node's device at all: the two ids were reachable here and this
    /// key was not, so the certificate could be described but not signed.
    ///
    /// Public by construction — it is what wrapped scope keys are addressed to, so
    /// it travels in every device binding this node publishes. The matching secret
    /// is the one thing that opens those deliveries and is reachable from no HTTP
    /// route.
    pub device_agreement_key: Option<String>,

    /// Whether this node holds the root key of the account it speaks for. False
    /// means no root key available: the node runs on a delegate device key, so it
    /// cannot certify another device into the account.
    ///
    /// Defaulted, so a response from a node predating the field still deserializes.
    #[serde(default)]
    pub holds_account_root: bool,

    /// Whether this node's device is certified into the account it speaks for.
    /// Pair-init mints the device and only pair-complete certifies it, and
    /// `holds_account_root` is false across both - this separates them.
    ///
    /// Defaulted, so a response from a node predating the field still deserializes.
    #[serde(default)]
    pub device_certified: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeIdentityApiResponse {
    pub data: NodeIdentityApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNamespacesQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNamespacesForApplicationQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[cfg(test)]
mod naming_back_compat_tests {
    use super::{
        ApplicationId, CreateGroupApiRequest, CreateNamespaceApiRequest, GroupInfoApiResponseData,
    };

    // A client on the old field name must keep working. The alias is the
    // contract; without it every existing caller breaks on a pure rename.
    #[test]
    fn create_group_request_still_accepts_the_legacy_camel_case_alias() {
        let json = r#"{"appKey":"aabb","applicationId":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
        let req: CreateGroupApiRequest =
            serde_json::from_str(json).expect("legacy appKey must still deserialize");
        assert_eq!(req.bytecode_id.as_deref(), Some("aabb"));
    }

    #[test]
    fn create_group_request_accepts_the_new_field() {
        let json = r#"{"bytecodeId":"aabb","applicationId":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
        let req: CreateGroupApiRequest =
            serde_json::from_str(json).expect("bytecodeId must deserialize");
        assert_eq!(req.bytecode_id.as_deref(), Some("aabb"));
    }

    // Responses keep the OLD wire name: the rename is internal, and every
    // deployed client reads `appKey`. Requests take either name via the alias.
    #[test]
    fn group_info_response_keeps_the_legacy_wire_name() {
        let data = GroupInfoApiResponseData {
            group_id: "g".to_owned(),
            bytecode_id: "b".to_owned(),
            target_application_id: ApplicationId::from([0_u8; 32]),
            member_count: 0,
            context_count: 0,
            active_upgrade: None,
            default_capabilities: 0,
            subgroup_visibility: String::new(),
            metadata: None,
            group_state_hash: String::new(),
        };
        let json = serde_json::to_string(&data).expect("serialize");
        assert!(json.contains("\"appKey\""), "got: {json}");
        assert!(!json.contains("\"bytecodeId\""), "got: {json}");
    }

    // The request half of the same contract: an old server only knows `appKey`,
    // so a new client must still put that name on the wire.
    #[test]
    fn create_group_request_serializes_the_legacy_wire_name() {
        let req = CreateGroupApiRequest {
            group_id: None,
            bytecode_id: Some("aabb".to_owned()),
            application_id: ApplicationId::from([0_u8; 32]),
            name: None,
            parent_group_id: None,
        };
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(json.contains("\"appKey\""), "got: {json}");
        assert!(!json.contains("\"bytecodeId\""), "got: {json}");
    }

    #[test]
    fn create_namespace_request_serializes_the_legacy_wire_name() {
        let req = CreateNamespaceApiRequest {
            application_id: ApplicationId::from([0_u8; 32]),
            name: None,
            bytecode_id: Some("aabb".to_owned()),
        };
        let json = serde_json::to_string(&req).expect("serialize");
        assert!(json.contains("\"appKey\""), "got: {json}");
        assert!(!json.contains("\"bytecodeId\""), "got: {json}");
    }

    #[test]
    fn create_namespace_request_accepts_the_new_field() {
        let json = r#"{"bytecodeId":"aabb","applicationId":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
        let req: CreateNamespaceApiRequest =
            serde_json::from_str(json).expect("bytecodeId must deserialize");
        assert_eq!(req.bytecode_id.as_deref(), Some("aabb"));
    }

    #[test]
    fn create_namespace_request_still_accepts_the_legacy_camel_case_alias() {
        let json = r#"{"appKey":"aabb","applicationId":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
        let req: CreateNamespaceApiRequest =
            serde_json::from_str(json).expect("legacy appKey must still deserialize");
        assert_eq!(req.bytecode_id.as_deref(), Some("aabb"));
    }
}
