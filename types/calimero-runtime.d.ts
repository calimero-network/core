/**
 * TypeScript bindings for Calimero runtime functions
 * These provide type-safe access to the underlying Rust Calimero SDK
 */

export interface CalimeroRuntime {
  // Storage functions
  storageRead(key: string): string | null;
  storageWrite(key: string, value: string): boolean;
  storageRemove(key: string): boolean;
  
  // Logging and events
  logUtf8(message: string): void;
  emitEvent(kind: string, data: string): void;
  
  // Context and execution
  getContextId(): string;
  getExecutorId(): string;
  
  // Utility functions
  randomBytes(length: number): Uint8Array;
  timeNow(): number;
}

export interface KvStoreInterface {
  init(): void;
  set(key: string, value: string): void;
  get(key: string): string | null;
  remove(key: string): boolean;
  len(): number;
  clear(): void;
  runDemo(): void;
  isInitialized(): boolean;
}

// WASM module exports
export interface KvStoreWasmModule {
  init(): void;
  set(keyPtr: number, keyLen: number, valuePtr: number, valueLen: number): void;
  get(keyPtr: number, keyLen: number): number;
  remove(keyPtr: number, keyLen: number): number;
  len(): number;
  clear(): void;
  runDemo(): void;
  isInitialized(): number;
  
  // Memory management
  memory: WebAssembly.Memory;
  alloc(size: number): number;
  dealloc(ptr: number): void;
}
