use color_eyre::owo_colors::OwoColorize;
use inquire::{InquireError, Password, Text};
use std::process;

use crate::storage::save_keys_to_storage;

pub fn handle_login_result(result: Result<&str, InquireError>) {
    match result {
        Ok(selected_option) => {
            if selected_option == "Browser Login" {
                let _ = open::that("https://www.mynearwallet.com/");
            } else if selected_option == "CLI Login" {
                let account_id = Text::new("Account ID:").prompt().unwrap_or_else(|_| {
                    println!("Error: Not a valid account ID!");
                    process::exit(1);
                });

                let private_key = Password::new("Account private key:").prompt().unwrap_or_else(|_| {
                    println!("Error: Not a valid private key!");
                    process::exit(1);
                });
                match save_keys_to_storage(&account_id, &private_key, &String::new()) {
                    Ok(()) => println!("Logged in with: {}", account_id.green()),
                    Err(err) => eprintln!("Failed to login: {}", err),
                }
            }
        }
        Err(err) => {
            println!("Error: {:?}", err);
        }
    }
}

pub fn cli_login() {
    println!("Adding raw keypair");
    let account_id = Text::new("Account ID:").prompt().unwrap_or_else(|_| {
        println!("Error: Not a valid account ID!");
        process::exit(1);
    });

    let public_key = Text::new("Account public key:").prompt().unwrap_or_else(|_| {
        println!("Error: Not a valid public key!");
        process::exit(1);
    });

    let private_key = Password::new("Account private key:").prompt().unwrap_or_else(|_| {
        println!("Error: Not a valid private key!");
        process::exit(1);
    });

    match save_keys_to_storage(&account_id, &private_key, &public_key) {
        Ok(()) => println!("Keys saved successfully."),
        Err(err) => eprintln!("Error saving keys: {}", err),
    }
}