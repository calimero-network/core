//! A release's first-party connectors (`BundleManifest::connectors`): for each connector of
//! the node it calls (`stripe`, `openai`), the operations it may call and the values each of
//! the connector's limits allows (`metadata` keys, `model`s). Signed with the rest and shown
//! on the consent page; the node checks the names against its own connectors at install
//! (`calimero_outbound::check_grant`), and here only the shape and the bounds.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Most connectors one release declares.
pub const MAX_CONNECTORS: usize = 8;
/// Most operations one connector's declaration names.
pub const MAX_CONNECTOR_OPERATIONS: usize = 32;
/// Most limits one connector's declaration sets.
pub const MAX_CONNECTOR_LIMITS: usize = 8;
/// Most values one limit holds (the node's `calimero_outbound::MAX_LIMIT_VALUES`).
pub const MAX_CONNECTOR_LIMIT_VALUES: usize = 64;
/// Most bytes of a connector's, an operation's or a limit's name.
pub const MAX_CONNECTOR_NAME_BYTES: usize = 64;
/// Most bytes of one limit value (a metadata key, a model).
pub const MAX_CONNECTOR_VALUE_BYTES: usize = 128;

/// One connector a release declares: `{"operations": [..], "<limit>": [..]}`, e.g.
/// `{"operations": ["createPaymentIntent"], "metadata": ["order_id"]}`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct BundleConnector {
    /// The operations the release may call; any other is refused.
    pub operations: BTreeSet<String>,
    /// The values each of the connector's limits allows, by the limit's name.
    #[serde(flatten)]
    pub limits: BTreeMap<String, BTreeSet<String>>,
}

/// Why a manifest's `connectors` are refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConnectorsError {
    #[error("a release declares at most {MAX_CONNECTORS} connectors")]
    TooMany,
    #[error(
        "connector name {0:?} must be a-z, 0-9 and _, starting with a letter, at most \
         {MAX_CONNECTOR_NAME_BYTES} bytes"
    )]
    BadName(String),
    #[error(
        "connector {0} declares 1 to {MAX_CONNECTOR_OPERATIONS} operations, each at most \
         {MAX_CONNECTOR_NAME_BYTES} bytes of A-Za-z0-9_"
    )]
    BadOperations(String),
    #[error(
        "connector {0} sets at most {MAX_CONNECTOR_LIMITS} limits, each 1 to \
         {MAX_CONNECTOR_LIMIT_VALUES} values of at most {MAX_CONNECTOR_VALUE_BYTES} printable bytes"
    )]
    BadLimits(String),
}

impl ConnectorsError {
    /// The stable code a node answers with.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        "BAD_CONNECTORS"
    }
}

fn is_name(name: &str, first_lower: bool) -> bool {
    !name.is_empty()
        && name.len() <= MAX_CONNECTOR_NAME_BYTES
        && name.starts_with(|c: char| {
            c.is_ascii_alphabetic() && (!first_lower || c.is_ascii_lowercase())
        })
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Checks the shape and bounds of `connectors`.
///
/// # Errors
/// The first part out of shape or over a bound.
pub fn check(connectors: &BTreeMap<String, BundleConnector>) -> Result<(), ConnectorsError> {
    if connectors.len() > MAX_CONNECTORS {
        return Err(ConnectorsError::TooMany);
    }
    for (name, c) in connectors {
        if !is_name(name, true) || name.chars().any(|c| c.is_ascii_uppercase()) {
            return Err(ConnectorsError::BadName(name.clone()));
        }
        if c.operations.is_empty()
            || c.operations.len() > MAX_CONNECTOR_OPERATIONS
            || !c.operations.iter().all(|o| is_name(o, false))
        {
            return Err(ConnectorsError::BadOperations(name.clone()));
        }
        let value_ok = |v: &String| {
            !v.is_empty()
                && v.len() <= MAX_CONNECTOR_VALUE_BYTES
                && v.chars().all(|c| c.is_ascii_graphic())
        };
        if c.limits.len() > MAX_CONNECTOR_LIMITS
            || !c.limits.iter().all(|(limit, values)| {
                is_name(limit, true)
                    && !values.is_empty()
                    && values.len() <= MAX_CONNECTOR_LIMIT_VALUES
                    && values.iter().all(value_ok)
            })
        {
            return Err(ConnectorsError::BadLimits(name.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(json: serde_json::Value) -> BTreeMap<String, BundleConnector> {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_declaration_reads_its_operations_and_limits() {
        let c = one(serde_json::json!({
            "stripe": { "operations": ["createPaymentIntent"], "metadata": ["order_id"] }
        }));
        assert_eq!(check(&c), Ok(()));
        let stripe = &c["stripe"];
        assert!(stripe.operations.contains("createPaymentIntent"));
        assert_eq!(stripe.limits["metadata"].len(), 1);
        // It writes back as it was read.
        assert_eq!(
            serde_json::to_value(stripe).unwrap(),
            serde_json::json!({ "operations": ["createPaymentIntent"], "metadata": ["order_id"] })
        );
    }

    #[test]
    fn out_of_shape_declarations_are_refused() {
        let err = |v| check(&one(v)).unwrap_err();
        assert_eq!(
            err(serde_json::json!({ "Stripe": { "operations": ["x"] } })),
            ConnectorsError::BadName("Stripe".into())
        );
        assert_eq!(
            err(serde_json::json!({ "stripe": { "operations": [] } })),
            ConnectorsError::BadOperations("stripe".into())
        );
        assert_eq!(
            err(serde_json::json!({ "stripe": { "operations": ["a/b"] } })),
            ConnectorsError::BadOperations("stripe".into())
        );
        assert_eq!(
            err(serde_json::json!({ "stripe": { "operations": ["x"], "metadata": [] } })),
            ConnectorsError::BadLimits("stripe".into())
        );
        assert_eq!(
            err(serde_json::json!({ "stripe": { "operations": ["x"], "metadata": ["a b"] } })),
            ConnectorsError::BadLimits("stripe".into())
        );
        let many: serde_json::Map<String, serde_json::Value> = (0..=MAX_CONNECTORS)
            .map(|i| (format!("c{i}"), serde_json::json!({ "operations": ["x"] })))
            .collect();
        assert_eq!(
            err(serde_json::Value::Object(many)),
            ConnectorsError::TooMany
        );
        // A limit that is not a list of strings does not parse at all.
        assert!(serde_json::from_value::<BTreeMap<String, BundleConnector>>(
            serde_json::json!({ "stripe": { "operations": ["x"], "metadata": "order_id" } })
        )
        .is_err());
    }
}
