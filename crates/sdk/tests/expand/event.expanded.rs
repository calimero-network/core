use calimero_sdk::app;
#[serde(crate = "::calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Event {
    Created,
    ValueSet { key: String, value: String },
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
                Event::Created => {
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
                            variant_name: "Created",
                        },
                    )?;
                    _serde::ser::SerializeStruct::end(__struct)
                }
                Event::ValueSet { ref key, ref value } => {
                    #[doc(hidden)]
                    struct __AdjacentlyTagged<'__a> {
                        data: (&'__a String, &'__a String),
                        phantom: _serde::__private229::PhantomData<Event>,
                    }
                    #[automatically_derived]
                    impl<'__a> _serde::Serialize for __AdjacentlyTagged<'__a> {
                        fn serialize<__S>(
                            &self,
                            __serializer: __S,
                        ) -> _serde::__private229::Result<__S::Ok, __S::Error>
                        where
                            __S: _serde::Serializer,
                        {
                            #[allow(unused_variables)]
                            let (key, value) = self.data;
                            let mut __serde_state = _serde::Serializer::serialize_struct(
                                __serializer,
                                "ValueSet",
                                0 + 1 + 1,
                            )?;
                            _serde::ser::SerializeStruct::serialize_field(
                                &mut __serde_state,
                                "key",
                                key,
                            )?;
                            _serde::ser::SerializeStruct::serialize_field(
                                &mut __serde_state,
                                "value",
                                value,
                            )?;
                            _serde::ser::SerializeStruct::end(__serde_state)
                        }
                    }
                    let mut __struct = _serde::Serializer::serialize_struct(
                        __serializer,
                        "Event",
                        2,
                    )?;
                    _serde::ser::SerializeStruct::serialize_field(
                        &mut __struct,
                        "kind",
                        &_serde::__private229::ser::AdjacentlyTaggedEnumVariant {
                            enum_name: "Event",
                            variant_index: 1u32,
                            variant_name: "ValueSet",
                        },
                    )?;
                    _serde::ser::SerializeStruct::serialize_field(
                        &mut __struct,
                        "data",
                        &__AdjacentlyTagged {
                            data: (key, value),
                            phantom: _serde::__private229::PhantomData::<Event>,
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
fn main() {}
