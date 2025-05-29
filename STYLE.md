# Rust Style Guide

## Formatting

### Use rustfmt with nightly features to maintain consistent code formatting

```bash
cargo +nightly fmt
```

### Sort `Cargo.toml` dependencies alphabetically

### Organize imports according to the `StdExternalCrate` pattern

1. Standard library
2. External crates
3. Symbols from local crate & parent module
4. Local modules definition
5. Symbols from local modules (optional)

```rust
// Standard library
use std::collections::HashMap;
use std::sync::Arc;
use std::{mem, time};

// External crates
use serde::{Serialize, Deserialize};
use tokio::sync::{oneshot, RwLock};

// Symbols from local crate & parent module
use crate::{common, Node};
use super::Shared;

// Local modules definition
mod config;
mod types;

// Symbols from local modules (optional)
use config::ContextConfig;
use types::BroadcastMessage;
```

### Use module import granularity. This means not grouping imports from the same crate together

**NOT ALLOWED:**

```rust
use core::{
    future::{pending, Future},
    pin::Pin,
    str,
};
use std::{thread, time::Duration};
```

**ALLOWED:**

```rust
use core::future::{pending, Future};
use core::pin::Pin;
use core::str;
use std::thread;
use std::time::Duration;
```

## Module Organization

### We don't use the `mod.rs` pattern

Instead, export modules from files named according to their context.

For example:

```bash
crates/meroctl/src/cli/app.rs
```

Contains:

```rust
mod get;
mod install;
mod list;
```

And we would have these individual files:

```bash
crates/meroctl/src/cli/app/get.rs
crates/meroctl/src/cli/app/install.rs
crates/meroctl/src/cli/app/list.rs
```

## Error Handling

### We use the eyre crate extensively in our code, and import it as follows

```rust
use eyre::{Result as EyreResult};
```

### Employ maximum caution dealing with panic points (.unwrap(), .expect(..), assert!, panic!, etc)

Only introduce them when you have maximum confidence it will NEVER panic, or if
it does, it's a fatal error from which we cannot recover and aborting the node
is the best thing to do. In this case, introduce a comment stating why

### If unwrapping is absolutely necessary, explain why with a comment

### On values that may return errors, use `.map_err()` to map the error into the appropriate Error type used in that crate/module

## Code Efficiency

### Try to limit unnecessary clones

### Use short-circuiting if statements

```rust
// NOT RECOMMENDED:
if some_condition {
    // ... (lots of code)
} else {
    return Err(YourError::Something);
}
// RECOMMENDED:
if !some_condition {
    return Err(YourError::Something);
}
// Continue with main code path...
```

### Use `let..else` for deep conditionals

When using `if let..else` with the consequent block extending beyond just a
couple of lines and the alternative effectively bails, prefer the `let..else`
syntax to reduce indentation.

```rust
// NOT RECOMMENDED:
if let Ok(val) = thing {
    func(val);
    do_ok_1;
    do_ok_2;
    do_ok_3;
} else {
    do_else_1;
    return;
}
// RECOMMENDED:
let Ok(val) = thing else {
    do_else_1;
    return;
};
func(val);
do_ok_1;
do_ok_2;
do_ok_3;
```

This syntax helps flatten out indentation and keeps short-circuits closer to the
condition that triggered them, making code easier to read.

## Code Organization

### Break functions into smaller parts if they become too large

### Place reusable functions in a `commons.rs` file or similar

### Put structs needed by multiple parts of the codebase into `primitives` files

### We try to avoid using fully qualified names, prefer using imports

```rust
// NOT RECOMMENDED:
std::fs::File::open("data.txt")

// RECOMMENDED:
use std::fs::File;
File::open("data.txt")
```

## Naming Conventions

- Types shall be `PascalCase`
- Enum variants shall be `PascalCase`
- Struct fields shall be `snake_case`
- Function and method names shall be `snake_case`
- Local variables shall be `snake_case`
- Macro names shall be `snake_case`
- Constants (consts and immutable statics) shall be `SCREAMING_SNAKE_CASE`

## General Guidelines

### Try to use functional Rust patterns when they improve code readability and maintainability
