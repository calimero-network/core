# Memory Model: Guest/Host Data Exchange

This document explains how data is exchanged between WASM guest code and the Calimero runtime host.

## Overview

WebAssembly has a linear memory model—a contiguous array of bytes that both guest and host can access. The Calimero runtime uses a **buffer descriptor pattern** to safely exchange data across this boundary.

```
┌─────────────────────────────────────────────────────────────────────────┐
│                         WASM LINEAR MEMORY                               │
│  (accessible by both guest code and host via Wasmer Memory API)          │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  0x0000 ─────────────────────────────────────────────────────────────   │
│         │                                                           │   │
│         │  Stack (grows down)                                       │   │
│         │                                                           │   │
│         │  Heap (grows up)                                          │   │
│         │    ┌─────────────────────────────────────────────────┐    │   │
│         │    │ Buffer Descriptor @ 0x100                       │    │   │
│         │    │ ┌─────────────┬─────────────┐                   │    │   │
│         │    │ │ ptr: 0x200  │ len: 32     │  (16 bytes)       │    │   │
│         │    │ └─────────────┴─────────────┘                   │    │   │
│         │    │                                                 │    │   │
│         │    │ Actual Data @ 0x200                             │    │   │
│         │    │ ┌─────────────────────────────────────────┐     │    │   │
│         │    │ │ 32 bytes of key/value/message data      │     │    │   │
│         │    │ └─────────────────────────────────────────┘     │    │   │
│         │    └─────────────────────────────────────────────────┘    │   │
│         │                                                           │   │
│  0xFFFF ─────────────────────────────────────────────────────────────   │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

## Buffer Descriptor Structure

All variable-length data uses a two-level indirection:

```rust
// Defined in calimero-sys crate
#[repr(C)]
pub struct Slice<'a, T> {
    ptr: Pointer<T>,  // u64: offset in WASM linear memory
    len: u64,         // length in elements (bytes for Buffer)
    _phantom: PhantomData<&'a T>,
}

pub type Buffer<'a> = Slice<'a, u8>;     // Read-only byte buffer
pub type BufferMut<'a> = Buffer<'a>;     // Writable byte buffer
```

**Size**: 16 bytes (8 for pointer + 8 for length)

## Host Function Call Flow

### Reading Data from Guest

```
Guest Code                          Host (VMHostFunctions)
─────────────────────────────────────────────────────────────────────────
1. Allocate space for data          
   let data = b"hello";             
   let data_ptr = allocate(5);      
   memory[data_ptr..] = data;       
                                    
2. Create buffer descriptor         
   let desc_ptr = allocate(16);     
   memory[desc_ptr..] = Buffer {    
       ptr: data_ptr,               
       len: 5                       
   };                               
                                    
3. Call host function               
   storage_write(desc_ptr, ...)  ──►  
                                    4. Read descriptor from memory
                                       let buf = read_guest_memory_typed::<Buffer>(desc_ptr)?;
                                       // buf.ptr = data_ptr, buf.len = 5
                                    
                                    5. Validate length
                                       if buf.len() > limits.max_storage_key_size { error }
                                    
                                    6. Read actual data
                                       let data = read_guest_memory_slice(&buf)?;
                                       // data = b"hello"
```

### Writing Data to Guest

For returning data, the runtime uses **registers** (host-side storage slots):

```
Guest Code                          Host (VMHostFunctions)
─────────────────────────────────────────────────────────────────────────
1. Call host function to            
   populate register                
   storage_read(key_ptr, reg_id) ──►  
                                    2. Read key, lookup value
                                       let value = storage.get(&key);
                                    
                                    3. Store in register
                                       registers.set(limits, reg_id, value)?;
                                    
                                    4. Return status
                                 ◄── returns 1 (found) or 0 (not found)
                                    
5. Check register length            
   let len = register_len(reg_id);  
                                    
6. Allocate output buffer           
   let out_ptr = allocate(len);     
   let desc_ptr = allocate(16);     
   memory[desc_ptr..] = BufferMut { 
       ptr: out_ptr,                
       len: len                     
   };                               
                                    
7. Read register to memory          
   read_register(reg_id, desc_ptr)──►
                                    8. Read descriptor
                                       let buf = read_guest_memory_typed::<BufferMut>(desc_ptr)?;
                                    
                                    9. Copy register data to guest memory
                                       memory[buf.ptr..].copy_from_slice(&register_data);
                                    
                                 ◄── returns 1 (success)
                                    
10. Use data at out_ptr             
```

## Compound Types

### sys::Event

```rust
#[repr(C)]
pub struct Event<'a> {
    kind: Buffer<'a>,  // Event type string
    data: Buffer<'a>,  // Event payload
}
```

Layout in memory (32 bytes):
```
offset 0:  kind.ptr  (u64)
offset 8:  kind.len  (u64)
offset 16: data.ptr  (u64)
offset 24: data.len  (u64)
```

### sys::Location

```rust
#[repr(C)]
pub struct Location<'a> {
    file: Buffer<'a>,  // File name
    line: u32,
    column: u32,
}
```

Layout in memory (24 bytes):
```
offset 0:  file.ptr  (u64)
offset 8:  file.len  (u64)
offset 16: line      (u32)
offset 20: column    (u32)
```

### sys::ValueReturn

```rust
#[repr(C, u64)]
pub enum ValueReturn<'a> {
    Ok(Buffer<'a>),   // discriminant = 0
    Err(Buffer<'a>),  // discriminant = 1
}
```

Layout in memory (24 bytes):
```
offset 0: discriminant (u64) - 0 for Ok, 1 for Err
offset 8: buffer.ptr   (u64)
offset 16: buffer.len  (u64)
```

### sys::XCall

```rust
#[repr(C)]
pub struct XCall<'a> {
    context_id: Buffer<'a>,  // 32-byte context ID
    function: Buffer<'a>,    // Function name
    params: Buffer<'a>,      // Parameters
}
```

Layout in memory (48 bytes):
```
offset 0:  context_id.ptr  (u64)
offset 8:  context_id.len  (u64)
offset 16: function.ptr    (u64)
offset 24: function.len    (u64)
offset 32: params.ptr      (u64)
offset 40: params.len      (u64)
```

## Memory Access Implementation

### In VMHostFunctions

```rust
impl VMHostFunctions<'_> {
    /// Read a typed struct from guest memory at the given offset
    pub unsafe fn read_guest_memory_typed<T>(&self, offset: u64) -> VMLogicResult<T> {
        let size = std::mem::size_of::<T>();
        let mut bytes = vec![0u8; size];
        self.borrow_memory()
            .read(offset, &mut bytes)
            .map_err(|_| HostError::InvalidMemoryAccess)?;
        Ok(std::ptr::read_unaligned(bytes.as_ptr() as *const T))
    }
    
    /// Read bytes from a buffer descriptor
    pub fn read_guest_memory_slice<'a>(&self, buf: &sys::Buffer<'a>) -> VMLogicResult<&'a [u8]> {
        let ptr = buf.ptr().as_u64();
        let len = buf.len() as usize;
        // Returns a view into WASM memory
        self.borrow_memory()
            .view_slice(ptr, len)
            .map_err(|_| HostError::InvalidMemoryAccess)
    }
    
    /// Read exactly N bytes from a buffer (for fixed-size data like hashes)
    pub fn read_guest_memory_sized<const N: usize>(
        &self, 
        buf: &sys::Buffer<'_>
    ) -> VMLogicResult<&[u8; N]> {
        if buf.len() != N as u64 {
            return Err(HostError::InvalidMemoryAccess.into());
        }
        let slice = self.read_guest_memory_slice(buf)?;
        Ok(slice.try_into().expect("length checked"))
    }
    
    /// Read UTF-8 string from buffer
    pub fn read_guest_memory_str<'a>(&self, buf: &sys::Buffer<'a>) -> VMLogicResult<&'a str> {
        let bytes = self.read_guest_memory_slice(buf)?;
        std::str::from_utf8(bytes).map_err(|_| HostError::BadUTF8.into())
    }
    
    /// Get mutable slice for writing to guest memory
    pub fn read_guest_memory_slice_mut<'a>(
        &self, 
        buf: &sys::BufferMut<'a>
    ) -> VMLogicResult<&'a mut [u8]> {
        // Similar to read, but returns mutable reference
    }
}
```

## Memory Limits

The runtime enforces strict limits on memory operations:

| Limit | Purpose | Default |
|-------|---------|---------|
| `max_memory_pages` | Total WASM memory (pages × 64KB) | 1024 pages (64MB) |
| `max_storage_key_size` | Maximum key length | 1MB |
| `max_storage_value_size` | Maximum value length | 10MB |
| `max_register_size` | Maximum data in single register | 100MB |
| `max_registers` | Number of available registers | 100 |
| `max_log_size` | Maximum log message length | 16KB |
| `max_event_kind_size` | Maximum event type string | 100 bytes |
| `max_event_data_size` | Maximum event payload | 16KB |

## Safety Considerations

1. **All pointer access is bounds-checked** via Wasmer's memory API
2. **`unsafe` blocks are minimal** - only for interpreting raw bytes as typed structs
3. **Buffer lengths validated before access** - prevents reading beyond allocated memory
4. **UTF-8 validation on string reads** - malformed strings return `HostError::BadUTF8`
5. **Register limits prevent memory exhaustion** - total registers and per-register size are bounded

## Example: Complete storage_read Flow

```rust
// Host function implementation (storage.rs)
pub fn storage_read(&mut self, src_key_ptr: u64, dest_register_id: u64) -> VMLogicResult<u32> {
    // 1. Read the buffer descriptor for the key
    let key_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_key_ptr)? };
    
    // 2. Check key length against limits
    let logic = self.borrow_logic();
    if key_buf.len() > logic.limits.max_storage_key_size.get() {
        return Err(HostError::KeyLengthOverflow.into());
    }
    
    // 3. Read actual key bytes from guest memory
    let key = self.read_guest_memory_slice(&key_buf)?.to_vec();
    
    // 4. Look up in storage
    if let Some(value) = logic.storage.get(&key) {
        // 5. Store result in register (host-side)
        self.with_logic_mut(|logic| {
            logic.registers.set(logic.limits, dest_register_id, value)
        })?;
        return Ok(1);  // Found
    }
    
    Ok(0)  // Not found
}
```

## Serialization Formats

| Data Type | Serialization |
|-----------|---------------|
| Primitive types | Little-endian, native layout |
| Strings | UTF-8 bytes (no null terminator) |
| Collections | Borsh encoding |
| Context IDs, Public Keys | Raw 32 bytes |
| Timestamps | `u64` nanoseconds since Unix epoch |
| Signatures | 64 bytes (Ed25519) |
