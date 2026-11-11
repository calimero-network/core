//! The route layer, generated from the node's OpenAPI document
//! (`docs/public/openapi.json`) by `calimero-client-gen` (`tools/client-gen`)
//! and checked in: [`routes`], one method per operation on its group's
//! accessor (`client.orgs().invite_create(..)`), and [`types`], what they take
//! and answer. Nothing under `generated/` is edited by hand: change the node's
//! routes, regenerate the document, then run `cargo run -p calimero-client-gen`.

pub mod routes;
pub mod types;
