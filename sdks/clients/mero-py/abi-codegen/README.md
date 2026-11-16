# Calimero ABI Code Generator

A Python tool for generating client wrappers from Calimero ABI schemas. This tool automatically creates Python client code that wraps the base Calimero client with methods corresponding to ABI-defined functions.

## Features

- Parse ABI JSON schemas (WASM ABI v1 and full ABI schemas)
- Generate type-safe Python client wrappers
- Support for both sync and async operations
- Automatic type hints and documentation generation
- Template-based code generation for easy customization

## Installation

```bash
cd abi-codegen
pip install -e .
```

## Usage

### Command Line Interface

```bash
# Generate client from ABI schema
calimero-abi-codegen generate --input schemas/abi.expected.json --output generated_client.py

# Generate with custom template
calimero-abi-codegen generate --input schemas/abi.expected.json --output generated_client.py --template templates/custom_client.py.j2

# Generate async client
calimero-abi-codegen generate --input schemas/abi.expected.json --output async_client.py --async
```

### Python API

```python
from calimero_abi_codegen import ABICodeGenerator

# Create generator
generator = ABICodeGenerator()

# Load ABI schema
with open("schemas/abi.expected.json", "r") as f:
    abi_schema = json.load(f)

# Generate client code
client_code = generator.generate_client(abi_schema, async_client=True)

# Save to file
with open("generated_client.py", "w") as f:
    f.write(client_code)
```

## ABI Schema Support

This tool supports:
- WASM ABI v1 schema format
- Full Calimero ABI schema (abi.expected.json)
- Custom ABI schemas following Calimero conventions

## Generated Client Features

The generated client includes:
- Type-safe method signatures
- Input/output validation
- Error handling
- Documentation strings
- Support for both sync and async operations
- Integration with the base Calimero client

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
