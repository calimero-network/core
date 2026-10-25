use calimero_sdk::app;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
#[serde(crate = "::calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Event {
    Touched,
}
#[doc(hidden)]
#[allow(
    non_upper_case_globals,
    unused_attributes,
    unused_qualifications,
    clippy::absolute_paths,
)]
const _: () = {
    use ::calimero_sdk::serde as _serde;
    #[automatically_derived]
    impl _serde::Serialize for Event {
        fn serialize<__S>(
            &self,
            __serializer: __S,
        ) -> _serde::__private229::Result<__S::Ok, __S::Error>
        where
            __S: _serde::Serializer,
        {
            match *self {
                Event::Touched => {
                    let mut __struct = _serde::Serializer::serialize_struct(
                        __serializer,
                        "Event",
                        1,
                    )?;
                    _serde::ser::SerializeStruct::serialize_field(
                        &mut __struct,
                        "kind",
                        &_serde::__private229::ser::AdjacentlyTaggedEnumVariant {
                            enum_name: "Event",
                            variant_index: 0u32,
                            variant_name: "Touched",
                        },
                    )?;
                    _serde::ser::SerializeStruct::end(__struct)
                }
            }
        }
    }
};
impl ::calimero_sdk::event::AppEvent for Event {
    fn kind(&self) -> ::std::borrow::Cow<str> {
        match ::calimero_sdk::serde_json::to_value(self) {
            ::core::result::Result::Ok(
                ::calimero_sdk::serde_json::Value::Object(mut __obj),
            ) => {
                match __obj.remove("kind") {
                    ::core::option::Option::Some(
                        ::calimero_sdk::serde_json::Value::String(__k),
                    ) => ::std::borrow::Cow::Owned(__k),
                    _ => {
                        ::calimero_sdk::env::panic_str(
                            "event did not serialize with a string `kind` tag",
                        )
                    }
                }
            }
            ::core::result::Result::Ok(_) => {
                ::calimero_sdk::env::panic_str(
                    "event did not serialize to a `{ kind, data }` object",
                )
            }
            ::core::result::Result::Err(__err) => {
                ::calimero_sdk::env::panic_str(
                    &::alloc::__export::must_use({
                        ::alloc::fmt::format(
                            format_args!("Failed to serialize event: {0:?}", __err),
                        )
                    }),
                )
            }
        }
    }
    fn data(&self) -> ::std::borrow::Cow<[u8]> {
        match ::calimero_sdk::serde_json::to_value(self) {
            ::core::result::Result::Ok(
                ::calimero_sdk::serde_json::Value::Object(__obj),
            ) => {
                match __obj.get("data") {
                    ::core::option::Option::None
                    | ::core::option::Option::Some(
                        ::calimero_sdk::serde_json::Value::Null,
                    ) => ::std::borrow::Cow::Borrowed(&[][..]),
                    ::core::option::Option::Some(__data) => {
                        match ::calimero_sdk::serde_json::to_vec(__data) {
                            ::core::result::Result::Ok(__bytes) => {
                                ::std::borrow::Cow::Owned(__bytes)
                            }
                            ::core::result::Result::Err(__err) => {
                                ::calimero_sdk::env::panic_str(
                                    &::alloc::__export::must_use({
                                        ::alloc::fmt::format(
                                            format_args!("Failed to serialize event data: {0:?}", __err),
                                        )
                                    }),
                                )
                            }
                        }
                    }
                }
            }
            ::core::result::Result::Ok(_) => {
                ::calimero_sdk::env::panic_str(
                    "event did not serialize to a `{ kind, data }` object",
                )
            }
            ::core::result::Result::Err(__err) => {
                ::calimero_sdk::env::panic_str(
                    &::alloc::__export::must_use({
                        ::alloc::fmt::format(
                            format_args!("Failed to serialize event: {0:?}", __err),
                        )
                    }),
                )
            }
        }
    }
}
impl ::calimero_sdk::event::AppEventExt for Event {}
#[borsh(crate = "::calimero_sdk::borsh")]
struct State {
    items: UnorderedMap<String, LwwRegister<String>>,
    label: LwwRegister<String>,
}
#[automatically_derived]
impl ::calimero_sdk::borsh::ser::BorshSerialize for State {
    fn serialize<__W: ::calimero_sdk::borsh::io::Write>(
        &self,
        writer: &mut __W,
    ) -> ::core::result::Result<(), ::calimero_sdk::borsh::io::Error> {
        ::calimero_sdk::borsh::BorshSerialize::serialize(&self.items, writer)?;
        ::calimero_sdk::borsh::BorshSerialize::serialize(&self.label, writer)?;
        Ok(())
    }
}
#[automatically_derived]
impl ::calimero_sdk::borsh::de::BorshDeserialize for State {
    fn deserialize_reader<__R: ::calimero_sdk::borsh::io::Read>(
        reader: &mut __R,
    ) -> ::core::result::Result<Self, ::calimero_sdk::borsh::io::Error> {
        Ok(Self {
            items: ::calimero_sdk::borsh::BorshDeserialize::deserialize_reader(reader)?,
            label: ::calimero_sdk::borsh::BorshDeserialize::deserialize_reader(reader)?,
        })
    }
}
impl ::calimero_sdk::state::AppState for State {
    type Event<'a> = Event;
}
impl ::calimero_storage::collections::MergeStrategy for State {
    const DISPATCHED: bool = false;
}
impl ::calimero_storage::collections::Mergeable for State {
    fn merge(
        &mut self,
        other: &Self,
    ) -> ::core::result::Result<
        (),
        ::calimero_storage::collections::crdt_meta::MergeError,
    > {
        ::calimero_storage::collections::Mergeable::merge(&mut self.items, &other.items)
            .map_err(|e| {
                ::calimero_storage::collections::crdt_meta::MergeError::StorageError(
                    ::alloc::__export::must_use({
                        ::alloc::fmt::format(
                            format_args!(
                                "Failed to merge field \'{0}\': {1:?}", "items", e,
                            ),
                        )
                    }),
                )
            })?;
        ::calimero_storage::collections::Mergeable::merge(&mut self.label, &other.label)
            .map_err(|e| {
                ::calimero_storage::collections::crdt_meta::MergeError::StorageError(
                    ::alloc::__export::must_use({
                        ::alloc::fmt::format(
                            format_args!(
                                "Failed to merge field \'{0}\': {1:?}", "label", e,
                            ),
                        )
                    }),
                )
            })?;
        ::core::result::Result::Ok(())
    }
}
impl ::calimero_storage::collections::rekey::RekeyTarget for State {
    fn rekey_relative_to(&mut self, parent_id: ::calimero_storage::address::Id) {
        {
            struct __RkProbe<'a, T: ?::core::marker::Sized>(&'a mut T);
            trait __RkViaRekey {
                fn __rk_go(self, p: ::calimero_storage::address::Id);
            }
            impl<
                'a,
                T: ::calimero_storage::collections::rekey::RekeyTarget + 'static,
            > __RkViaRekey for __RkProbe<'a, T> {
                fn __rk_go(self, p: ::calimero_storage::address::Id) {
                    ::calimero_storage::collections::rekey::RekeyTarget::rekey_relative_to(
                        self.0,
                        p,
                    );
                }
            }
            trait __RkViaNoop {
                fn __rk_go(self, p: ::calimero_storage::address::Id);
            }
            impl<'a, T: ?::core::marker::Sized> __RkViaNoop for &__RkProbe<'a, T> {
                fn __rk_go(self, _p: ::calimero_storage::address::Id) {}
            }
            __RkProbe(&mut self.items)
                .__rk_go(
                    ::calimero_storage::collections::rekey::field_child_id(
                        parent_id,
                        "items",
                    ),
                )
        };
        {
            struct __RkProbe<'a, T: ?::core::marker::Sized>(&'a mut T);
            trait __RkViaRekey {
                fn __rk_go(self, p: ::calimero_storage::address::Id);
            }
            impl<
                'a,
                T: ::calimero_storage::collections::rekey::RekeyTarget + 'static,
            > __RkViaRekey for __RkProbe<'a, T> {
                fn __rk_go(self, p: ::calimero_storage::address::Id) {
                    ::calimero_storage::collections::rekey::RekeyTarget::rekey_relative_to(
                        self.0,
                        p,
                    );
                }
            }
            trait __RkViaNoop {
                fn __rk_go(self, p: ::calimero_storage::address::Id);
            }
            impl<'a, T: ?::core::marker::Sized> __RkViaNoop for &__RkProbe<'a, T> {
                fn __rk_go(self, _p: ::calimero_storage::address::Id) {}
            }
            __RkProbe(&mut self.label)
                .__rk_go(
                    ::calimero_storage::collections::rekey::field_child_id(
                        parent_id,
                        "label",
                    ),
                )
        };
    }
    fn register_nested_value_types() {
        {
            struct __RgProbe<T>(::core::marker::PhantomData<T>);
            trait __RgViaReg {
                fn __rg_go(self);
            }
            impl<
                T: ::calimero_storage::collections::rekey::RekeyTarget + 'static,
            > __RgViaReg for __RgProbe<T> {
                fn __rg_go(self) {
                    ::calimero_storage::collections::rekey::register_rekey_cascade::<
                        T,
                    >();
                }
            }
            trait __RgViaNoop {
                fn __rg_go(self);
            }
            impl<T> __RgViaNoop for &__RgProbe<T> {
                fn __rg_go(self) {}
            }
            __RgProbe::<
                UnorderedMap<String, LwwRegister<String>>,
            >(::core::marker::PhantomData)
                .__rg_go()
        };
        {
            struct __RgProbe<T>(::core::marker::PhantomData<T>);
            trait __RgViaReg {
                fn __rg_go(self);
            }
            impl<
                T: ::calimero_storage::collections::rekey::RekeyTarget + 'static,
            > __RgViaReg for __RgProbe<T> {
                fn __rg_go(self) {
                    ::calimero_storage::collections::rekey::register_rekey_cascade::<
                        T,
                    >();
                }
            }
            trait __RgViaNoop {
                fn __rg_go(self);
            }
            impl<T> __RgViaNoop for &__RgProbe<T> {
                fn __rg_go(self) {}
            }
            __RgProbe::<String>(::core::marker::PhantomData).__rg_go()
        };
        {
            struct __RgProbe<T>(::core::marker::PhantomData<T>);
            trait __RgViaReg {
                fn __rg_go(self);
            }
            impl<
                T: ::calimero_storage::collections::rekey::RekeyTarget + 'static,
            > __RgViaReg for __RgProbe<T> {
                fn __rg_go(self) {
                    ::calimero_storage::collections::rekey::register_rekey_cascade::<
                        T,
                    >();
                }
            }
            trait __RgViaNoop {
                fn __rg_go(self);
            }
            impl<T> __RgViaNoop for &__RgProbe<T> {
                fn __rg_go(self) {}
            }
            __RgProbe::<LwwRegister<String>>(::core::marker::PhantomData).__rg_go()
        };
    }
}
#[no_mangle]
pub extern "C" fn __calimero_register_merge() {
    ::calimero_storage::register_crdt_merge::<State>();
    <State>::__calimero_register_rekey();
}
#[no_mangle]
pub extern "C" fn __calimero_merge_custom() {
    ::calimero_sdk::env::setup_panic_hook();
    let Some(args) = ::calimero_sdk::env::input() else {
        ::calimero_sdk::env::panic_str(
            "Expected MergeCustomRequest payload for __calimero_merge_custom",
        )
    };
    let request: ::calimero_storage::merge::MergeCustomRequest = ::calimero_sdk::borsh::from_slice(
            &args,
        )
        .unwrap_or_else(|err| {
            ::calimero_sdk::env::panic_str(
                &::alloc::__export::must_use({
                    ::alloc::fmt::format(
                        format_args!(
                            "Failed to deserialize MergeCustomRequest: {0}", err,
                        ),
                    )
                }),
            )
        });
    let response = match ::calimero_storage::merge::merge_custom(
        request.type_id,
        &request.existing,
        &request.incoming,
    ) {
        ::core::result::Result::Ok(bytes) => {
            ::calimero_storage::merge::MergeCustomResponse::Ok(bytes)
        }
        ::core::result::Result::Err(err) => {
            ::calimero_storage::merge::MergeCustomResponse::Err(
                ::alloc::__export::must_use({
                    ::alloc::fmt::format(format_args!("{0:?}", err))
                }),
            )
        }
    };
    let serialized = ::calimero_sdk::borsh::to_vec(&response)
        .unwrap_or_else(|err| {
            ::calimero_sdk::env::panic_str(
                &::alloc::__export::must_use({
                    ::alloc::fmt::format(
                        format_args!("Failed to serialize MergeCustomResponse: {0}", err),
                    )
                }),
            )
        });
    ::calimero_sdk::env::value_return(
        &::core::result::Result::<
            ::std::vec::Vec<u8>,
            ::std::vec::Vec<u8>,
        >::Ok(serialized),
    );
}
#[no_mangle]
pub extern "C" fn __calimero_merge_root_state() {
    ::calimero_sdk::env::setup_panic_hook();
    let Some(args) = ::calimero_sdk::env::input() else {
        ::calimero_sdk::env::panic_str(
            "Expected MergeRootStateRequest payload for __calimero_merge_root_state",
        )
    };
    let request: ::calimero_storage::merge::MergeRootStateRequest = ::calimero_sdk::borsh::from_slice(
            &args,
        )
        .unwrap_or_else(|err| {
            ::calimero_sdk::env::panic_str(
                &::alloc::__export::must_use({
                    ::alloc::fmt::format(
                        format_args!(
                            "Failed to deserialize MergeRootStateRequest: {0}", err,
                        ),
                    )
                }),
            )
        });
    let response = ::calimero_storage::merge::merge_root_state_typed::<State>(&request);
    let serialized = ::calimero_sdk::borsh::to_vec(&response)
        .unwrap_or_else(|err| {
            ::calimero_sdk::env::panic_str(
                &::alloc::__export::must_use({
                    ::alloc::fmt::format(
                        format_args!(
                            "Failed to serialize MergeRootStateResponse: {0}", err,
                        ),
                    )
                }),
            )
        });
    ::calimero_sdk::env::value_return(
        &::core::result::Result::<
            ::std::vec::Vec<u8>,
            ::std::vec::Vec<u8>,
        >::Ok(serialized),
    );
}
impl State {
    /// Registers nested CRDT-value re-key thunks. Called at WASM module
    /// load and by the native test bridge; idempotent. Not for direct use.
    #[doc(hidden)]
    pub fn __calimero_register_rekey() {
        {
            struct __RgProbe<T>(::core::marker::PhantomData<T>);
            trait __RgViaReg {
                fn __rg_go(self);
            }
            impl<
                T: ::calimero_storage::collections::rekey::RekeyTarget + 'static,
            > __RgViaReg for __RgProbe<T> {
                fn __rg_go(self) {
                    ::calimero_storage::collections::rekey::register_rekey_cascade::<
                        T,
                    >();
                }
            }
            trait __RgViaNoop {
                fn __rg_go(self);
            }
            impl<T> __RgViaNoop for &__RgProbe<T> {
                fn __rg_go(self) {}
            }
            __RgProbe::<
                UnorderedMap<String, LwwRegister<String>>,
            >(::core::marker::PhantomData)
                .__rg_go()
        };
        {
            struct __RgProbe<T>(::core::marker::PhantomData<T>);
            trait __RgViaReg {
                fn __rg_go(self);
            }
            impl<
                T: ::calimero_storage::collections::rekey::RekeyTarget + 'static,
            > __RgViaReg for __RgProbe<T> {
                fn __rg_go(self) {
                    ::calimero_storage::collections::rekey::register_rekey_cascade::<
                        T,
                    >();
                }
            }
            trait __RgViaNoop {
                fn __rg_go(self);
            }
            impl<T> __RgViaNoop for &__RgProbe<T> {
                fn __rg_go(self) {}
            }
            __RgProbe::<String>(::core::marker::PhantomData).__rg_go()
        };
        {
            struct __RgProbe<T>(::core::marker::PhantomData<T>);
            trait __RgViaReg {
                fn __rg_go(self);
            }
            impl<
                T: ::calimero_storage::collections::rekey::RekeyTarget + 'static,
            > __RgViaReg for __RgProbe<T> {
                fn __rg_go(self) {
                    ::calimero_storage::collections::rekey::register_rekey_cascade::<
                        T,
                    >();
                }
            }
            trait __RgViaNoop {
                fn __rg_go(self);
            }
            impl<T> __RgViaNoop for &__RgProbe<T> {
                fn __rg_go(self) {}
            }
            __RgProbe::<LwwRegister<String>>(::core::marker::PhantomData).__rg_go()
        };
    }
}
impl State {
    /// Assigns deterministic IDs to all collection fields based on their field names.
    ///
    /// This is called automatically by the init wrapper. Users should not call this directly.
    #[doc(hidden)]
    pub fn __assign_deterministic_ids(&mut self) {
        self.items.reassign_deterministic_id("items");
    }
}
fn main() {}
