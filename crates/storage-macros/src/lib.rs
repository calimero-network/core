use borsh as _;
/// For documentation links
use calimero_storage as _;
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, parse_quote, Data, DeriveInput, Fields, Type};

#[cfg(test)]
mod integration_tests_package_usage {
    use {borsh as _, calimero_storage as _, calimero_test_utils as _, trybuild as _};
}

/// Derives the [`AtomicUnit`](calimero_storage::entities::AtomicUnit) trait for
/// a struct.
///
/// This macro automatically implements the [`AtomicUnit`](calimero_storage::entities::AtomicUnit)
/// trait for a struct, which extends the [`Data`](calimero_storage::entities::Data)
/// trait.
///
/// # Requirements
///
/// The following are mandatory requirements for the struct:
///
///   - A private `storage` field of type [`Element`](calimero_storage::entities::Element).
///     This is needed as the [`Data`](calimero_storage::entities::Data)-based
///     struct needs to own an [`Element`](calimero_storage::entities::Element).
///
/// # Generated implementations
///
/// This macro will generate the following implementations:
///
///   - [`Data`](calimero_storage::entities::Data) trait implementation.
///   - [`AtomicUnit`](calimero_storage::entities::AtomicUnit) trait
///     implementation.
///   - Getter and setter methods for each field. These help to ensure that the
///     access to the fields is controlled, and that any changes to the fields
///     are reflected in the [`Element`](calimero_storage::entities::Element)'s
///     state.
///   - [`BorshSerialize`](borsh::BorshSerialize) and [`BorshDeserialize`](borsh::BorshDeserialize)
///     will be implemented for the struct, so they should be omitted from the
///     struct definition.
///
/// # Struct attributes
///
/// * `#[children]` - An optional attribute to specify the child type for the
///                   struct, written as `#[children(ChildType)]`. If the
///                   attribute or its value are omitted, then the child type
///                   will default to [`NoChildren`](calimero_storage::entities::NoChildren).
///
/// # Field attributes
///
/// * `#[collection]` - Indicates that the field is a collection of other
///                     [`Data`] types.
/// * `#[private]`    - Designates fields that are local-only, and so should not
///                     be shared with other nodes in the network. These fields
///                     will not be serialised or included in the Merkle hash
///                     calculation. Note that being local-only is not the same
///                     as applying permissions via ACLs to share with only the
///                     current user — these fields are not shared at all.
/// * `#[skip]`       - Can be applied to fields that should not be serialised
///                     or included in the Merkle hash calculation. These fields
///                     will be completely ignored by the storage system, and
///                     not even have getters and setters implemented.
/// * `#[storage]`    - Indicates that the field is the storage element for the
///                     struct. This is a mandatory field, and if it is missing,
///                     there will be a panic during compilation. The name is
///                     arbitrary, but the type has to be an [`Element`](calimero_storage::entities::Element).
///
/// Note that fields marked with `#[private]` or `#[skip]` must have [`Default`]
/// implemented so that they can be initialised when deserialising.
///
/// TODO: The `#[collection]` attribute is not yet implemented, and the
///       `#[private]` attribute is implemented with the same functionality as
///       `#[skip]`, but in future these will be differentiated.
///
/// # Getters and setters
///
/// The macro will generate getter and setter methods for each field. These
/// methods will allow the struct to control access to its fields, and ensure
/// that any changes to the fields are reflected in the [`Element`](calimero_storage::entities::Element)'s
/// state.
///
/// The getter methods will have the same name as the field, and the setter
/// methods will be prefixed with `set_`. For example, given a field `name`, the
/// getter method will be `name()`, and the setter method will be `set_name()`.
///
/// The setter methods will return a boolean indicating whether the update was
/// carried out. Note that this is more an indication of change than it is of
/// error — if the value is the same as the current value, the update will not
/// be carried out, and the method will return `false`.
///
/// # Examples
///
/// ```
/// use calimero_storage::entities::Element;
/// use calimero_storage_macros::AtomicUnit;
///
/// #[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
/// #[children(Paragraph)]
/// struct Page {
///     title: String,
///     #[private]
///     secret: String,
///     #[storage]
///     storage: Element,
/// }
///
/// #[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
/// struct Paragraph {
///     text: String,
///     #[storage]
///     storage: Element,
/// }
/// ```
///
/// TODO: Once multiple child types are supported, this example will represent
///       the correct approach.
///
/// ```ignore
/// use calimero_storage::entities::Element;
/// use calimero_storage_macros::AtomicUnit;
///
/// #[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
/// struct Person {
///     name: String,
///     age: u32,
///     #[private]
///     secret: String,
///     #[collection]
///     friends: Vec<Person>,
///     #[storage]
///     storage: Element,
/// }
/// ```
///
/// # Panics
///
/// This macro will panic during compilation if:
///
///   - It is applied to anything other than a struct
///   - The struct has unnamed fields
///   - The struct does not have a field annotated as `#[storage]`
///   - The struct has fields with types that do not implement [`Default`]
///   - The struct already has methods with the same names as the generated
///     getter and setter methods
///
#[expect(
    clippy::too_many_lines,
    reason = "Okay for now - will be restructured later"
)]
#[proc_macro_derive(AtomicUnit, attributes(children, collection, private, skip, storage))]
pub fn atomic_unit_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let child_type = input
        .attrs
        .iter()
        .find_map(|attr| {
            if attr.path().is_ident("children") {
                attr.parse_args::<Type>().ok()
            } else {
                None
            }
        })
        .unwrap_or_else(|| parse_quote!(calimero_storage::entities::NoChildren));

    let fields = match &input.data {
        Data::Struct(data) => &data.fields,
        Data::Enum(_) | Data::Union(_) => panic!("AtomicUnit can only be derived for structs"),
    };

    let named_fields = match fields {
        Fields::Named(fields) => &fields.named,
        Fields::Unnamed(_) | Fields::Unit => {
            panic!("AtomicUnit can only be derived for structs with named fields")
        }
    };

    // Find the field marked with the #[storage] attribute
    let storage_field = named_fields
        .iter()
        .find(|f| f.attrs.iter().any(|attr| attr.path().is_ident("storage")))
        .expect("You must designate one field with #[storage] for the Element");

    let storage_ident = storage_field.ident.as_ref().unwrap();
    let storage_ty = &storage_field.ty;

    let field_implementations = named_fields
        .iter()
        .filter_map(|f| {
            let ident = f.ident.as_ref().unwrap();
            let ty = &f.ty;

            let skip = f.attrs.iter().any(|attr| attr.path().is_ident("skip"));

            if skip || ident == storage_ident {
                None
            } else {
                let getter = format_ident!("{}", ident);
                let setter = format_ident!("set_{}", ident);

                Some(quote! {
                    pub fn #getter(&self) -> &#ty {
                        &self.#ident
                    }

                    pub fn #setter(&mut self, value: #ty) -> bool {
                        if self.#ident == value {
                            false
                        } else {
                            self.#ident = value;
                            self.#storage_ident.update();
                            true
                        }
                    }
                })
            }
        })
        .collect::<Vec<_>>();

    let serializable_fields: Vec<_> = named_fields
        .iter()
        .filter(|f| {
            !f.attrs
                .iter()
                .any(|attr| attr.path().is_ident("skip") || attr.path().is_ident("private"))
                && f.ident.as_ref().unwrap() != storage_ident
        })
        .map(|f| f.ident.as_ref().unwrap())
        .collect();

    let skipped_fields: Vec<_> = named_fields
        .iter()
        .filter(|f| {
            f.attrs
                .iter()
                .any(|attr| attr.path().is_ident("skip") || attr.path().is_ident("private"))
        })
        .map(|f| f.ident.as_ref().unwrap())
        .collect();

    let deserialize_impl = quote! {
        impl borsh::BorshDeserialize for #name {
            fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
                let #storage_ident = #storage_ty::deserialize_reader(reader)?;
                Ok(Self {
                    #storage_ident,
                    #(#serializable_fields: borsh::BorshDeserialize::deserialize_reader(reader)?,)*
                    #(#skipped_fields: Default::default(),)*
                })
            }
        }
    };

    let serialize_impl = quote! {
        impl borsh::BorshSerialize for #name {
            fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
                borsh::BorshSerialize::serialize(&self.#storage_ident, writer)?;
                #(borsh::BorshSerialize::serialize(&self.#serializable_fields, writer)?;)*
                Ok(())
            }
        }
    };

    let expanded = quote! {
        impl #name {
            #(#field_implementations)*
        }

        impl calimero_storage::entities::Data for #name {
            type Child = #child_type;

            fn element(&self) -> &calimero_storage::entities::Element {
                &self.#storage_ident
            }

            fn element_mut(&mut self) -> &mut calimero_storage::entities::Element {
                &mut self.#storage_ident
            }
        }

        impl calimero_storage::entities::AtomicUnit for #name {}

        #deserialize_impl

        #serialize_impl
    };

    TokenStream::from(expanded)
}
