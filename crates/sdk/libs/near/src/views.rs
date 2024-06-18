#[derive(serde::Deserialize, Debug, Clone)]
pub struct AccountView {
    pub amount: String,
    pub locked: String,
    pub code_hash: String,
    pub storage_usage: String,
    pub storage_paid_at: String,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct ContractCodeView {
    #[serde(rename = "code_base64")]
    pub code: String,
    pub hash: String,
}
