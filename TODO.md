# Migration: Replace SDK Macros with Emitter for ABI Generation

## Overview
Replace the current SDK macro-based ABI generation system with the emitter-based approach to:
- Reduce source code size significantly
- Create a single point of truth for ABI generation
- Improve maintainability and consistency

## Current State Analysis

### Current System (SDK Macros)
- **Location**: `crates/sdk/macros/src/abi.rs` (~1123 lines)
- **Process**: Compile-time macro expansion → WASM embedding → extraction
- **Issues**: 
  - Large, complex macro code
  - Incomplete Vec/BTreeMap handling
  - Hardcoded type definitions
  - Difficult to maintain and debug

### Target System (Emitter)
- **Location**: `crates/wasm-abi-v1/src/emitter.rs` (~300 lines)
- **Process**: Source code analysis → ABI generation → WASM embedding
- **Benefits**:
  - Cleaner, more maintainable code
  - Proper type handling
  - Single source of truth
  - Better testability

## Migration Steps

### Phase 1: Emitter Enhancement
- [ ] **1.1** Complete emitter type analysis
  - [ ] Add proper struct field analysis
  - [ ] Add enum variant analysis with payloads
  - [ ] Handle all Rust type patterns:
    - [ ] **Generics**: `Vec<T>`, `Option<T>`, `Result<T,E>`, `BTreeMap<K,V>`, custom generics
    - [ ] **Lifetimes**: `&'a T`, `&'static T`, lifetime parameters, lifetime bounds
    - [ ] **Traits**: `impl Trait`, associated types, trait bounds
    - [ ] **Complex patterns**: tuple structs, newtypes, phantom data
    - [ ] **Const generics**: `[T; N]`, `Array<T, N>`
  - [ ] Add support for `#[derive]` attributes
  - [ ] **Keep wasm32 parameter** for correct type mapping (usize/isize → u32/i32)

- [ ] **1.2** Enhance emitter with full type resolution
  - [ ] Implement proper type definition collection
  - [ ] Handle cross-module type references
  - [ ] Add support for external crate types
  - [ ] Implement proper error handling and reporting
  - [ ] Add support for complex type paths: `std::collections::HashMap`

- [ ] **1.3** Add emitter integration points
  - [ ] Create build-time integration (build.rs)
  - [ ] Add WASM embedding functionality
  - [ ] Create extraction tool integration
  - [ ] Add validation and error reporting

### Phase 2: Build System Integration
- [ ] **2.1** Create build-time emitter integration
  - [ ] Create `build.rs` script for each app
  - [ ] Implement source code parsing and analysis
  - [ ] Generate ABI manifest at build time
  - [ ] Embed ABI into WASM custom section

- [ ] **2.2** Update build scripts
  - [ ] Modify `apps/abi_conformance/build.sh`
  - [ ] Update other app build scripts
  - [ ] Add emitter invocation to build process
  - [ ] Ensure proper error handling

- [ ] **2.3** Update extraction tools
  - [ ] Modify `tools/calimero-abi` to work with emitter output
  - [ ] Ensure compatibility with existing tools
  - [ ] Add validation for emitter-generated ABI

### Phase 3: SDK Macro Replacement
- [ ] **3.1** Identify macro usage points
  - [ ] Audit all `#[app::logic]` usages
  - [ ] Audit all `#[app::event]` usages
  - [ ] Audit all `#[app::state]` usages
  - [ ] Document current macro behavior

- [ ] **3.2** Create migration utilities
  - [ ] Build compatibility layer for existing macros
  - [ ] Create migration scripts
  - [ ] Add deprecation warnings
  - [ ] Ensure backward compatibility during transition

- [ ] **3.3** Gradual macro removal
  - [ ] Start with `abi_conformance` app
  - [ ] Migrate other apps one by one
  - [ ] Remove macro code after all apps migrated
  - [ ] Clean up unused dependencies

### Phase 4: Testing and Validation
- [ ] **4.1** Comprehensive testing
  - [ ] Test all existing apps with emitter
  - [ ] Verify ABI compatibility
  - [ ] Test build process integration
  - [ ] Validate extraction tools

- [ ] **4.2** Performance validation
  - [ ] Measure build time impact
  - [ ] Compare generated ABI sizes
  - [ ] Validate memory usage
  - [ ] Ensure no regressions

- [ ] **4.3** Documentation updates
  - [ ] Update developer documentation
  - [ ] Create migration guides
  - [ ] Update API documentation
  - [ ] Add troubleshooting guides

## Potential Inconveniences/Challenges

### 1. **Build Time Impact**
- **Issue**: Source code analysis at build time may increase build times
- **Mitigation**: Implement caching, incremental analysis, parallel processing

### 2. **Complex Type Analysis**
- **Issue**: Rust's type system is complex (generics, lifetimes, traits, etc.)
- **Mitigation**: Start with simple cases, gradually add complexity, extensive testing

### 3. **External Dependencies**
- **Issue**: Apps may use types from external crates
- **Mitigation**: Implement proper dependency resolution, fallback mechanisms

### 4. **Macro Behavior Differences**
- **Issue**: Current macros may have edge cases not covered by emitter
- **Mitigation**: Comprehensive testing, gradual migration, compatibility layer

### 5. **Breaking Changes**
- **Issue**: Migration may introduce breaking changes
- **Mitigation**: Maintain backward compatibility, provide migration tools

### 6. **Error Handling**
- **Issue**: Source code analysis errors need clear reporting
- **Mitigation**: Implement detailed error messages, validation tools

## Risk Assessment

### High Risk
- Complex type analysis edge cases
- Build time performance impact
- Breaking existing functionality

### Medium Risk
- External dependency handling
- Migration complexity
- Testing coverage gaps

### Low Risk
- Basic type analysis
- Simple integration points
- Documentation updates

## Success Criteria

- [ ] All existing apps work with emitter
- [ ] Build times remain acceptable
- [ ] ABI output is identical or better
- [ ] No breaking changes for users
- [ ] Reduced codebase size
- [ ] Improved maintainability

## Timeline Estimate

- **Phase 1**: 2-3 weeks (emitter enhancement)
- **Phase 2**: 1-2 weeks (build integration)
- **Phase 3**: 2-3 weeks (migration)
- **Phase 4**: 1-2 weeks (testing)

**Total**: 6-10 weeks

## Next Steps

1. **Immediate**: Start with Phase 1.1 (complete emitter type analysis)
2. **Validation**: Test with `abi_conformance` app first
3. **Iterative**: Build and test incrementally
4. **Communication**: Keep stakeholders informed of progress and challenges 