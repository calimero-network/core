# Calimero ABI Code Generator

> **Deprecated.**
> Use `meroctl app codegen --abi abi.json --lang python -o client` instead.
> This tool's output calls `client.call(...)`, which the current client no longer has, so it fails at runtime.

A Python tool for generating client wrappers from Calimero ABI schemas. This tool automatically creates Python client code that wraps the base Calimero client with methods corresponding to ABI-defined functions.

## Installation

```bash
cd abi-codegen
pip install -e .
```

## Usage

### Command Line Interface

```bash
# Generate client from ABI schema
calimero-abi-codegen --input examples/schemas/example_abi.json --output generated_client
```

## ABI Schema Support

This tool supports:
- WASM ABI v1 schema format
- Full Calimero ABI schema (abi.expected.json)
- Custom ABI schemas following Calimero conventions

## Development

```bash
# Install development dependencies
pip install -e ".[dev]"

# Run tests
pytest

# Format code
black src tests
isort src tests

# Type checking
mypy src
```

## Project Structure

```
abi-codegen/
├── src/
│   └── calimero_abi_codegen/
│       ├── __init__.py
│       ├── parser.py          # ABI schema parser
│       ├── generator.py       # Code generator
│       ├── templates.py       # Template management
│       └── cli.py            # Command-line interface
├── templates/
│   ├── client.py.j2          # Sync client template
│   ├── async_client.py.j2    # Async client template
│   └── types.py.j2           # Type definitions template
├── schemas/
│   ├── wasm-abi-v1.schema.json
│   └── abi.expected.json
├── tests/
├── examples/
└── pyproject.toml
```
