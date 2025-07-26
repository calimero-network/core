# SDK External API

This document explains how to use the `Self::external()` API in the Calimero SDK to call services outside of your Calimero network. It covers creating proposals for external calls, approving or managing them, handling errors and security considerations, and provides a complete end‑to‑end example.

## Overview

`Self::external()` returns a builder that lets you construct a proposal to call an external HTTP endpoint or another smart contract. Because external calls are potentially dangerous, they must be proposed and approved by the organisation’s governance (usually a DAO or multi‑sig) before they are executed. The external call will only be executed once the required approvals are collected.

External calls can be used for things like fetching data from off‑chain APIs, interacting with third‑party services, or bridging into other blockchains.

## Propose external call

To create an external call proposal you call `Self::external()` and configure the target, HTTP method, payload, gas and deposit. Once configured, call `create_proposal()` (or `schedule()` depending on your version) to submit it for approval. The function returns a unique proposal ID.

```rust
use serde_json::json;
use calimero_sdk::prelude::*;

/// Example method inside your contract
pub fn propose_weather_fetch(&mut self) -> u64 {
    // Construct an external call to fetch weather from a public API
    let url = "https://api.weatherapi.com/v1/current.json?key=demo&q=London";
    Self::external()
        .http_get(url)            // target endpoint and method
        .gas(50_000_000_000_000)  // attach gas for the call
        .deposit(0)               // attach NEAR deposit if required by the endpoint
        .memo("Fetch current weather") // optional memo
        .create_proposal()        // returns proposal ID
}
```

In the example above we build a GET request to a weather API.  The `proposal_id` returned can be used to approve or reject the call later.

### Custom payloads

For POST/PUT requests you can include a JSON payload:

```rust
pub fn propose_notify_service(&mut self, message: String) -> u64 {
    let payload = json!({ "message": message }).to_string();

    Self::external()
        .http_post("https://api.example.com/notify", payload)
        .gas(75_000_000_000_000)
        .deposit(0)
        .create_proposal()
}
```

## Approve / Manage external calls

Once a proposal has been created, authorised members of the organisation must approve it.  When enough approvals are collected, the SDK will execute the external call automatically.  You can also cancel or reject proposals that are no longer needed.

Approvals are recorded on chain.  To approve a proposal call `approve_proposal()` with the proposal ID.  To reject or cancel, call `reject_proposal()`.

```rust
/// Approve a pending external call proposal
pub fn approve_weather_fetch(&mut self, proposal_id: u64) {
    // Only authorised signers (e.g. DAO members) should call this
    self.approve_proposal(proposal_id);
}

/// Reject a pending proposal
pub fn reject_weather_fetch(&mut self, proposal_id: u64) {
    self.reject_proposal(proposal_id);
}
```

After approval the external call will be executed and the result (success/failure) will be recorded in the proposal status.  You can query the status via `get_proposal(proposal_id)` to see whether the call has been executed.

## Error handling & security

External calls introduce new failure cases and security considerations:

- **Untrusted endpoints:** Only call well‑known APIs or contracts.  Avoid endpoints that can return unbounded data or cause heavy processing.
- **Gas limits:** Ensure you attach enough gas using `.gas(...)` for the external call to complete.  Insufficient gas will cause the call to fail.
- **Input validation:** Validate all inputs (such as URLs and payloads) before constructing the proposal.  Do not include secrets in the payload—remember that everything on chain is public.
- **Timeouts & retries:** External calls may time out or fail due to network issues.  Design your contract logic to handle failures gracefully.

If a call fails the proposal status will include an error message.  You can handle errors by checking the status after execution and taking appropriate action in your contract.

## End‑to‑end example

Below is a complete example that proposes an external call to fetch the current price of NEAR from a public API, waits for approval, and then processes the result.

```rust
use serde_json::Value;
use calimero_sdk::prelude::*;

#[near_bindgen]
impl PriceOracle {
    pub fn propose_price_update(&mut self) -> u64 {
        let url = "https://api.coinapi.io/v1/exchangerate/NEAR/USD?apikey=demo";

        Self::external()
            .http_get(url)
            .gas(50_000_000_000_000)
            .memo("Fetch NEAR/USD price")
            .create_proposal()
    }

    pub fn approve_price_update(&mut self, proposal_id: u64) {
        self.approve_proposal(proposal_id);
    }

    /// This method could be triggered by an off‑chain indexer after execution
    pub fn on_price_update(&mut self, response: String) {
        // Parse the JSON returned by the API
        let parsed: Value = serde_json::from_str(&response).expect("Invalid JSON");
        let price = parsed["rate"]
            .as_f64()
            .expect("Missing rate");

        self.latest_price = price;
    }
}
```

This contract proposes a price update, an authorised signer approves it, and after execution the result is passed to `on_price_update` where the price is stored.

## FAQ

**What happens if I never approve the proposal?**  
Nothing—the external call will never be executed.  You can close stale proposals with `reject_proposal()`.

**Can I call smart contracts on other networks?**  
Yes, as long as the target is supported by Calimero’s external call infrastructure (e.g., HTTP endpoints or other Near contracts).  Specify the appropriate URL or contract address when building the call.

**How do I view proposal statuses?**  
Use `get_proposal(proposal_id)` to retrieve information about a proposal, including current status, approvals and result once executed.

See also the Rust API documentation for [`Self::external`](#) for details of all builder methods.
