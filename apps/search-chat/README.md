# search-chat

The example app for **full-text search**: a chat log whose search view asks the
node's index instead of reading every message in WASM.

## The opt-in

Two declarations. The value type says which fields are indexed, and how:

```rust
#[derive(BorshSerialize, BorshDeserialize, AbiType, app::Mergeable, app::Searchable)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Message {
    #[search(keyword)]     pub sender: LwwRegister<String>, // exact filter
    #[search(text, infix)] pub text: LwwRegister<String>,   // ranked words + substrings
    #[search(number)]      pub ts: LwwRegister<u64>,        // range filter
}
```

and the state says which collection is the index:

```rust
app::search_indexes!(Chat {
    "messages" (version = 1) => messages,
});
```

The `search` view then builds a typed `Query` and runs it through the map,
which reads every hit back from state:

```rust
let results = self.messages.search("messages", &Query::words(query).eq("sender", sender))?;
```

## Methods

| method | kind | what it does |
|---|---|---|
| `post`, `post_many`, `edit`, `delete` | write | change messages; the node indexes them on its own, off the write path |
| `get`, `count` | view | read state |
| `search(query, mode?, sender?, since?, cursor?, limit?)` | view | `words` (default), `prefix`, `substring` or `fuzzy`; an empty query lists what the filters admit |
| `scan_search(term, limit?)` | view | the baseline the index replaces: a lowercase substring test over every message, in WASM |
| `search_in_a_write(query)` | write | a fixture: a mutating method that searches traps, and commits nothing |

## What it costs

On this app a `post` costs about 2.5 M gas whether the node runs search or not;
the dirty row the node writes beside it is host bookkeeping, never gas. A
`search` view for the top 20 of 200,000 messages costs a few million gas, the
host's work included. `scan_search` runs out of the one-billion budget before
16,000 messages. The measurements are in `tools/search-bench/README.md`.

## The scenario

`workflows/search-chat.yml` runs two real nodes and two contexts:

1. Node 1 posts; node 2 receives the message by sync, indexes it itself, and
   finds it.
2. Node 2 deletes the message; both nodes stop returning it, and both indexes
   drop it.
3. The same query in the second context, on either node, never returns the
   first context's message.

## Build and test

```bash
cargo mero build
cargo test -p calimero-context --lib search_tests   # the node-side suite, which builds this app
merobox bootstrap run workflows/search-chat.yml
```
