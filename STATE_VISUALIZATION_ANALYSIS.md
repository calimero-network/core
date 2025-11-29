# State Visualization Analysis - Branch: state-visualization-debug

## Summary

This branch was created to systematically verify that:
1. All example apps build and generate `state-schema.json` files ✅
2. Merobox workflows can run successfully (pending - merobox tool not found)
3. State visualization works correctly in merodb GUI
4. All data is properly deserialized in merodb

## Status

### ✅ Completed

1. **All Apps Build Successfully**
   - Verified all 10 example apps build and generate `state-schema.json`:
     - `abi_conformance` ✅
     - `access-control` ✅
     - `blobs` ✅
     - `collaborative-editor` ✅
     - `kv-store` ✅
     - `kv-store-init` ✅
     - `kv-store-with-handlers` ✅
     - `kv-store-with-user-and-frozen-storage` ✅
     - `state-schema-conformance` ✅
     - `xcall-example` ✅

### ✅ Completed

2. **Merobox Installation**
   - **Status**: `merobox` is installed at `/Users/chefsale/.local/bin/merobox` (version 0.2.10)
   - **Note**: Requires Docker to run workflows (Docker not currently running)
   - **Workaround**: Using existing test databases from `apps/collaborative-editor/data/`

3. **Merodb GUI Setup**
   - Built merodb with GUI feature: `cargo build --release --features gui`
   - GUI server started on port 8080
   - Test database available: `apps/collaborative-editor/data/calimero-node-1/calimero-node-1/data`
   - State schema available: `apps/collaborative-editor/res/state-schema.json`

### ⏳ Pending

2. **Merobox Workflows**
   - **Status**: `merobox` is installed but requires Docker
   - **Workflows Found**:
     - `apps/access-control/workflows/test_access_control.yml`
     - `apps/blobs/workflows/blobs-example.yml`
     - `apps/collaborative-editor/workflows/collaborative-editor.yml`
     - `apps/kv-store-with-user-and-frozen-storage/workflows/test_frozen_storage.yml`
     - `apps/kv-store-with-user-and-frozen-storage/workflows/test_user_storage.yml`
     - `apps/kv-store/workflows/bundle-invitation-test.yml`
     - `apps/private_data/workflows/example.yml`
     - `apps/xcall-example/workflows/xcall-example.yml`
   - **Action Required**: Install or locate `merobox` tool, or run workflows manually using `merod` and `meroctl`

3. **State Visualization Analysis**
   - Need to run workflows to generate test data
   - Then analyze merodb GUI state tree visualization
   - Verify deserialization of all CRDT types (RGA, LwwRegister, Counter, etc.)

## State Visualization Architecture

### Flow

1. **Frontend (GUI)**:
   - User loads database path and state schema file
   - GUI calls `/api/contexts` to list all contexts (fast, Meta column scan only)
   - User selects a context from dropdown
   - GUI calls `/api/context-tree` with `context_id` and `state_schema_file`
   - Tree is rendered using D3.js

2. **Backend (merodb)**:
   - `handle_context_tree` endpoint receives request
   - Loads state schema from JSON
   - Calls `export::extract_context_tree(db, context_id, schema)`
   - `extract_context_tree`:
     - Gets `root_hash` from `ContextMeta` (Meta column)
     - Scans State column to find root `EntityIndex` matching `root_hash`
     - Calls `find_and_build_tree_for_context` which:
       - Finds root node by scanning State column
       - Calls `decode_state_root_bfs` to recursively decode state tree
       - Matches schema fields to collection roots
       - Decodes collection entries using BFS traversal
       - Handles RGA specially via `collect_rga_entries`

### Key Functions

- `extract_context_tree`: Main entry point for extracting a context's state tree
- `find_and_build_tree_for_context`: Locates root node and initiates BFS traversal
- `decode_state_root_bfs`: Recursively decodes state tree following schema
- `decode_collection_field_with_root`: Decodes a collection field when root is known
- `decode_collection_entries_bfs`: Decodes entries within a collection
- `collect_rga_entries`: Special handling for RGA text reconstruction
- `decode_state_entry`: Decodes individual state entries (Map, List, RGA)

### Potential Issues to Investigate

1. **Root Node Lookup**:
   - Currently scans entire State column to find root by `root_hash`
   - Could be slow for large databases
   - Alternative: Construct expected key directly if key format is known

2. **Collection Field Matching**:
   - Matches collection fields to collection roots by scanning root's children
   - May fail if multiple collections have same structure
   - Relies on order and structure matching

3. **RGA Deserialization**:
   - Special handling in `collect_rga_entries` to reconstruct text
   - Individual RGA entries decoded as `(CharKey, RgaChar)` tuples
   - Need to verify text reconstruction is correct

4. **Non-Collection Fields**:
   - Currently returns placeholder for non-collection fields
   - These are stored in state root itself, not as separate entries
   - May need to decode state root value directly

5. **Tree Structure**:
   - D3.js expects hierarchical structure with `children` array
   - Collection entries are direct children of field nodes
   - Need to verify structure matches D3.js expectations

## Testing State Visualization

### Current Test Setup

1. **GUI Server**: Running on http://127.0.0.1:8080
   ```bash
   # GUI is already running in background (PID: 6514)
   # To restart:
   cd tools/merodb
   ./target/release/merodb --gui --port 8080
   ```

2. **Test Database**: 
   - Path: `apps/collaborative-editor/data/calimero-node-1/calimero-node-1/data`
   - Contains test data from collaborative-editor workflow

3. **State Schema File**:
   - Path: `apps/collaborative-editor/res/state-schema.json`
   - Generated during build, contains full manifest

### Testing Steps

1. **Open GUI**: Navigate to http://127.0.0.1:8080 in browser

2. **Load Database**:
   - Enter database path: `/Users/chefsale/workspace/calimero/core/apps/collaborative-editor/data/calimero-node-1/calimero-node-1/data`
   - Upload state schema file: `apps/collaborative-editor/res/state-schema.json`
   - Click "Load Database"

3. **Test State Tree Visualization**:
   - Switch to "State Tree" tab
   - Select a context from the dropdown
   - Verify tree structure is displayed correctly
   - Check if all CRDT types are deserialized properly
   - Test RGA text reconstruction

4. **Debug Issues**:
   - Check browser console for errors
   - Check server logs: `tail -f /tmp/merodb-gui.log`
   - Verify root node is found
   - Check collection entries are decoded

## Next Steps

1. **Run Additional Workflows** (when Docker is available):
   ```bash
   # Start Docker first
   # Then run workflows to generate more test data
   cd apps/kv-store
   merobox bootstrap run workflows/bundle-invitation-test.yml
   ```

2. **Test with Different Apps**:
   - Test kv-store database
   - Test other apps with different CRDT types
   - Verify all CRDT types deserialize correctly

4. **Debug Issues**:
   - Check console logs for errors
   - Verify state schema is loaded correctly
   - Check if root node is found
   - Verify collection entries are decoded
   - Test RGA text reconstruction

5. **Verify Deserialization**:
   - Test all CRDT types:
     - `LwwRegister<T>`
     - `Counter`
     - `ReplicatedGrowableArray` (RGA)
     - `UnorderedMap<K, V>`
     - `Vector<T>`
     - `UnorderedSet<T>`

## Files Modified

- `tools/merodb/src/export.rs`: State tree extraction and deserialization
- `tools/merodb/src/gui/server.rs`: API endpoints for state tree
- `tools/merodb/src/gui/static/js/state-tree-visualizer.js`: Frontend visualization
- `tools/calimero-abi/src/extract.rs`: ABI extraction from `abi.json` files
- `crates/wasm-abi/src/validate.rs`: ABI validation including `inner_type` for collections

## Notes

- All apps now generate `state-schema.json` files during build
- ABI is no longer embedded in WASM, always read from `abi.json`
- State schema includes full manifest (schema_version, methods, events, state_root, types)
- Collection types now properly handle `inner_type` field (e.g., `LwwRegister<String>`)

