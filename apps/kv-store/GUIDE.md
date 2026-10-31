## Overview
A string key-value store: state is one map of entries keyed by string, and nothing the app relies on lives outside it.
Every change emits an event: `Inserted` (new key), `Updated` (existing key), `Removed` (one key) or `Cleared` (all keys).

## Context model
One context is one independent store, running the app's single service.
Everyone the context admits reads and writes the same entries; a separate store needs a separate context.

## Getting started
1. Create the context (`meroctl context create --name <name> --path kv_store.wasm`, or from an installed package). The store starts empty: there is no setup call.
2. To share it, create it in an organization (`--namespace <org>`) and invite the others to the organization.
3. Call the procedures below with `meroctl call -c <context> <method> --args <json>`.

## Procedures
### Store a value
Call `set`; it replaces any value already under the key.
Example: {"key": "greeting", "value": "hello"}
### Read a value
Call `get`, which returns `null` for a missing key, or `getResult`, which fails with `NotFound` instead.
Example: {"key": "greeting"}
### List everything
Call `entries` for every key and value, ordered by key, or `len` for the number of keys (a decimal string).
Example: {}
### Update only an existing key, or insert only a missing one
`updateIfExists` replaces an existing value and returns `false` without writing when the key is missing.
`getOrInsert` writes only when the key is missing and returns the value the key now holds.
Example: {"key": "greeting", "value": "hi"}
### Delete
Call `remove` for one key (it returns the old value, or `null`) or `clear` for all of them.
Example: {"key": "greeting"}

## Rules and limits
- Keys and values are strings; a key's storage key must stay under 1 KiB.
- Writes are ordered by the app's log: a later write to a key replaces an earlier one.
- There are no roles: anyone the context admits can write, remove or clear any key.
- `getUnchecked` aborts the call when the key is missing; use `get` or `getResult`.
- `remove` on a missing key and `clear` on an empty store change nothing and emit no event.
- `entries` lists at most 1000 keys.
