## Overview
A string key-value store: state is one map from string keys to string values, and nothing the app relies on lives outside it.
Every change emits an event: `Inserted` (new key), `Updated` (existing key), `Removed` (one key) or `Cleared` (all keys).

## Context model
One context is one independent store, running the app's single service (there are no named services, so `create_context` needs no `service`).
Every member of a context reads and writes the same map; a separate store needs a separate context.

## Getting started
1. Create a namespace for this app with `create_namespace`, then a context in it with `create_context`. The store starts empty: there is no setup call.
2. To share the store, call `invite_to_namespace` and have the other node call `join_namespace` with that invitation, unchanged.
3. Call `select_app` for this app, then use the procedures below.

## Procedures
### Store a value
Call `set`; it replaces any value already under the key.
Example: {"key": "greeting", "value": "hello"}
### Read a value
Call `get`, which returns `null` for a missing key, or `get_result`, which fails with `NotFound` instead.
Example: {"key": "greeting"}
### List everything
Call `entries` for every key and value, ordered by key, or `len` for the number of keys.
Example: {}
### Update only an existing key, or insert only a missing one
`update_if_exists` replaces an existing value and returns `false` without writing when the key is missing.
`get_or_insert` writes only when the key is missing and returns the value the key now holds.
Example: {"key": "greeting", "value": "hi"}
### Delete
Call `remove` for one key (it returns the old value, or `null`) or `clear` for all of them.
Example: {"key": "greeting"}

## Rules and limits
- Keys and values are strings; the app sets no length limit of its own.
- Concurrent writes to the same key from different members converge to the latest write; the earlier ones are lost.
- There are no roles: any member of the context can write, remove or clear any key.
- `get_unchecked` aborts the call when the key is missing; use `get` or `get_result`.
- `remove` on a missing key and `clear` on an empty store change nothing and emit no event.
