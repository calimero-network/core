# calimero-connector-nearai

One exchange with [NEAR AI Cloud](https://cloud-api.near.ai)'s chat completions (OpenAI-compatible) from a Calimero app's [action](https://calimero-network.github.io/core/protocol/execution/#actions-component-apps).

```rust
pub fn chat(ctx: &mut ActionCtx, request: &Prompt) -> Result<Answer, Error>
```

`chat` posts the prompt to `https://cloud-api.near.ai/v1/chat/completions` with `content-type: application/json` and an `idempotency-key` that stays the same across the node's retries of the action, so a retried action is answered once.
It never sends a key: the node attaches the instance's `NEAR_AI_API_KEY` through the manifest's credential binding, and the app never sees it.
What an app keeps, who may ask and how much stay the app's own.

## Add it to an app

Take it from core by tag, the same tag as `calimero-sdk` (one core tag for every core crate):

```toml
[dependencies]
calimero-sdk = { git = "https://github.com/calimero-network/core", tag = "<core tag>" }
calimero-connector-nearai = { git = "https://github.com/calimero-network/core", tag = "<core tag>" }
```

Declare the endpoint and bind the key in the app's `Cargo.toml`, which `meroctl app pack` signs into the manifest:

```toml
[package.metadata.calimero]
outbound = ["https://cloud-api.near.ai/v1/chat/completions"]

[package.metadata.calimero.secrets.NEAR_AI_API_KEY]
host = "cloud-api.near.ai"
paths = ["/v1/chat/completions"]
header = "authorization"
format = "Bearer {}"
description = "NEAR AI Cloud API key"
required = true
```

## Call it from an action

```rust
use calimero_connector_nearai::{chat, Answer, Prompt};

#[app::action]
pub fn ask(ctx: &mut ActionCtx, prompt: Prompt) -> app::Result<Answer> {
    Ok(chat(ctx, &prompt)?)
}
```

`Prompt`, `Answer` and the message types derive `AbiType`, so they can be a method's arguments or result.
They keep OpenAI's snake_case field names on the wire.
An answer's `message` goes straight back into the next prompt's `messages`, which is how a tool call's result returns to the model.

## Errors

`Error` converts into the SDK's `AppError` (`?` in a method returning `app::Result`):

| Variant | Code | Fields | When |
| --- | --- | --- | --- |
| `NotSent` | `NearAiNotSent` | `reason` | The node did not send the request (host not approved, timeout) |
| `Status` | `NearAiStatus` | `status`, `message` | NEAR AI answered an error status; `message` is its error's, or `null` |
| `Unusable` | `NearAiUnusable` | `reason` | No answer to use, e.g. "the model ran out of tokens before answering" |
