//! OpenAI-compatible chat completions, pinned to OpenAI's `v1` Chat Completions
//! API (<https://platform.openai.com/docs/api-reference/chat/create>): a JSON
//! body, the API key as a bearer token.

use calimero_primitives::outbound::OutboundMethod;

use super::{Body, Connector, Credential, Field, Kind, Operation, Output, Place};

/// The secret binding the OpenAI connector attaches.
pub const OPENAI_SECRET: &str = "OPENAI_API_KEY";

/// The limit holding the models the instance's owner allows.
pub const MODEL: &str = "model";

/// `openai`: one chat completion, answered with its text and token usage.
pub const OPENAI: Connector = Connector {
    name: "openai",
    version: "v1",
    host: "api.openai.com",
    credential: Credential {
        secret: OPENAI_SECRET,
        header: "authorization",
        format: "Bearer {}",
    },
    headers: &[],
    operations: &[Operation {
        name: "chatCompletion",
        method: OutboundMethod::Post,
        path: "/v1/chat/completions",
        body: Body::Json,
        fields: &[
            Field {
                name: "model",
                kind: Kind::Choice { limit: MODEL },
                place: Place::Body,
                required: true,
            },
            Field {
                name: "messages",
                kind: Kind::Messages {
                    max: 64,
                    roles: &["system", "user", "assistant"],
                    max_content: 32 * 1024,
                },
                place: Place::Body,
                required: true,
            },
            Field {
                name: "max_tokens",
                kind: Kind::Integer {
                    min: 1,
                    max: 16_384,
                },
                place: Place::Body,
                required: false,
            },
        ],
        output: &[
            Output {
                to: &["content"],
                from: &["choices", "0", "message", "content"],
                limit: None,
            },
            Output {
                to: &["usage", "prompt_tokens"],
                from: &["usage", "prompt_tokens"],
                limit: None,
            },
            Output {
                to: &["usage", "completion_tokens"],
                from: &["usage", "completion_tokens"],
                limit: None,
            },
            Output {
                to: &["usage", "total_tokens"],
                from: &["usage", "total_tokens"],
                limit: None,
            },
        ],
    }],
};
