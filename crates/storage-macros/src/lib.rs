use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields, Type};

/// Derives the `AtomicUnit` trait for a struct.
///
/// This macro automatically implements the `AtomicUnit` trait for a struct, which extends the `Data` trait.
///
/// # Requirements
///
/// The following are mandatory requirements for the struct:
///
///   - A private `storage` field of type `Element`.
///     This is needed as the `Data`-based struct needs to own an `Element`.
///
/// # Generated implementations
///
/// This macro will generate the following implementations:
///
///   - `Data` trait implementation.
///   - `AtomicUnit` trait implementation.
///   - Getter and setter methods for each field. These help to ensure that the
///     access to the fields is controlled, and that any changes to the fields
///     are reflected in the `Element`'s state.
///   - [`BorshSerialize`](borsh::BorshSerialize) and [`BorshDeserialize`](borsh::BorshDeserialize)
///     will be implemented for the struct, so they should be omitted from the
///     struct definition.
///
/// # Struct attributes
///
/// * `#[root]`       - Indicates that the type represents a root in the
///                     hierarchy, and doesn't have a parent. This is an
///                     optional attribute.
/// * `#[type_id(n)]` - Indicates the type ID for the struct. This is a
///                     mandatory attribute, and the value `n` must be a `u8`.
///                     This is used to differentiate between different types
///                     of structs when deserialising, and each type should have
///                     a unique ID.
///
/// # Field attributes
///
/// * `#[collection]` - Indicates that the field is a collection of other `Data` types.
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
///                     arbitrary, but the type has to be an `Element`.
///
/// Note that fields marked with `#[private]` or `#[skip]` must have [`Default`]
/// implemented so that they can be initialised when deserialising.
///
/// TODO: The `#[private]` attribute is implemented with the same functionality
///       as `#[skip]`, but in future these will be differentiated.
///
/// # Getters and setters
///
/// The macro will generate getter and setter methods for each field. These
/// methods will allow the struct to control access to its fields, and ensure
/// that any changes to the fields are reflected in the [`Element`](crate::entities::Element)'s
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
/// use crate::entities::Element;
/// use crate::AtomicUnit;
/// use borsh::{BorshSerialize, BorshDeserialize};
///
/// #[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
/// #[type_id(43)]
/// struct Page {
///     title: String,
///     #[private]
///     secret: String,
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
///   - The `#[storage]` attribute is missing or invalid
///   - The `#[type_id(n)]` attribute is missing or invalid
///
/// # See also
///
/// * [`Collection`] - For defining a collection of child elements, for use with
///                    [`AtomicUnit`] (the parent and children are all atomic
///                    units, with the collection being the grouping mechanism
///                    at field level on the parent).
///
#[expect(
    clippy::too_many_lines,
    reason = "Okay for now - will be restructured later"
)]
#[proc_macro_derive(
    AtomicUnit,
    attributes(children, collection, private, root, skip, storage, type_id)
)]
pub fn atomic_unit_derive(input: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let where_clause = input.generics.make_where_clause().clone();
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();

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

    let collection_fields: Vec<_> = named_fields
        .iter()
        .filter(|f| {
            f.attrs
                .iter()
                .any(|attr| attr.path().is_ident("collection"))
        })
        .map(|f| f.ident.as_ref().unwrap())
        .collect();

    let mut serde_where_clause = where_clause.clone();

    for ty in input.generics.type_params() {
        let ident = &ty.ident;
        serde_where_clause.predicates.push(syn::parse_quote!(
            #ident: calimero_sdk::borsh::BorshSerialize
                + calimero_sdk::borsh::BorshDeserialize
        ));
    }

    let expanded = quote! {
        impl #impl_generics calimero_storage::entities::Data for #name #ty_generics #serde_where_clause {
            fn collections(&self) -> std::collections::BTreeMap<String, Vec<calimero_storage::entities::ChildInfo>> {
                use calimero_storage::entities::Collection;
                let mut collections = std::collections::BTreeMap::new();
                #(
                    collections.insert(
                        stringify!(#collection_fields).to_owned(),
                        calimero_storage::interface::MainInterface::child_info_for(self.id(), &self.#collection_fields).unwrap_or_default()
                    );
                )*
                collections
            }

            fn element(&self) -> &calimero_storage::entities::Element {
                &self.#storage_ident
            }

            fn element_mut(&mut self) -> &mut calimero_storage::entities::Element {
                &mut self.#storage_ident
            }
        }

        impl #impl_generics calimero_storage::entities::AtomicUnit for #name #ty_generics #serde_where_clause {}
    };

    TokenStream::from(quote! {
        #[allow(unused_mut)]
        const _: () = {
            #expanded
        };
    })
}

/// Derives the [`Collection`](crate::entities::Collection) trait for
/// a struct.
///
/// This macro will automatically implement the [`Collection`](crate::entities::Collection)
/// trait for the struct it's applied to.
///
/// # Requirements
///
/// The following are mandatory requirements for the struct:
///
///   - A `#[children(Type)]` attribute must be present, where `Type` is the
///     type of the children that this collection will contain.
///
/// # Generated implementations
///
/// This macro will generate the following implementations:
///
///   - [`Collection`](crate::entities::Collection) trait implementation.
///   - [`Default`] trait implementation.
///   - [`BorshSerialize`](borsh::BorshSerialize) and [`BorshDeserialize`](borsh::BorshDeserialize)
///     implementations.
///
/// # Examples
///
/// ```
/// use calimero_storage::entities::Collection;
///
/// #[derive(Collection)]
/// #[children(Page)]
/// struct Pages {
///     content: String,
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
///   - The `#[children(Type)]` attribute is missing or invalid
///
/// # See also
///
/// * [`AtomicUnit`] - For defining a single atomic unit of data that either
///                    stands alone, or owns one or more collections, or is a
///                    child in a collection.
#[proc_macro_derive(Collection, attributes(children))]
pub fn collection_derive(input: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let where_clause = input.generics.make_where_clause().clone();
    let (impl_generics, ty_generics, _) = input.generics.split_for_impl();
    let child_type = input
        .attrs
        .iter()
        .find(|attr| attr.path().is_ident("children"))
        .and_then(|attr| attr.parse_args::<Type>().ok())
        .expect("Collection derive requires #[children(Type)] attribute");

    let fields = match input.data {
        Data::Struct(data) => data.fields,
        Data::Enum(_) | Data::Union(_) => panic!("Collection can only be derived for structs"),
    };

    let deserialize_impl = quote! {
        impl #impl_generics calimero_sdk::borsh::BorshDeserialize for #name #ty_generics #where_clause {
            fn deserialize_reader<R: std::io::Read>(reader: &mut R) -> std::io::Result<Self> {
                Ok(Self::default())
            }
        }
    };

    let serialize_impl = quote! {
        impl #impl_generics calimero_sdk::borsh::BorshSerialize for #name #ty_generics #where_clause {
            fn serialize<W: std::io::Write>(&self, writer: &mut W) -> std::io::Result<()> {
                Ok(())
            }
        }
    };

    let data = fields
        .iter()
        .map(|field| {
            let ident = field.ident.as_ref().unwrap();
            quote! { #ident: Default::default(), }
        })
        .collect::<Vec<_>>();

    let data = (!data.is_empty()).then(|| quote! { { #(#data)* } });

    let default_impl = quote! {
        impl #impl_generics ::core::default::Default for #name #ty_generics {
            fn default() -> Self {
                Self #data
            }
        }
    };

    let mut collection_where_clause = where_clause;

    for ty in input.generics.type_params() {
        let ident = &ty.ident;
        collection_where_clause.predicates.push(syn::parse_quote!(
            #ident: calimero_sdk::borsh::BorshSerialize
                + calimero_sdk::borsh::BorshDeserialize
        ));
    }

    let expanded = quote! {
        impl #impl_generics calimero_storage::entities::Collection for #name #ty_generics #collection_where_clause {
            type Child = #child_type;

            fn name(&self) -> &str {
                stringify!(#name)
            }
        }

        #default_impl

        #deserialize_impl

        #serialize_impl
    };

    TokenStream::from(expanded)
}
