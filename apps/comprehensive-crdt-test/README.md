# Comprehensive CRDT Test Application

This application tests **ALL** CRDT types, UserStorage, FrozenStorage, and root-level concurrent modifications.

## Features Tested

### CRDT Types
- ✅ **Counter** - Grow-only counter with concurrent increments
- ✅ **UnorderedMap** - Field-level merge semantics
- ✅ **Vector** - Element-wise merge
- ✅ **UnorderedSet** - Union merge semantics
- ✅ **RGA (ReplicatedGrowableArray)** - Text CRDT for collaborative editing
- ✅ **LwwRegister** - Last-write-wins register

### Storage Types
- ✅ **UserStorage (Simple)** - User-owned simple values
- ✅ **UserStorage (Nested)** - User-owned nested data structures
- ✅ **FrozenStorage** - Content-addressable immutable storage

### Root-Level Merging
- ✅ **Concurrent Root Modifications** - Tests that root merge works when different nodes modify different root fields concurrently

## Purpose

This app is designed to:
1. Test all CRDT types in a single application
2. Verify root-level concurrent modifications trigger proper merging
3. Test UserStorage and FrozenStorage alongside CRDT types
4. Serve as a comprehensive integration test for the sync protocol

## Usage

Build the app:
```bash
./build.sh
```

Run the workflow:
```bash
merobox bootstrap run workflows/comprehensive-crdt-test.yml
```

## Workflow Tests

The `comprehensive-crdt-test.yml` workflow tests:
1. Root Counter - concurrent increments merge correctly
2. Root Map - field-level merge when different nodes modify different keys
3. Root Vector - element-wise merge
4. Root Set - union merge
5. Root RGA - text CRDT merge
6. Root Register - LWW semantics
7. UserStorage Simple - user-owned data sync
8. UserStorage Nested - nested user data with CRDTs
9. FrozenStorage - content-addressable storage
10. Root-Level Concurrent Modifications - different nodes modifying different root fields simultaneously

## Architecture

The app state (`ComprehensiveCrdtApp`) contains all CRDT types and storage types as root-level fields. This design allows testing root-level concurrent modifications where:
- Node 1 modifies `root_counter`
- Node 2 modifies `root_map` 
- Node 1 modifies `root_set`

All concurrently, triggering `merge_root_state` to merge all fields correctly.
