## Overview
cargo-mero multi-service pipeline fixture: two key-value services.

## Context model
Each context runs one service, `svc-a` (string values) or `svc-b` (integer values).

## Getting started
Create a context for this app naming the service, then call `set`.

## Procedures
### Store a value
Call `set` on the chosen service. Example: {"key":"hello","value":"world"}

## Rules and limits
`svc-b` accepts only unsigned integer values.
