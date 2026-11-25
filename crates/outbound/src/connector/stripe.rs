//! Stripe PaymentIntents, pinned to API version `2024-06-20`
//! (<https://docs.stripe.com/api/payment_intents/create> and
//! <https://docs.stripe.com/api/payment_intents/retrieve>): form-encoded, the
//! secret key as a bearer token.

use calimero_primitives::outbound::OutboundMethod;

use super::{Body, Connector, Credential, Field, Kind, Operation, Output, Place};

/// The secret binding the Stripe connector attaches: a restricted or secret key.
pub const STRIPE_SECRET: &str = "STRIPE_SECRET_KEY";

/// The limit holding the metadata keys a release declares.
pub const METADATA: &str = "metadata";

/// What both operations return.
const PAYMENT_INTENT: &[Output] = &[
    Output {
        to: &["id"],
        from: &["id"],
        limit: None,
    },
    Output {
        to: &["status"],
        from: &["status"],
        limit: None,
    },
    Output {
        to: &["amount"],
        from: &["amount"],
        limit: None,
    },
    Output {
        to: &["currency"],
        from: &["currency"],
        limit: None,
    },
    Output {
        to: &["customer"],
        from: &["customer"],
        limit: None,
    },
    // What a payer's browser confirms the intent with (Stripe.js), not the key.
    Output {
        to: &["client_secret"],
        from: &["client_secret"],
        limit: None,
    },
    Output {
        to: &["metadata"],
        from: &["metadata"],
        limit: Some(METADATA),
    },
];

/// `stripe`: create and retrieve a PaymentIntent.
pub const STRIPE: Connector = Connector {
    name: "stripe",
    version: "2024-06-20",
    host: "api.stripe.com",
    credential: Credential {
        secret: STRIPE_SECRET,
        header: "authorization",
        format: "Bearer {}",
    },
    headers: &[("stripe-version", "2024-06-20")],
    operations: &[
        Operation {
            name: "createPaymentIntent",
            method: OutboundMethod::Post,
            path: "/v1/payment_intents",
            body: Body::Form,
            fields: &[
                // The smallest currency unit; Stripe's own ceiling is eight digits.
                Field {
                    name: "amount",
                    kind: Kind::Integer {
                        min: 1,
                        max: 99_999_999,
                    },
                    place: Place::Body,
                    required: true,
                },
                Field {
                    name: "currency",
                    kind: Kind::Code { len: 3 },
                    place: Place::Body,
                    required: true,
                },
                Field {
                    name: "customer",
                    kind: Kind::Id {
                        prefix: "cus_",
                        max: 255,
                    },
                    place: Place::Body,
                    required: false,
                },
                // Stripe takes values up to 500 characters.
                Field {
                    name: "metadata",
                    kind: Kind::Map {
                        limit: METADATA,
                        max_value: 500,
                    },
                    place: Place::Body,
                    required: false,
                },
            ],
            output: PAYMENT_INTENT,
        },
        Operation {
            name: "retrievePaymentIntent",
            method: OutboundMethod::Get,
            path: "/v1/payment_intents/{id}",
            body: Body::None,
            fields: &[Field {
                name: "id",
                kind: Kind::Id {
                    prefix: "pi_",
                    max: 255,
                },
                place: Place::Path,
                required: true,
            }],
            output: PAYMENT_INTENT,
        },
    ],
};
