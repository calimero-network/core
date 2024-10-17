use calimero_sdk::app;
use calimero_sdk::borsh::to_vec;
use calimero_storage::address::{Id, Path};
use calimero_storage::entities::{Data, Element};
use calimero_storage::integration::Comparison;
use calimero_storage::interface::StorageError::ActionNotAllowed;
use calimero_storage::interface::{Action, Interface, StorageError};
use calimero_storage_macros::{AtomicUnit, Collection};

#[app::state(emits = for<'a> Event<'a>)]
#[derive(AtomicUnit, Clone, Debug, PartialEq, PartialOrd)]
#[root]
#[type_id(11)]
pub struct Library {
    #[collection]
    books: Books,
    #[storage]
    storage: Element,
}

#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Book)]
pub struct Books;

#[derive(AtomicUnit, Clone, Debug, PartialEq, PartialOrd)]
#[type_id(12)]
pub struct Book {
    authors: Vec<String>,
    isbn: String,
    publisher: String,
    year: u16,
    rating: f32,
    #[collection]
    reviews: Reviews,
    #[collection]
    pages: Pages,
    #[storage]
    storage: Element,
}

#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Page)]
pub struct Pages;

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(13)]
pub struct Page {
    content: String,
    number: u16,
    title: String,
    #[collection]
    paragraphs: Paragraphs,
    #[storage]
    storage: Element,
}

#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Paragraph)]
pub struct Paragraphs;

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(14)]
pub struct Paragraph {
    content: String,
    #[storage]
    storage: Element,
}

#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Review)]
pub struct Reviews;

#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(15)]
pub struct Review {
    author: String,
    content: String,
    rating: u8,
    #[storage]
    storage: Element,
}

#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
    Removed { key: &'a str },
    Cleared,
}

#[app::logic]
impl Library {
    #[app::init]
    pub fn init() -> Library {
        Library {
            books: Books {},
            storage: Element::new(&Path::new("::library").unwrap()),
        }
    }

    pub fn apply_action(&self, action: Action) -> Result<Option<Id>, StorageError> {
        match action {
            Action::Add { type_id, .. } | Action::Update { type_id, .. } => {
                // TODO: This is long-hand - it will be put inside an enum and generated
                // TODO: with a macro
                match type_id {
                    11 => Interface::apply_action::<Library>(action),
                    12 => Interface::apply_action::<Book>(action),
                    13 => Interface::apply_action::<Page>(action),
                    14 => Interface::apply_action::<Paragraph>(action),
                    15 => Interface::apply_action::<Review>(action),
                    _ => Err(StorageError::UnknownType(type_id)),
                }
            }
            Action::Delete { .. } => Interface::apply_action::<Library>(action),
            Action::Compare { .. } => Err(ActionNotAllowed("Compare".to_owned())),
        }
    }

    pub fn compare_trees(
        &self,
        comparison: Comparison,
    ) -> Result<(Vec<Action>, Vec<Action>), StorageError> {
        fn instantiate<D: Data>(data: &[u8]) -> Result<D, StorageError> {
            D::try_from_slice(data).map_err(StorageError::DeserializationError)
        }
        let Comparison {
            type_id,
            data,
            comparison_data,
        } = comparison;
        match type_id {
            11 => Interface::compare_trees(&instantiate::<Library>(&data)?, &comparison_data),
            12 => Interface::compare_trees(&instantiate::<Book>(&data)?, &comparison_data),
            13 => Interface::compare_trees(&instantiate::<Page>(&data)?, &comparison_data),
            14 => Interface::compare_trees(&instantiate::<Paragraph>(&data)?, &comparison_data),
            15 => Interface::compare_trees(&instantiate::<Review>(&data)?, &comparison_data),
            _ => Err(StorageError::UnknownType(type_id)),
        }
    }

    pub fn generate_comparison_data(&self, id: Id) -> Result<Comparison, StorageError> {
        fn generate_for<D: Data>(id: Id) -> Result<Comparison, StorageError> {
            let data = Interface::find_by_id::<D>(id)?.ok_or(StorageError::NotFound(id))?;
            Ok(Comparison {
                type_id: D::type_id(),
                data: to_vec(&data).map_err(StorageError::SerializationError)?,
                comparison_data: Interface::generate_comparison_data(&data)?,
            })
        }
        let type_id = Interface::type_of(id)?;
        match type_id {
            11 => generate_for::<Library>(id),
            12 => generate_for::<Book>(id),
            13 => generate_for::<Page>(id),
            14 => generate_for::<Paragraph>(id),
            15 => generate_for::<Review>(id),
            _ => Err(StorageError::UnknownType(type_id)),
        }
    }
}
