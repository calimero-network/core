use calimero_sdk::app;
use calimero_storage::collections::LwwRegister;
#[borsh(crate = "::calimero_sdk::borsh")]
struct State {
    value: LwwRegister<String>,
}
#[automatically_derived]
impl ::calimero_sdk::borsh::ser::BorshSerialize for State {
    fn serialize<__W: ::calimero_sdk::borsh::io::Write>(
        &self,
        writer: &mut __W,
    ) -> ::core::result::Result<(), ::calimero_sdk::borsh::io::Error> {
        ::calimero_sdk::borsh::BorshSerialize::serialize(&self.value, writer)?;
        Ok(())
    }
}
#[automatically_derived]
impl ::calimero_sdk::borsh::de::BorshDeserialize for State {
    fn deserialize_reader<__R: ::calimero_sdk::borsh::io::Read>(
        reader: &mut __R,
    ) -> ::core::result::Result<Self, ::calimero_sdk::borsh::io::Error> {
        Ok(Self {
            value: ::calimero_sdk::borsh::BorshDeserialize::deserialize_reader(reader)?,
        })
    }
}
impl ::calimero_sdk::state::AppState for State {
    type Event<'a> = ::calimero_sdk::event::NoEvent;
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
        ::calimero_storage::collections::Mergeable::merge(&mut self.value, &other.value)
            .map_err(|e| {
                ::calimero_storage::collections::crdt_meta::MergeError::StorageError(
                    ::alloc::__export::must_use({
                        ::alloc::fmt::format(
                            format_args!(
                                "Failed to merge field \'{0}\': {1:?}", "value", e,
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
            __RkProbe(&mut self.value)
                .__rk_go(
                    ::calimero_storage::collections::rekey::field_child_id(
                        parent_id,
                        "value",
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
            __RgProbe::<LwwRegister<String>>(::core::marker::PhantomData).__rg_go()
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
            __RgProbe::<LwwRegister<String>>(::core::marker::PhantomData).__rg_go()
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
    }
}
impl State {
    /// Assigns deterministic IDs to all collection fields based on their field names.
    ///
    /// This is called automatically by the init wrapper. Users should not call this directly.
    #[doc(hidden)]
    pub fn __assign_deterministic_ids(&mut self) {}
}
impl State {
    pub fn init() -> State {
        State {
            value: LwwRegister::new(String::new()),
        }
    }
    pub fn get(&self) -> &str {
        &self.value
    }
    pub fn ping(&mut self) {}
    pub fn set(&mut self, value: String) {
        self.value.set(value);
    }
}
#[no_mangle]
pub extern "C" fn init() {
    ::calimero_sdk::env::setup_panic_hook();
    ::calimero_sdk::env::init_logging();
    ::calimero_sdk::event::register::<State>();
    ::calimero_sdk::app::register_schema_version::<State>();
    if let Some(input) = ::calimero_sdk::env::input() {
        if !input.is_empty() {
            if let Ok(::calimero_sdk::serde_json::Value::Object(fields)) = ::calimero_sdk::serde_json::from_slice::<
                ::calimero_sdk::serde_json::Value,
            >(&input) {
                if !fields.is_empty() {
                    ::calimero_sdk::env::panic_str(
                        &::alloc::__export::must_use({
                            ::alloc::fmt::format(
                                format_args!(
                                    "{0}: takes no arguments, but the call sent unknown field(s): {1:?}",
                                    "init", fields.keys().collect::<::std::vec::Vec < _ >> (),
                                ),
                            )
                        }),
                    );
                }
            }
        }
    }
    if ::calimero_storage::collections::Root::<State>::fetch().is_some() {
        ::calimero_sdk::env::panic_str("Cannot initialize over already existing state.")
    }
    let app = ::calimero_storage::collections::Root::new(|| {
        let mut state = <State>::init();
        state.__assign_deterministic_ids();
        state
    });
    app.commit();
}
#[no_mangle]
pub extern "C" fn __calimero_sync_next() {
    ::calimero_sdk::env::setup_panic_hook();
    let Some(args) = ::calimero_sdk::env::input() else {
        ::calimero_sdk::env::panic_str("Expected payload to sync method.")
    };
    let __sync_ctx = ::calimero_storage::interface::ApplyContext::empty();
    ::calimero_storage::collections::Root::<State>::sync(&args, &__sync_ctx)
        .expect("fatal: sync failed");
}
impl ::calimero_sdk::state::AppStateInit for State {
    type Return = State;
}
#[no_mangle]
pub extern "C" fn get() {
    ::calimero_sdk::env::setup_panic_hook();
    ::calimero_sdk::env::init_logging();
    ::calimero_sdk::event::register::<State>();
    ::calimero_sdk::app::register_schema_version::<State>();
    if let Some(input) = ::calimero_sdk::env::input() {
        if !input.is_empty() {
            if let Ok(::calimero_sdk::serde_json::Value::Object(fields)) = ::calimero_sdk::serde_json::from_slice::<
                ::calimero_sdk::serde_json::Value,
            >(&input) {
                if !fields.is_empty() {
                    ::calimero_sdk::env::panic_str(
                        &::alloc::__export::must_use({
                            ::alloc::fmt::format(
                                format_args!(
                                    "{0}: takes no arguments, but the call sent unknown field(s): {1:?}",
                                    "get", fields.keys().collect::<::std::vec::Vec < _ >> (),
                                ),
                            )
                        }),
                    );
                }
            }
        }
    }
    let Some(app) = ::calimero_storage::collections::Root::<State>::fetch() else {
        ::calimero_sdk::env::panic_str("Failed to find or read app state")
    };
    let output = app.get();
    let output = {
        #[allow(unused_imports)]
        use ::calimero_sdk::__private::IntoResult;
        match ::calimero_sdk::__private::WrappedReturn::new(output)
            .into_result()
            .to_json()
        {
            Ok(output) => output,
            Err(err) => {
                ::calimero_sdk::env::panic_str(
                    &::alloc::__export::must_use({
                        ::alloc::fmt::format(
                            format_args!(
                                "{0}: failed to serialize the return value to JSON: {1:?}",
                                "get", err,
                            ),
                        )
                    }),
                )
            }
        }
    };
    ::calimero_sdk::env::value_return(&output);
}
#[no_mangle]
pub extern "C" fn ping() {
    ::calimero_sdk::env::setup_panic_hook();
    ::calimero_sdk::env::init_logging();
    ::calimero_sdk::event::register::<State>();
    ::calimero_sdk::app::register_schema_version::<State>();
    if let Some(input) = ::calimero_sdk::env::input() {
        if !input.is_empty() {
            if let Ok(::calimero_sdk::serde_json::Value::Object(fields)) = ::calimero_sdk::serde_json::from_slice::<
                ::calimero_sdk::serde_json::Value,
            >(&input) {
                if !fields.is_empty() {
                    ::calimero_sdk::env::panic_str(
                        &::alloc::__export::must_use({
                            ::alloc::fmt::format(
                                format_args!(
                                    "{0}: takes no arguments, but the call sent unknown field(s): {1:?}",
                                    "ping", fields.keys().collect::<::std::vec::Vec < _ >> (),
                                ),
                            )
                        }),
                    );
                }
            }
        }
    }
    let Some(mut app) = ::calimero_storage::collections::Root::<State>::fetch() else {
        ::calimero_sdk::env::panic_str("Failed to find or read app state")
    };
    app.ping();
    app.commit();
}
#[no_mangle]
pub extern "C" fn set() {
    ::calimero_sdk::env::setup_panic_hook();
    ::calimero_sdk::env::init_logging();
    ::calimero_sdk::event::register::<State>();
    ::calimero_sdk::app::register_schema_version::<State>();
    #[serde(crate = "::calimero_sdk::serde", deny_unknown_fields)]
    struct CalimeroInput {
        value: String,
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
        impl<'de> _serde::Deserialize<'de> for CalimeroInput {
            fn deserialize<__D>(
                __deserializer: __D,
            ) -> _serde::__private229::Result<Self, __D::Error>
            where
                __D: _serde::Deserializer<'de>,
            {
                #[allow(non_camel_case_types)]
                #[doc(hidden)]
                enum __Field {
                    __field0,
                }
                #[doc(hidden)]
                struct __FieldVisitor;
                #[automatically_derived]
                impl<'de> _serde::de::Visitor<'de> for __FieldVisitor {
                    type Value = __Field;
                    fn expecting(
                        &self,
                        __formatter: &mut _serde::__private229::Formatter,
                    ) -> _serde::__private229::fmt::Result {
                        _serde::__private229::Formatter::write_str(
                            __formatter,
                            "field identifier",
                        )
                    }
                    fn visit_u64<__E>(
                        self,
                        __value: u64,
                    ) -> _serde::__private229::Result<Self::Value, __E>
                    where
                        __E: _serde::de::Error,
                    {
                        match __value {
                            0u64 => _serde::__private229::Ok(__Field::__field0),
                            _ => {
                                _serde::__private229::Err(
                                    _serde::de::Error::invalid_value(
                                        _serde::de::Unexpected::Unsigned(__value),
                                        &"field index 0 <= i < 1",
                                    ),
                                )
                            }
                        }
                    }
                    fn visit_str<__E>(
                        self,
                        __value: &str,
                    ) -> _serde::__private229::Result<Self::Value, __E>
                    where
                        __E: _serde::de::Error,
                    {
                        match __value {
                            "value" => _serde::__private229::Ok(__Field::__field0),
                            _ => {
                                _serde::__private229::Err(
                                    _serde::de::Error::unknown_field(__value, FIELDS),
                                )
                            }
                        }
                    }
                    fn visit_bytes<__E>(
                        self,
                        __value: &[u8],
                    ) -> _serde::__private229::Result<Self::Value, __E>
                    where
                        __E: _serde::de::Error,
                    {
                        match __value {
                            b"value" => _serde::__private229::Ok(__Field::__field0),
                            _ => {
                                let __value = &_serde::__private229::from_utf8_lossy(
                                    __value,
                                );
                                _serde::__private229::Err(
                                    _serde::de::Error::unknown_field(__value, FIELDS),
                                )
                            }
                        }
                    }
                }
                #[automatically_derived]
                impl<'de> _serde::Deserialize<'de> for __Field {
                    #[inline]
                    fn deserialize<__D>(
                        __deserializer: __D,
                    ) -> _serde::__private229::Result<Self, __D::Error>
                    where
                        __D: _serde::Deserializer<'de>,
                    {
                        _serde::Deserializer::deserialize_identifier(
                            __deserializer,
                            __FieldVisitor,
                        )
                    }
                }
                #[doc(hidden)]
                struct __Visitor<'de> {
                    marker: _serde::__private229::PhantomData<CalimeroInput>,
                    lifetime: _serde::__private229::PhantomData<&'de ()>,
                }
                #[automatically_derived]
                impl<'de> _serde::de::Visitor<'de> for __Visitor<'de> {
                    type Value = CalimeroInput;
                    fn expecting(
                        &self,
                        __formatter: &mut _serde::__private229::Formatter,
                    ) -> _serde::__private229::fmt::Result {
                        _serde::__private229::Formatter::write_str(
                            __formatter,
                            "struct CalimeroInput",
                        )
                    }
                    #[inline]
                    fn visit_seq<__A>(
                        self,
                        mut __seq: __A,
                    ) -> _serde::__private229::Result<Self::Value, __A::Error>
                    where
                        __A: _serde::de::SeqAccess<'de>,
                    {
                        let __field0 = match _serde::de::SeqAccess::next_element::<
                            String,
                        >(&mut __seq)? {
                            _serde::__private229::Some(__value) => __value,
                            _serde::__private229::None => {
                                return _serde::__private229::Err(
                                    _serde::de::Error::invalid_length(
                                        0usize,
                                        &"struct CalimeroInput with 1 element",
                                    ),
                                );
                            }
                        };
                        _serde::__private229::Ok(CalimeroInput { value: __field0 })
                    }
                    #[inline]
                    fn visit_map<__A>(
                        self,
                        mut __map: __A,
                    ) -> _serde::__private229::Result<Self::Value, __A::Error>
                    where
                        __A: _serde::de::MapAccess<'de>,
                    {
                        let mut __field0: _serde::__private229::Option<String> = _serde::__private229::None;
                        while let _serde::__private229::Some(__key) = _serde::de::MapAccess::next_key::<
                            __Field,
                        >(&mut __map)? {
                            match __key {
                                __Field::__field0 => {
                                    if _serde::__private229::Option::is_some(&__field0) {
                                        return _serde::__private229::Err(
                                            <__A::Error as _serde::de::Error>::duplicate_field("value"),
                                        );
                                    }
                                    __field0 = _serde::__private229::Some(
                                        _serde::de::MapAccess::next_value::<String>(&mut __map)?,
                                    );
                                }
                            }
                        }
                        let __field0 = match __field0 {
                            _serde::__private229::Some(__field0) => __field0,
                            _serde::__private229::None => {
                                _serde::__private229::de::missing_field("value")?
                            }
                        };
                        _serde::__private229::Ok(CalimeroInput { value: __field0 })
                    }
                }
                #[doc(hidden)]
                const FIELDS: &'static [&'static str] = &["value"];
                _serde::Deserializer::deserialize_struct(
                    __deserializer,
                    "CalimeroInput",
                    FIELDS,
                    __Visitor {
                        marker: _serde::__private229::PhantomData::<CalimeroInput>,
                        lifetime: _serde::__private229::PhantomData,
                    },
                )
            }
        }
    };
    let _ = ::calimero_sdk::__private::assert_app_arg::<String>;
    let Some(input) = ::calimero_sdk::env::input() else {
        ::calimero_sdk::env::panic_str(
            &::alloc::__export::must_use({
                ::alloc::fmt::format(
                    format_args!(
                        "{0}: missing arguments — expected a JSON object {1}", "set",
                        "{ value: String }",
                    ),
                )
            }),
        )
    };
    let CalimeroInput { value } = match ::calimero_sdk::serde_json::from_slice(&input) {
        Ok(value) => value,
        Err(err) => {
            ::calimero_sdk::env::panic_str(
                &::alloc::__export::must_use({
                    ::alloc::fmt::format(
                        format_args!(
                            "{0}: failed to deserialize arguments (expected {1}): {2:?}",
                            "set", "{ value: String }", err,
                        ),
                    )
                }),
            )
        }
    };
    let Some(mut app) = ::calimero_storage::collections::Root::<State>::fetch() else {
        ::calimero_sdk::env::panic_str("Failed to find or read app state")
    };
    app.set(value);
    app.commit();
}
#[allow(unexpected_cfgs, non_snake_case)]
#[doc(hidden)]
pub mod __calimero_abi_State {}
fn main() {}
