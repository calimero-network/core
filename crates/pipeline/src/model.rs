use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// One structural or apply-time problem, addressed by a dotted path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpecError {
    pub path: String,
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}]: {}", self.path, self.code, self.message)
    }
}

/// What the cluster offers: module methods with their caps, and pool memory ceilings.
#[derive(Clone, Debug, Default)]
pub struct Env {
    /// The module a step runs in when neither it nor the pipeline names one.
    pub default_module: Option<String>,
    pub modules: BTreeMap<String, BTreeMap<String, Caps>>,
    pub pools: BTreeMap<String, u32>,
}

/// The operator's prices, in micro-units of the billing currency per unit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PriceTable {
    /// Strictly increasing; a run records the version it was admitted under.
    pub version: u64,
    /// Fuel one vCPU-second is worth, about 1e9.
    pub fuel_per_vcpu_second: u64,
    pub vcpu_second: u64,
    pub gib_second: u64,
    /// Stored now, used once connectors exist.
    pub connector_gib: u64,
    pub storage_gib_month: u64,
    pub egress_gib: u64,
}

impl PriceTable {
    /// A table prices fuel in vCPU-seconds, so the rate cannot be zero.
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.fuel_per_vcpu_second == 0 {
            return Err(SpecError {
                path: "fuelPerVcpuSecond".into(),
                code: "BAD_PRICES",
                message: "the fuel rate per vCPU-second must be above zero".into(),
            });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Caps {
    pub fuel: u64,
    pub memory_mib: u32,
    pub wall_secs: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pipeline {
    pub api_version: String,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub params: BTreeMap<String, Param>,
    #[serde(default)]
    pub inputs: BTreeMap<String, Input>,
    #[serde(default)]
    pub sources: BTreeMap<String, Connector>,
    #[serde(default)]
    pub sinks: BTreeMap<String, Connector>,
    pub steps: BTreeMap<String, Step>,
    #[serde(default)]
    pub publish: Vec<Publish>,
    #[serde(default)]
    pub notify: Vec<Notify>,
    #[serde(default)]
    pub outputs: Vec<String>,
    #[serde(default)]
    pub fail_fast: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamType {
    Number,
    String,
    Boolean,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Param {
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(default)]
    pub default: Option<Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default, rename = "enum")]
    pub values: Option<Vec<Value>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    Files,
    Artifact,
    Table,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    pub kind: InputKind,
    #[serde(default)]
    pub accept: Vec<String>,
    #[serde(default)]
    pub max_items: Option<u32>,
    #[serde(default)]
    pub max_file_size: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub schema: BTreeMap<String, String>,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Connector {
    pub connector: String,
    pub uri: String,
    #[serde(default)]
    pub connection: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Publish {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Notify {
    pub on: Vec<String>,
    pub to: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Step {
    #[serde(default)]
    pub module: Option<String>,
    pub method: String,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub args: BTreeMap<String, Value>,
    #[serde(default)]
    pub for_each: Option<String>,
    #[serde(default)]
    pub max_items: Option<u32>,
    #[serde(default)]
    pub max_parallel: Option<u32>,
    #[serde(default)]
    pub batch: Option<u32>,
    #[serde(default)]
    pub collect: Option<String>,
    #[serde(default)]
    pub allow_failures: Option<AllowFailures>,
    #[serde(default)]
    pub pool: Option<String>,
    #[serde(default)]
    pub limits: Option<Limits>,
    #[serde(default = "default_true")]
    pub cache: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    #[serde(default)]
    pub fuel: Option<u64>,
    #[serde(default)]
    pub memory_mib: Option<u32>,
    #[serde(default)]
    pub wall_secs: Option<u32>,
}

/// Tolerated failed items: a percentage ("5%") or a count (3 or "3").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllowFailures {
    Percent(u8),
    Count(u32),
}

impl Serialize for AllowFailures {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Percent(p) => s.serialize_str(&format!("{p}%")),
            Self::Count(n) => s.serialize_u32(*n),
        }
    }
}

impl<'de> Deserialize<'de> for AllowFailures {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(u32),
            Text(String),
        }
        let bad = |what: &str| serde::de::Error::custom(format!("invalid allowFailures {what}"));
        match Raw::deserialize(d)? {
            Raw::Num(n) => Ok(Self::Count(n)),
            Raw::Text(t) => match t.strip_suffix('%') {
                Some(p) => match p.parse::<u8>() {
                    Ok(p) if p <= 100 => Ok(Self::Percent(p)),
                    _ => Err(bad("percentage")),
                },
                None => t.parse().map(Self::Count).map_err(|_| bad("count")),
            },
        }
    }
}
