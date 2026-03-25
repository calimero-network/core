use crate::panic_payload::panic_payload_to_string;
use crate::{
    errors::HostError,
    logic::{sys, VMHostFunctions, VMLogicResult},
};
use calimero_storage::{
    address::Id,
    env::{with_runtime_env, RuntimeEnv},
    interface::Interface,
    js::{
        JsCollection, JsCounter, JsFrozenStorage, JsLwwRegister, JsPnCounter, JsRga,
        JsUnorderedMap, JsUnorderedSet, JsUserStorage, JsVector,
    },
    store::MainStorage,
};
use std::{
    convert::TryFrom,
    fmt::Display,
    panic::{self, AssertUnwindSafe},
};

use super::system::build_runtime_env;

const COLLECTION_ID_LEN: usize = 32;

// ---------------------------------------------------------------------------
// Macros for the `new` and `new_with_id` host function patterns
// ---------------------------------------------------------------------------

/// Generates a `crdt_X_new` private method that creates a new collection,
/// saves it, and writes the id to a register.
macro_rules! impl_crdt_new {
    ($fn_name:ident, $ty:ty) => {
        fn $fn_name(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
            let outcome = panic::catch_unwind(AssertUnwindSafe(|| -> Result<$ty, String> {
                let mut instance = <$ty as JsCollection>::collection_new();
                instance.js_save()?;
                Ok(instance)
            }));

            match outcome {
                Ok(Ok(instance)) => {
                    self.write_register_bytes(
                        dest_register_id,
                        instance.collection_id().as_bytes(),
                    )?;
                    Ok(0)
                }
                Ok(Err(err)) => self.write_error_message(dest_register_id, err),
                Err(payload) => self.write_error_message(
                    dest_register_id,
                    panic_payload_to_string(payload.as_ref(), "unknown panic"),
                ),
            }
        }
    };
}

/// Generates a `crdt_X_new_with_id` private method that loads-or-creates a
/// collection by id and writes the id to a register.
///
/// Unlike the old code that unconditionally created a new instance,
/// this delegates to `JsCollection::js_load` which returns the existing
/// instance if present (Phase 3.1 fix).
macro_rules! impl_crdt_new_with_id {
    ($fn_name:ident, $ty:ty) => {
        fn $fn_name(&mut self, id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
            let id = match self.read_map_id(id_ptr)? {
                Ok(id) => id,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            let outcome = panic::catch_unwind(AssertUnwindSafe(|| -> Result<$ty, String> {
                <$ty as JsCollection>::js_load(id)
            }));

            match outcome {
                Ok(Ok(instance)) => {
                    self.write_register_bytes(
                        dest_register_id,
                        instance.collection_id().as_bytes(),
                    )?;
                    Ok(0)
                }
                Ok(Err(err)) => self.write_error_message(dest_register_id, err),
                Err(payload) => self.write_error_message(
                    dest_register_id,
                    panic_payload_to_string(payload.as_ref(), "unknown panic"),
                ),
            }
        }
    };
}

// ---------------------------------------------------------------------------
// VMHostFunctions implementation
// ---------------------------------------------------------------------------

impl VMHostFunctions<'_> {
    fn make_runtime_env(&mut self) -> VMLogicResult<RuntimeEnv> {
        self.with_logic_mut(|logic| {
            Ok(build_runtime_env(
                logic.storage,
                logic.context.context_id,
                logic.context.executor_public_key,
            ))
        })
    }

    fn invoke_with_storage_env<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> VMLogicResult<T>,
    ) -> VMLogicResult<T> {
        let env = self.make_runtime_env()?;
        with_runtime_env(env, || f(self))
    }

    // ── Map public API ────────────────────────────────────────────────

    pub fn js_crdt_map_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_map_new(dest_register_id))
    }

    pub fn js_crdt_map_get(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_map_get(map_id_ptr, key_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_map_insert(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_map_insert(map_id_ptr, key_ptr, value_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_map_remove(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_map_remove(map_id_ptr, key_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_map_contains(&mut self, map_id_ptr: u64, key_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_map_contains(map_id_ptr, key_ptr))
    }

    pub fn js_crdt_map_iter(
        &mut self,
        map_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_map_iter(map_id_ptr, dest_register_id))
    }

    // ── Vector public API ─────────────────────────────────────────────

    pub fn js_crdt_vector_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_new(dest_register_id))
    }

    pub fn js_crdt_vector_len(
        &mut self,
        vector_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_len(vector_id_ptr, dest_register_id))
    }

    pub fn js_crdt_vector_push(
        &mut self,
        vector_id_ptr: u64,
        value_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_push(vector_id_ptr, value_ptr))
    }

    pub fn js_crdt_vector_get(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_vector_get(vector_id_ptr, index, dest_register_id)
        })
    }

    pub fn js_crdt_vector_pop(
        &mut self,
        vector_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_pop(vector_id_ptr, dest_register_id))
    }

    // ── Set public API ────────────────────────────────────────────────

    pub fn js_crdt_set_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_new(dest_register_id))
    }

    pub fn js_crdt_set_insert(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_insert(set_id_ptr, value_ptr))
    }

    pub fn js_crdt_set_contains(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_contains(set_id_ptr, value_ptr))
    }

    pub fn js_crdt_set_remove(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_remove(set_id_ptr, value_ptr))
    }

    pub fn js_crdt_set_len(
        &mut self,
        set_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_len(set_id_ptr, dest_register_id))
    }

    pub fn js_crdt_set_iter(
        &mut self,
        set_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_iter(set_id_ptr, dest_register_id))
    }

    pub fn js_crdt_set_clear(&mut self, set_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_clear(set_id_ptr))
    }

    // ── LWW Register public API ───────────────────────────────────────

    pub fn js_crdt_lww_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_lww_new(dest_register_id))
    }

    pub fn js_crdt_lww_set(
        &mut self,
        register_id_ptr: u64,
        value_ptr: u64,
        has_value: u32,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_lww_set(register_id_ptr, value_ptr, has_value)
        })
    }

    pub fn js_crdt_lww_get(
        &mut self,
        register_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_lww_get(register_id_ptr, dest_register_id))
    }

    pub fn js_crdt_lww_timestamp(
        &mut self,
        register_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_lww_timestamp(register_id_ptr, dest_register_id)
        })
    }

    // ── GCounter public API ───────────────────────────────────────────

    pub fn js_crdt_counter_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_counter_new(dest_register_id))
    }

    pub fn js_crdt_counter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_counter_increment(counter_id_ptr))
    }

    pub fn js_crdt_counter_value(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_counter_value(counter_id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_counter_get_executor_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_counter_get_executor_count(
                counter_id_ptr,
                executor_ptr,
                has_executor,
                dest_register_id,
            )
        })
    }

    // ── new_with_id public API ────────────────────────────────────────

    pub fn js_crdt_map_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_map_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_vector_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_set_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_lww_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_lww_new_with_id(id_ptr, dest_register_id))
    }

    // ── GCounter aliases (Phase 3.2: delegate through public methods) ─

    pub fn js_crdt_g_counter_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.js_crdt_counter_new(dest_register_id)
    }

    pub fn js_crdt_g_counter_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_counter_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_g_counter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        self.js_crdt_counter_increment(counter_id_ptr)
    }

    pub fn js_crdt_g_counter_value(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.js_crdt_counter_value(counter_id_ptr, dest_register_id)
    }

    pub fn js_crdt_g_counter_get_executor_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.js_crdt_counter_get_executor_count(
            counter_id_ptr,
            executor_ptr,
            has_executor,
            dest_register_id,
        )
    }

    pub fn js_crdt_g_counter_serialize(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_counter_serialize(counter_id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_g_counter_deserialize(
        &mut self,
        data_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_counter_deserialize(data_ptr, dest_register_id)
        })
    }

    // ── PNCounter public API ──────────────────────────────────────────

    pub fn js_crdt_pn_counter_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_pn_counter_new(dest_register_id))
    }

    pub fn js_crdt_pn_counter_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_pn_counter_new_with_id(id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_pn_counter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_pn_counter_increment(counter_id_ptr))
    }

    pub fn js_crdt_pn_counter_decrement(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_pn_counter_decrement(counter_id_ptr))
    }

    pub fn js_crdt_pn_counter_value(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_pn_counter_value(counter_id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_pn_counter_get_positive_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_pn_counter_get_positive_count(
                counter_id_ptr,
                executor_ptr,
                has_executor,
                dest_register_id,
            )
        })
    }

    pub fn js_crdt_pn_counter_get_negative_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_pn_counter_get_negative_count(
                counter_id_ptr,
                executor_ptr,
                has_executor,
                dest_register_id,
            )
        })
    }

    pub fn js_crdt_pn_counter_serialize(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_pn_counter_serialize(counter_id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_pn_counter_deserialize(
        &mut self,
        data_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_pn_counter_deserialize(data_ptr, dest_register_id)
        })
    }

    // ── RGA public API ────────────────────────────────────────────────

    pub fn js_crdt_rga_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_new(dest_register_id))
    }

    pub fn js_crdt_rga_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_rga_insert(
        &mut self,
        rga_id_ptr: u64,
        pos: u64,
        text_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_insert(rga_id_ptr, pos, text_ptr))
    }

    pub fn js_crdt_rga_delete(&mut self, rga_id_ptr: u64, pos: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_delete(rga_id_ptr, pos))
    }

    pub fn js_crdt_rga_get_text(
        &mut self,
        rga_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_get_text(rga_id_ptr, dest_register_id))
    }

    pub fn js_crdt_rga_len(
        &mut self,
        rga_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_len(rga_id_ptr, dest_register_id))
    }

    pub fn js_crdt_rga_serialize(
        &mut self,
        rga_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_serialize(rga_id_ptr, dest_register_id))
    }

    pub fn js_crdt_rga_deserialize(
        &mut self,
        data_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_deserialize(data_ptr, dest_register_id))
    }

    // ── UserStorage public API ────────────────────────────────────────

    pub fn js_user_storage_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.user_storage_new(dest_register_id))
    }

    pub fn js_user_storage_insert(
        &mut self,
        storage_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.user_storage_insert(storage_id_ptr, value_ptr, dest_register_id)
        })
    }

    pub fn js_user_storage_get(
        &mut self,
        storage_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.user_storage_get(storage_id_ptr, dest_register_id))
    }

    pub fn js_user_storage_get_for_user(
        &mut self,
        storage_id_ptr: u64,
        user_key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.user_storage_get_for_user(storage_id_ptr, user_key_ptr, dest_register_id)
        })
    }

    pub fn js_user_storage_remove(
        &mut self,
        storage_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.user_storage_remove(storage_id_ptr, dest_register_id)
        })
    }

    pub fn js_user_storage_contains(&mut self, storage_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.user_storage_contains(storage_id_ptr))
    }

    pub fn js_user_storage_contains_user(
        &mut self,
        storage_id_ptr: u64,
        user_key_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.user_storage_contains_user(storage_id_ptr, user_key_ptr)
        })
    }

    // ── FrozenStorage public API ──────────────────────────────────────

    pub fn js_frozen_storage_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.frozen_storage_new(dest_register_id))
    }

    pub fn js_frozen_storage_add(
        &mut self,
        storage_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.frozen_storage_add(storage_id_ptr, value_ptr, dest_register_id)
        })
    }

    pub fn js_frozen_storage_get(
        &mut self,
        storage_id_ptr: u64,
        hash_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.frozen_storage_get(storage_id_ptr, hash_ptr, dest_register_id)
        })
    }

    pub fn js_frozen_storage_contains(
        &mut self,
        storage_id_ptr: u64,
        hash_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.frozen_storage_contains(storage_id_ptr, hash_ptr))
    }

    // ===================================================================
    // Private implementations — `new` and `new_with_id` via macros
    // ===================================================================

    impl_crdt_new!(crdt_map_new, JsUnorderedMap);
    impl_crdt_new!(crdt_vector_new, JsVector);
    impl_crdt_new!(crdt_set_new, JsUnorderedSet);
    impl_crdt_new!(crdt_lww_new, JsLwwRegister);
    impl_crdt_new!(crdt_counter_new, JsCounter);
    impl_crdt_new!(crdt_pn_counter_new, JsPnCounter);
    impl_crdt_new!(crdt_rga_new, JsRga);
    impl_crdt_new!(user_storage_new, JsUserStorage);
    impl_crdt_new!(frozen_storage_new, JsFrozenStorage);

    impl_crdt_new_with_id!(crdt_map_new_with_id, JsUnorderedMap);
    impl_crdt_new_with_id!(crdt_vector_new_with_id, JsVector);
    impl_crdt_new_with_id!(crdt_set_new_with_id, JsUnorderedSet);
    impl_crdt_new_with_id!(crdt_lww_new_with_id, JsLwwRegister);
    impl_crdt_new_with_id!(crdt_counter_new_with_id, JsCounter);
    impl_crdt_new_with_id!(crdt_pn_counter_new_with_id, JsPnCounter);
    impl_crdt_new_with_id!(crdt_rga_new_with_id, JsRga);

    // ===================================================================
    // Private implementations — Map operations
    // ===================================================================

    fn crdt_map_get(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let key = self.read_buffer(key_ptr)?;
        let map = match JsUnorderedMap::js_load(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match map.get(&key) {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_map_insert(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let key = self.read_buffer(key_ptr)?;
        let value = self.read_buffer(value_ptr)?;
        let mut map = match JsUnorderedMap::js_load(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match map.insert(&key, &value) {
            Ok(previous) => {
                if let Err(message) = map.js_save() {
                    return self.write_error_message(dest_register_id, message);
                }
                if let Some(prev) = previous {
                    self.write_register_bytes(dest_register_id, &prev)?;
                    Ok(1)
                } else {
                    self.clear_register(dest_register_id)?;
                    Ok(0)
                }
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_map_remove(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let key = self.read_buffer(key_ptr)?;
        let mut map = match JsUnorderedMap::js_load(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match map.remove(&key) {
            Ok(Some(previous)) => {
                if let Err(message) = map.js_save() {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &previous)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_map_contains(&mut self, map_id_ptr: u64, key_ptr: u64) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let key = self.read_buffer(key_ptr)?;
        let map = match JsUnorderedMap::js_load(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(0, message),
        };
        match map.contains(&key) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_map_iter(&mut self, map_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let map = match JsUnorderedMap::js_load(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let entries = match map.entries() {
            Ok(entries) => entries,
            Err(err) => return self.write_error_message(dest_register_id, err),
        };
        let count = u32::try_from(entries.len()).map_err(|_| HostError::IntegerOverflow)?;
        let mut total_len: usize = 4;
        for (key, value) in &entries {
            u32::try_from(key.len()).map_err(|_| HostError::IntegerOverflow)?;
            u32::try_from(value.len()).map_err(|_| HostError::IntegerOverflow)?;
            total_len = total_len
                .checked_add(4 + key.len() + 4 + value.len())
                .ok_or(HostError::IntegerOverflow)?;
        }
        let mut buffer = Vec::with_capacity(total_len);
        buffer.extend_from_slice(&count.to_le_bytes());
        for (key, value) in entries {
            let key_len = u32::try_from(key.len()).map_err(|_| HostError::IntegerOverflow)?;
            let value_len = u32::try_from(value.len()).map_err(|_| HostError::IntegerOverflow)?;
            buffer.extend_from_slice(&key_len.to_le_bytes());
            buffer.extend_from_slice(&key);
            buffer.extend_from_slice(&value_len.to_le_bytes());
            buffer.extend_from_slice(&value);
        }
        self.write_register_bytes(dest_register_id, &buffer)?;
        Ok(1)
    }

    // ===================================================================
    // Private implementations — Vector operations
    // ===================================================================

    fn crdt_vector_len(&mut self, vector_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let vector = match JsVector::js_load(vector_id) {
            Ok(v) => v,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match vector.len() {
            Ok(len) => {
                let len_u64 = u64::try_from(len).map_err(|_| HostError::IntegerOverflow)?;
                self.write_register_bytes(dest_register_id, &len_u64.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_vector_push(&mut self, vector_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let value = self.read_buffer(value_ptr)?;
        let mut vector = match JsVector::js_load(vector_id) {
            Ok(v) => v,
            Err(message) => return self.write_error_message(0, message),
        };
        match vector.push(&value) {
            Ok(()) => match vector.js_save() {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_vector_get(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let idx = match usize::try_from(index) {
            Ok(value) => value,
            Err(_) => {
                return self.write_error_message(
                    dest_register_id,
                    format!("index {index} does not fit into usize"),
                )
            }
        };
        let vector = match JsVector::js_load(vector_id) {
            Ok(v) => v,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match vector.get(idx) {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_vector_pop(&mut self, vector_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let mut vector = match JsVector::js_load(vector_id) {
            Ok(v) => v,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match vector.pop() {
            Ok(Some(value)) => {
                if let Err(message) = vector.js_save() {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    // ===================================================================
    // Private implementations — Set operations
    // ===================================================================

    fn crdt_set_insert(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let value = self.read_buffer(value_ptr)?;
        let mut set = match JsUnorderedSet::js_load(set_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(0, message),
        };
        match set.insert(&value) {
            Ok(inserted) => {
                if !inserted {
                    return Ok(0);
                }
                if let Err(message) = set.js_save() {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_set_contains(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let value = self.read_buffer(value_ptr)?;
        let set = match JsUnorderedSet::js_load(set_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(0, message),
        };
        match set.contains(&value) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_set_remove(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let value = self.read_buffer(value_ptr)?;
        let mut set = match JsUnorderedSet::js_load(set_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(0, message),
        };
        match set.remove(&value) {
            Ok(removed) => {
                if !removed {
                    return Ok(0);
                }
                if let Err(message) = set.js_save() {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_set_len(&mut self, set_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let set = match JsUnorderedSet::js_load(set_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match set.len() {
            Ok(len) => {
                let len_u64 = u64::try_from(len).map_err(|_| HostError::IntegerOverflow)?;
                self.write_register_bytes(dest_register_id, &len_u64.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_set_iter(&mut self, set_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let set = match JsUnorderedSet::js_load(set_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let values = match set.values() {
            Ok(v) => v,
            Err(err) => return self.write_error_message(dest_register_id, err),
        };
        let count = u32::try_from(values.len()).map_err(|_| HostError::IntegerOverflow)?;
        let mut total_len: usize = 4;
        for value in &values {
            u32::try_from(value.len()).map_err(|_| HostError::IntegerOverflow)?;
            total_len = total_len
                .checked_add(4 + value.len())
                .ok_or(HostError::IntegerOverflow)?;
        }
        let mut buffer = Vec::with_capacity(total_len);
        buffer.extend_from_slice(&count.to_le_bytes());
        for value in values {
            let value_len = u32::try_from(value.len()).map_err(|_| HostError::IntegerOverflow)?;
            buffer.extend_from_slice(&value_len.to_le_bytes());
            buffer.extend_from_slice(&value);
        }
        self.write_register_bytes(dest_register_id, &buffer)?;
        Ok(1)
    }

    fn crdt_set_clear(&mut self, set_id_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let mut set = match JsUnorderedSet::js_load(set_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(0, message),
        };
        let len_before = match set.len() {
            Ok(len) => len,
            Err(err) => return self.write_error_message(0, err),
        };
        match set.clear() {
            Ok(()) => {
                if len_before == 0 {
                    return Ok(0);
                }
                if let Err(message) = set.js_save() {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    // ===================================================================
    // Private implementations — LWW Register operations
    // ===================================================================

    fn crdt_lww_set(
        &mut self,
        register_id_ptr: u64,
        value_ptr: u64,
        has_value: u32,
    ) -> VMLogicResult<i32> {
        let register_id = match self.read_map_id(register_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let value = if has_value != 0 {
            Some(self.read_buffer(value_ptr)?)
        } else {
            None
        };
        let mut register = match JsLwwRegister::js_load(register_id) {
            Ok(r) => r,
            Err(message) => return self.write_error_message(0, message),
        };
        let previous_value = register.get();
        let values_equal = previous_value.as_deref() == value.as_deref();
        register.set(value.as_deref());
        match register.js_save() {
            Ok(()) => Ok(i32::from(!values_equal)),
            Err(message) => self.write_error_message(0, message),
        }
    }

    fn crdt_lww_get(&mut self, register_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let register_id = match self.read_map_id(register_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let register = match JsLwwRegister::js_load(register_id) {
            Ok(r) => r,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match register.get() {
            Some(value) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            None => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
        }
    }

    fn crdt_lww_timestamp(
        &mut self,
        register_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let register_id = match self.read_map_id(register_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let register = match JsLwwRegister::js_load(register_id) {
            Ok(r) => r,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        if register.get().is_none() {
            self.clear_register(dest_register_id)?;
            return Ok(0);
        }
        let timestamp = register.timestamp();
        let time_le = timestamp.get_time().as_u64().to_le_bytes();
        let node_id: u128 = (*timestamp.get_id()).into();
        let mut buffer = [0u8; 24];
        buffer[..8].copy_from_slice(&time_le);
        buffer[8..].copy_from_slice(&node_id.to_le_bytes());
        self.write_register_bytes(dest_register_id, &buffer)?;
        Ok(1)
    }

    // ===================================================================
    // Private implementations — GCounter operations
    // ===================================================================

    fn crdt_counter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let mut counter = match JsCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(0, message),
        };
        match counter.increment() {
            Ok(()) => match counter.js_save() {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_counter_value(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let counter = match JsCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match counter.value() {
            Ok(value) => {
                self.write_register_bytes(dest_register_id, &value.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_counter_get_executor_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let executor_bytes: [u8; 32] = if has_executor != 0 {
            let bytes = self.read_buffer(executor_ptr)?;
            match <[u8; 32]>::try_from(bytes.as_slice()) {
                Ok(array) => array,
                Err(_) => {
                    return self.write_error_message(
                        dest_register_id,
                        "executor id must be exactly 32 bytes",
                    )
                }
            }
        } else {
            self.borrow_logic().context.executor_public_key
        };
        let counter = match JsCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match counter.get_executor_count(&executor_bytes) {
            Ok(value) => {
                self.write_register_bytes(dest_register_id, &value.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_counter_serialize(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let counter = match JsCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match borsh::to_vec(&counter) {
            Ok(bytes) => {
                self.write_register_bytes(dest_register_id, &bytes)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_counter_deserialize(
        &mut self,
        data_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let data = self.read_buffer(data_ptr)?;
        let mut counter: JsCounter = match borsh::from_slice(&data) {
            Ok(c) => c,
            Err(err) => return self.write_error_message(dest_register_id, err),
        };
        match counter.js_save() {
            Ok(()) => {
                self.write_register_bytes(dest_register_id, counter.id().as_bytes())?;
                Ok(0)
            }
            Err(message) => self.write_error_message(dest_register_id, message),
        }
    }

    // ===================================================================
    // Private implementations — PNCounter operations
    // ===================================================================

    fn crdt_pn_counter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let mut counter = match JsPnCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(0, message),
        };
        match counter.increment() {
            Ok(()) => match counter.js_save() {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_pn_counter_decrement(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let mut counter = match JsPnCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(0, message),
        };
        match counter.decrement() {
            Ok(()) => match counter.js_save() {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_pn_counter_value(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let counter = match JsPnCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match counter.value() {
            Ok(value) => {
                self.write_register_bytes(dest_register_id, &value.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_pn_counter_get_positive_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let executor_bytes: [u8; 32] = if has_executor != 0 {
            let bytes = self.read_buffer(executor_ptr)?;
            match <[u8; 32]>::try_from(bytes.as_slice()) {
                Ok(array) => array,
                Err(_) => {
                    return self.write_error_message(
                        dest_register_id,
                        "executor id must be exactly 32 bytes",
                    )
                }
            }
        } else {
            self.borrow_logic().context.executor_public_key
        };
        let counter = match JsPnCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match counter.get_positive_count(&executor_bytes) {
            Ok(value) => {
                self.write_register_bytes(dest_register_id, &value.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_pn_counter_get_negative_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let executor_bytes: [u8; 32] = if has_executor != 0 {
            let bytes = self.read_buffer(executor_ptr)?;
            match <[u8; 32]>::try_from(bytes.as_slice()) {
                Ok(array) => array,
                Err(_) => {
                    return self.write_error_message(
                        dest_register_id,
                        "executor id must be exactly 32 bytes",
                    )
                }
            }
        } else {
            self.borrow_logic().context.executor_public_key
        };
        let counter = match JsPnCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match counter.get_negative_count(&executor_bytes) {
            Ok(value) => {
                self.write_register_bytes(dest_register_id, &value.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_pn_counter_serialize(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let counter = match JsPnCounter::js_load(counter_id) {
            Ok(c) => c,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match borsh::to_vec(&counter) {
            Ok(bytes) => {
                self.write_register_bytes(dest_register_id, &bytes)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_pn_counter_deserialize(
        &mut self,
        data_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let data = self.read_buffer(data_ptr)?;
        let mut counter: JsPnCounter = match borsh::from_slice(&data) {
            Ok(c) => c,
            Err(err) => return self.write_error_message(dest_register_id, err),
        };
        match counter.js_save() {
            Ok(()) => {
                self.write_register_bytes(dest_register_id, counter.id().as_bytes())?;
                Ok(0)
            }
            Err(message) => self.write_error_message(dest_register_id, message),
        }
    }

    // ===================================================================
    // Private implementations — RGA operations
    // ===================================================================

    fn crdt_rga_insert(&mut self, rga_id_ptr: u64, pos: u64, text_ptr: u64) -> VMLogicResult<i32> {
        let rga_id = match self.read_map_id(rga_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let idx = match usize::try_from(pos) {
            Ok(v) => v,
            Err(_) => {
                return self
                    .write_error_message(0, format!("position {pos} does not fit into usize"))
            }
        };
        let text_bytes = self.read_buffer(text_ptr)?;
        let text = match std::str::from_utf8(&text_bytes) {
            Ok(s) => s,
            Err(err) => return self.write_error_message(0, err),
        };
        let mut rga = match JsRga::js_load(rga_id) {
            Ok(r) => r,
            Err(message) => return self.write_error_message(0, message),
        };
        match rga.insert(idx, text) {
            Ok(()) => match rga.js_save() {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_rga_delete(&mut self, rga_id_ptr: u64, pos: u64) -> VMLogicResult<i32> {
        let rga_id = match self.read_map_id(rga_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let idx = match usize::try_from(pos) {
            Ok(v) => v,
            Err(_) => {
                return self
                    .write_error_message(0, format!("position {pos} does not fit into usize"))
            }
        };
        let mut rga = match JsRga::js_load(rga_id) {
            Ok(r) => r,
            Err(message) => return self.write_error_message(0, message),
        };
        match rga.delete(idx) {
            Ok(()) => match rga.js_save() {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_rga_get_text(&mut self, rga_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let rga_id = match self.read_map_id(rga_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let rga = match JsRga::js_load(rga_id) {
            Ok(r) => r,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match rga.get_text() {
            Ok(text) => {
                self.write_register_bytes(dest_register_id, text.as_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_rga_len(&mut self, rga_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let rga_id = match self.read_map_id(rga_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let rga = match JsRga::js_load(rga_id) {
            Ok(r) => r,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match rga.len() {
            Ok(len) => {
                let len_u64 = u64::try_from(len).map_err(|_| HostError::IntegerOverflow)?;
                self.write_register_bytes(dest_register_id, &len_u64.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_rga_serialize(&mut self, rga_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let rga_id = match self.read_map_id(rga_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let rga = match JsRga::js_load(rga_id) {
            Ok(r) => r,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match borsh::to_vec(&rga) {
            Ok(bytes) => {
                self.write_register_bytes(dest_register_id, &bytes)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_rga_deserialize(&mut self, data_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let data = self.read_buffer(data_ptr)?;
        let mut rga: JsRga = match borsh::from_slice(&data) {
            Ok(r) => r,
            Err(err) => return self.write_error_message(dest_register_id, err),
        };
        match rga.js_save() {
            Ok(()) => {
                self.write_register_bytes(dest_register_id, rga.id().as_bytes())?;
                Ok(0)
            }
            Err(message) => self.write_error_message(dest_register_id, message),
        }
    }

    // ===================================================================
    // Private implementations — UserStorage operations
    // ===================================================================

    fn user_storage_insert(
        &mut self,
        storage_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let value = self.read_buffer(value_ptr)?;
        let mut storage = match JsUserStorage::js_load(storage_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match storage.insert(&value) {
            Ok(previous) => {
                if let Err(message) = storage.js_save() {
                    return self.write_error_message(dest_register_id, message);
                }
                if let Some(prev) = previous {
                    self.write_register_bytes(dest_register_id, &prev)?;
                    Ok(1)
                } else {
                    self.clear_register(dest_register_id)?;
                    Ok(0)
                }
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn user_storage_get(
        &mut self,
        storage_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let storage = match JsUserStorage::js_load(storage_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match storage.get() {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn user_storage_get_for_user(
        &mut self,
        storage_id_ptr: u64,
        user_key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let user_key_bytes = self.read_buffer(user_key_ptr)?;
        let user_key: [u8; 32] = match <[u8; 32]>::try_from(user_key_bytes.as_slice()) {
            Ok(array) => array,
            Err(_) => {
                return self
                    .write_error_message(dest_register_id, "user key must be exactly 32 bytes")
            }
        };
        let storage = match JsUserStorage::js_load(storage_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match storage.get_for_user(&user_key) {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn user_storage_remove(
        &mut self,
        storage_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let mut storage = match JsUserStorage::js_load(storage_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match storage.remove() {
            Ok(Some(previous)) => {
                if let Err(message) = storage.js_save() {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &previous)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn user_storage_contains(&mut self, storage_id_ptr: u64) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let storage = match JsUserStorage::js_load(storage_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(0, message),
        };
        match storage.contains_current_user() {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn user_storage_contains_user(
        &mut self,
        storage_id_ptr: u64,
        user_key_ptr: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let user_key_bytes = self.read_buffer(user_key_ptr)?;
        let user_key: [u8; 32] = match <[u8; 32]>::try_from(user_key_bytes.as_slice()) {
            Ok(array) => array,
            Err(_) => return self.write_error_message(0, "user key must be exactly 32 bytes"),
        };
        let storage = match JsUserStorage::js_load(storage_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(0, message),
        };
        match storage.contains_user(&user_key) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    // ===================================================================
    // Private implementations — FrozenStorage operations
    // ===================================================================

    fn frozen_storage_add(
        &mut self,
        storage_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let value = self.read_buffer(value_ptr)?;
        let mut storage = match JsFrozenStorage::js_load(storage_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match storage.insert(&value) {
            Ok(hash) => {
                if let Err(message) = storage.js_save() {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &hash)?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn frozen_storage_get(
        &mut self,
        storage_id_ptr: u64,
        hash_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let hash_bytes = self.read_buffer(hash_ptr)?;
        let hash: [u8; 32] = match <[u8; 32]>::try_from(hash_bytes.as_slice()) {
            Ok(array) => array,
            Err(_) => {
                return self.write_error_message(dest_register_id, "hash must be exactly 32 bytes")
            }
        };
        let storage = match JsFrozenStorage::js_load(storage_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        match storage.get(&hash) {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn frozen_storage_contains(
        &mut self,
        storage_id_ptr: u64,
        hash_ptr: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };
        let hash_bytes = self.read_buffer(hash_ptr)?;
        let hash: [u8; 32] = match <[u8; 32]>::try_from(hash_bytes.as_slice()) {
            Ok(array) => array,
            Err(_) => return self.write_error_message(0, "hash must be exactly 32 bytes"),
        };
        let storage = match JsFrozenStorage::js_load(storage_id) {
            Ok(s) => s,
            Err(message) => return self.write_error_message(0, message),
        };
        match storage.contains(&hash) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    // ===================================================================
    // Utility methods
    // ===================================================================

    fn read_map_id(&mut self, map_id_ptr: u64) -> VMLogicResult<Result<Id, String>> {
        let buffer = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(map_id_ptr)? };
        let data = self.read_guest_memory_slice(&buffer)?;
        if data.len() != COLLECTION_ID_LEN {
            return Ok(Err(format!(
                "mapId must be exactly {} bytes (received {})",
                COLLECTION_ID_LEN,
                data.len()
            )));
        }
        let mut bytes = [0u8; COLLECTION_ID_LEN];
        bytes.copy_from_slice(data);
        Ok(Ok(Id::new(bytes)))
    }

    fn read_buffer(&mut self, ptr: u64) -> VMLogicResult<Vec<u8>> {
        let buffer = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(ptr)? };
        Ok(self.read_guest_memory_slice(&buffer)?.to_vec())
    }

    fn write_register_bytes(&mut self, register_id: u64, bytes: &[u8]) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| logic.registers.set(logic.limits, register_id, bytes))
    }

    fn write_error_message(
        &mut self,
        register_id: u64,
        message: impl Display,
    ) -> VMLogicResult<i32> {
        let string = message.to_string();
        self.write_register_bytes(register_id, string.as_bytes())?;
        Ok(-1)
    }

    fn clear_register(&mut self, register_id: u64) -> VMLogicResult<()> {
        self.write_register_bytes(register_id, &[])
    }
}
