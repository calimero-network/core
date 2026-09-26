## Overview
cargo-mero pipeline fixture: a string key-value store.

## Context model
One context is one store, running the single service.

## Getting started
Create a context for this app, then call `set`.

## Procedures
### Store a value
Call `set` with a key and a value. Example: {"key":"hello","value":"world"}

## Rules and limits
Keys and values are strings; `get_unchecked` fails on a missing key.
