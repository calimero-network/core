# 🚀 TypeScript WASM Demo - Calimero Integration

This project demonstrates how to implement a **TypeScript-over-Rust WASM** approach for Calimero applications, allowing developers to write application logic in TypeScript while leveraging Rust bindings for Calimero SDK functions.

## 🎯 **What This Achieves**

Instead of writing the entire KV store logic in Rust (like the [original kv-store implementation](https://github.com/calimero-network/kv-store/blob/master/logic/src/lib.rs)), this approach allows you to:

1. **Write application logic in TypeScript** - More familiar for JavaScript/TypeScript developers
2. **Use Rust bindings for Calimero SDK** - Leverage the existing, battle-tested Calimero runtime
3. **Compile to WASM** - Deploy as a standard Calimero WASM module
4. **Maintain type safety** - Full TypeScript support with proper interfaces

## 🏗️ **Architecture**

```
┌─────────────────────────────────────────────────────────────┐
│                    TypeScript Layer                        │
├─────────────────────────────────────────────────────────────┤
│  KvStore Class (Application Logic)                        │
│  • init(), set(), get(), remove(), clear()                │
│  • Event handling, statistics, demo functions             │
├─────────────────────────────────────────────────────────────┤
│                 Integration Layer                          │
├─────────────────────────────────────────────────────────────┤
│  CalimeroWasmIntegration                                  │
│  • WASM module loading                                    │
│  • Global binding setup                                   │
│  • Error handling                                         │
├─────────────────────────────────────────────────────────────┤
│                    Rust Bindings                           │
├─────────────────────────────────────────────────────────────┤
│  wasm-bindgen functions                                   │
│  • storage_read, storage_write, storage_remove            │
│  • log_message, emit_event, get_context_id               │
│  • Mock implementations (easily replaceable)              │
├─────────────────────────────────────────────────────────────┤
│                 Calimero Runtime                           │
├─────────────────────────────────────────────────────────────┤
│  • Storage operations                                     │
│  • Event system                                           │
│  • Context management                                     │
│  • WASM execution environment                             │
└─────────────────────────────────────────────────────────────┘
```

## 📁 **Project Structure**

```
typescript-wasm-demo/
├── src/
│   ├── lib.rs              # Rust bindings with wasm-bindgen
│   ├── kv-store.ts         # TypeScript KV store implementation
│   └── integration.ts      # WASM integration layer
├── dist/                   # Compiled JavaScript (from TypeScript)
├── target/                 # Compiled WASM (from Rust)
├── demo.html               # Interactive demo page
├── package.json            # TypeScript project configuration
├── tsconfig.json           # TypeScript compiler settings
└── Cargo.toml             # Rust project configuration
```

## 🚀 **Getting Started**

### Prerequisites

- Rust toolchain with WASM target: `rustup target add wasm32-unknown-unknown`
- Node.js and npm
- TypeScript: `npm install -g typescript`

### Build Steps

1. **Install dependencies:**
   ```bash
   npm install
   ```

2. **Build TypeScript:**
   ```bash
   npm run build
   ```

3. **Build WASM module:**
   ```bash
   npm run build:wasm
   ```

4. **Start demo server:**
   ```bash
   cd .. && python3 -m http.server 8080
   ```

5. **Open demo:**
   ```
   http://localhost:8080/typescript-wasm-demo/demo.html
   ```

## 🧪 **Testing the Demo**

The demo page provides an interactive interface to test:

- **KV Store Operations**: Set, get, remove, clear key-value pairs
- **Demo Sequence**: Run a complete demonstration of all operations
- **Real-time Updates**: See store state, events, and console output
- **Integration Status**: Monitor WASM loading and initialization

## 🔧 **Key Components**

### 1. **Rust Bindings (`src/lib.rs`)**

Provides the bridge between TypeScript and Calimero runtime:

```rust
#[wasm_bindgen]
pub fn storage_read(key: &str) -> Option<String> {
    // Mock implementation - easily replaceable with real Calimero SDK calls
    match key {
        "demo_key" => Some("demo_value".to_string()),
        "hello" => Some("world".to_string()),
        _ => None
    }
}
```

### 2. **TypeScript KV Store (`src/kv-store.ts`)**

The actual application logic written in TypeScript:

```typescript
export class KvStore {
    private initialized: boolean = false;
    private events: KvStoreEvent[] = [];

    init(): void {
        // Call Rust binding to log initialization
        (globalThis as any).log_message('Initializing TypeScript KV Store');
        this.initialized = true;
        this.addEvent('init', 'Store initialized successfully');
    }

    set(key: string, value: string): void {
        // Call Rust binding to write to storage
        const success = (globalThis as any).storage_write(key, value);
        if (success) {
            this.addEvent('set', `Set ${key} = ${value}`);
        }
    }
}
```

### 3. **Integration Layer (`src/integration.ts`)**

Handles WASM module loading and global binding setup:

```typescript
export class CalimeroWasmIntegration {
    async initialize(wasmPath: string): Promise<void> {
        // Load and compile WASM module
        const wasmBuffer = await fetch(wasmPath).then(r => r.arrayBuffer());
        const wasmModule = await WebAssembly.compile(wasmBuffer);
        
        // Set up global bindings for TypeScript to use
        this.setupGlobalBindings();
    }
}
```

## 🔄 **Migration Path**

### **Phase 1: Mock Implementation (Current)**
- ✅ TypeScript KV store logic
- ✅ Rust bindings interface
- ✅ WASM compilation
- ✅ Integration testing

### **Phase 2: Real Calimero SDK Integration**
- 🔄 Replace mock functions with real Calimero SDK calls
- 🔄 Implement proper error handling
- 🔄 Add real storage operations
- 🔄 Integrate with Calimero event system

### **Phase 3: Production Deployment**
- 🔄 Deploy to Calimero node
- 🔄 Test with real blockchain context
- 🔄 Performance optimization
- 🔄 Documentation and examples

## 🎯 **Benefits of This Approach**

1. **Developer Experience**: Write logic in familiar TypeScript
2. **Type Safety**: Full TypeScript support with proper interfaces
3. **Maintainability**: Easier to maintain and extend than pure Rust
4. **Performance**: Rust bindings provide near-native performance
5. **Ecosystem**: Leverage existing TypeScript/JavaScript tooling
6. **Integration**: Seamless integration with Calimero runtime

## 🔍 **Current Limitations**

- **Mock Implementations**: Storage operations are currently mocked
- **Limited SDK Integration**: Not yet using full Calimero SDK capabilities
- **Development Stage**: Proof of concept, not production ready

## 🚧 **Next Steps**

1. **Real SDK Integration**: Replace mock functions with actual Calimero SDK calls
2. **Error Handling**: Implement proper error handling and recovery
3. **Testing**: Add comprehensive unit and integration tests
4. **Documentation**: Create developer guides and examples
5. **Performance**: Optimize for production use cases

## 🤝 **Contributing**

This is a proof of concept for the TypeScript-over-Rust WASM approach. Contributions are welcome to:

- Improve the integration layer
- Add real Calimero SDK functionality
- Enhance error handling and testing
- Create additional examples and use cases

## 📚 **References**

- [Original Rust KV Store](https://github.com/calimero-network/kv-store/blob/master/logic/src/lib.rs)
- [Calimero Core Repository](https://github.com/calimero-network/core)
- [wasm-bindgen Documentation](https://rustwasm.github.io/docs/wasm-bindgen/)
- [WebAssembly MDN](https://developer.mozilla.org/en-US/docs/WebAssembly)

---

**🎉 This demonstrates that it's possible to write Calimero applications in TypeScript while maintaining the performance and reliability of Rust-based WASM modules!**
