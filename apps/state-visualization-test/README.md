# State Visualization Test App

This app is a **test fixture** for merodb's state visualization and schema inference capabilities.

## Purpose

This app is designed to verify that:

1. **`field_name`** is correctly stored in entity metadata for all CRDT collection types
2. **Schema inference** can detect all field types from the database without requiring an external schema file
3. **merodb GUI** correctly displays and visualizes different collection types

## CRDT Types Included

| Field | CRDT Type | Description |
|-------|-----------|-------------|
| `items` | `UnorderedMap<String, LwwRegister<String>>` | Key-value pairs |
| `operation_count` | `Counter` | Grow-only counter |
| `operation_history` | `Vector<LwwRegister<String>>` | Ordered operation log |
| `tags` | `UnorderedSet<String>` | Unique tags |
| `metadata` | `LwwRegister<String>` | Single value register |

## Usage

### Build

```bash
./build.sh
```

### Test with merodb

1. Install the app on a node
2. Create a context
3. Call `populate_sample_data` to generate test data
4. Use `merodb gui` to visualize the state

```bash
# Install and create context
meroctl --node <node> app install --path apps/state-visualization-test/res/state_visualization_test.wasm
meroctl --node <node> context create --application-id <app-id> --protocol near

# Populate test data
meroctl --node <node> call --context <ctx> --as <executor> populate_sample_data

# View stats
meroctl --node <node> call --context <ctx> --as <executor> get_stats

# Start merodb GUI (no schema file needed!)
merodb gui
```

## Note

This app is **NOT for production use**. It's a development test fixture for the merodb visualization tools.
