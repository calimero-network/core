use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::{env, fs};

use rust_embed::RustEmbed;

use crate::cli::NewCommand;

#[derive(RustEmbed)]
#[folder = "../../apps/app-template/"]
struct AppTemplate;

pub async fn run(args: NewCommand) -> eyre::Result<()> {
    println!("Creating a new project {:?} from template...", args.name);
    let path = args.name;

    fs::create_dir_all(&path)?;
    env::set_current_dir(path)?;

    for file in AppTemplate::iter() {
        let path = Path::new(&*file);

        // Create directories if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file_handle = File::create(path)?;
        if let Some(file_content) = AppTemplate::get(&file) {
            file_handle.write_all(&file_content.data)?;
        }
    }

    Ok(())
}
