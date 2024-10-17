#![allow(unused_crate_dependencies, reason = "Not actually unused")]

use std::env;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use borsh::BorshSerialize;
use calimero_runtime::errors::FunctionCallError;
use calimero_runtime::logic::{VMContext, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::{run, Constraint};
use calimero_storage::address::{Id, Path as EntityPath};
use calimero_storage::entities::{ChildInfo, Element};
use calimero_storage::interface::Action;
use eyre::Result as EyreResult;
use owo_colors::OwoColorize;
use serde_json::json;

fn main() -> EyreResult<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        println!("Usage: {args:?} <path-to-wasm>");
        return Ok(());
    }

    let path = &args[1];
    let path = Path::new(path);
    if !path.exists() {
        eyre::bail!("KV wasm file not found");
    }

    let file = File::open(path)?.bytes().collect::<Result<Vec<u8>, _>>()?;

    let mut storage = InMemoryStorage::default();

    let limits = VMLimits::new(
        /*max_stack_size:*/ 200 << 10, // 200 KiB
        /*max_memory_pages:*/ 1 << 10, // 1 KiB
        /*max_registers:*/ 100,
        /*max_register_size:*/ (100 << 20).validate()?, // 100 MiB
        /*max_registers_capacity:*/ 1 << 30, // 1 GiB
        /*max_logs:*/ 100,
        /*max_log_size:*/ 16 << 10, // 16 KiB
        /*max_events:*/ 100,
        /*max_event_kind_size:*/ 100,
        /*max_event_data_size:*/ 16 << 10, // 16 KiB
        /*max_storage_key_size:*/ (1 << 20).try_into()?, // 1 MiB
        /*max_storage_value_size:*/ (10 << 20).try_into()?, // 10 MiB
    );

    let cx = VMContext::new(Vec::new(), [0; 32]);

    let _outcome = run(&file, "init", cx, &mut storage, &limits)?;
    // dbg!(&outcome);

    // Set up Library
    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Setting up Library".bold());
    println!("{}", "--".repeat(20).dimmed());

    #[derive(BorshSerialize)]
    pub struct Library {
        books: Books,
        storage: Element,
    }
    #[derive(BorshSerialize)]
    pub struct Books;

    // let library_id: [u8; 16] =
    //     hex::decode("deadbeef-1122-3344-5566-778899aabb01".replace("-", ""))?
    //         .as_slice()
    //         .try_into()?;
    // let library_data = json!({
    //     "books": [],
    //     "storage": {
    //         "id": hex::encode(library_id),
    //         "is_dirty": false,
    //         "merkle_hash": hex::encode([0; 32]),
    //         "metadata": {
    //             "created_at": 0,
    //             "updated_at": 0
    //         },
    //         "path": "::library"
    //     }
    // });
    let library_data = Library {
        books: Books {},
        storage: Element::new(&EntityPath::new("::library")?),
    };
    let library_id = library_data.storage.id();

    let action = Action::Add {
        id: Id::from(library_id),
        type_id: 11,
        // data: serde_json::to_vec(&library_data)?,
        data: borsh::to_vec(&library_data)?,
        ancestors: Vec::new(),
    };
    let serialized_action = serde_json::to_string(&action)?;
    let input = std::fmt::format(format_args!("{{\"action\": {}}}", serialized_action));
    println!("Input: {}", input);

    println!("Action: {serialized_action}");
    match run(
        &file,
        "apply_action",
        VMContext::new(input.as_bytes().to_owned(), [0; 32]),
        &mut storage,
        &limits,
    ) {
        Ok(outcome) => {
            // dbg!(&outcome);
            match outcome.returns {
                Ok(returns) => {
                    println!("Outcome: {}", String::from_utf8_lossy(&returns.unwrap()));
                }
                Err(err) => match err {
                    FunctionCallError::ExecutionError(data) => {
                        println!("ExecutionError: {}", String::from_utf8_lossy(&data))
                    }
                    _ => {
                        println!("Other error: {err}");
                    }
                },
            }
        }
        Err(err) => {
            println!("Error: {err}");
        }
    }

    // Add a Book
    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Adding Book".bold());
    println!("{}", "--".repeat(20).dimmed());

    // let book_id: [u8; 16] = hex::decode("deadbeef-1122-3344-5566-778899aabb02".replace("-", ""))?
    //     .as_slice()
    //     .try_into()?;
    // let book_data = json!({
    //     "authors": ["John Doe"],
    //     "isbn": "1234567890",
    //     "publisher": "Example Publishing",
    //     "year": 2023,
    //     "rating": 4.5,
    //     "reviews": [],
    //     "pages": [],
    //     "storage": {
    //         "id": hex::encode(book_id),
    //         "is_dirty": false,
    //         "merkle_hash": hex::encode([0; 32]),
    //         "metadata": {
    //             "created_at": 0,
    //             "updated_at": 0
    //         },
    //         "path": "::library::books::1"
    //     }
    // });
    #[derive(BorshSerialize)]
    pub struct Book {
        authors: Vec<String>,
        isbn: String,
        publisher: String,
        year: u16,
        rating: f32,
        reviews: Reviews,
        pages: Pages,
        storage: Element,
    }
    #[derive(BorshSerialize)]
    pub struct Pages;
    #[derive(BorshSerialize)]
    pub struct Reviews;

    let book_data = Book {
        authors: vec!["John Doe".to_owned()],
        isbn: "1234567890".to_owned(),
        publisher: "Example Publishing".to_owned(),
        year: 2023,
        rating: 4.5,
        reviews: Reviews {},
        pages: Pages {},
        storage: Element::new(&EntityPath::new("::library::books::1")?),
    };
    let book_id = book_data.storage.id();
    let add_book_action = Action::Add {
        id: Id::from(book_id),
        type_id: 12,
        // data: serde_json::to_vec(&book_data)?,
        data: borsh::to_vec(&book_data)?,
        ancestors: vec![ChildInfo::new(Id::from(library_id), [0; 32])],
    };

    let serialized_add_book_action = serde_json::to_string(&add_book_action)?;
    let input = std::fmt::format(format_args!(
        "{{\"action\": {}}}",
        serialized_add_book_action
    ));
    println!("Input: {}", input);

    println!("Action: {serialized_add_book_action}");
    match run(
        &file,
        "apply_action",
        VMContext::new(input.as_bytes().to_owned(), [0; 32]),
        &mut storage,
        &limits,
    ) {
        Ok(outcome) => {
            // dbg!(&outcome);
            match outcome.returns {
                Ok(returns) => {
                    println!("Outcome: {}", String::from_utf8_lossy(&returns.unwrap()));
                }
                Err(err) => match err {
                    FunctionCallError::ExecutionError(data) => {
                        println!("ExecutionError: {}", String::from_utf8_lossy(&data))
                    }
                    _ => {
                        println!("Other error: {err}");
                    }
                },
            }
        }
        Err(err) => {
            println!("Error: {err}");
        }
    }

    println!("{}", "--".repeat(20).dimmed());
    println!("{:>35}", "Now, let's inspect the storage".bold());
    println!("{}", "--".repeat(20).dimmed());

    dbg!(storage);

    Ok(())
}
