use camino::Utf8PathBuf;
use eyre::{Context, Result};
use std::fs;

pub trait FileOperations {
    fn read_file(&self, file_name: &str) -> Result<String>;
    fn write_file(&self, file_name: &str, content: &str) -> Result<()>;
    fn contains_file(&self, file_name: &str) -> bool;
    fn debug_location(&self) -> &Utf8PathBuf;
    fn exists(&self) -> bool;
}

pub struct FileManager {
    pub base_dir: Utf8PathBuf,
}

impl FileManager {
    pub fn new(base_dir: Utf8PathBuf) -> Result<Self> {
        if !base_dir.exists() {
            // if default_chat_dir(base_dir) {
            //     fs::create_dir_all(&base_dir)
            // } else {
            //     fs::create_dir(&base_dir)
            // }
            fs::create_dir_all(&base_dir)
                .wrap_err_with(|| format!("failed to create directory {:?}", base_dir))?;
        }

        Ok(FileManager { base_dir })
    }
    
}

impl FileOperations for FileManager {

    fn read_file(&self, file_name: &str) -> Result<String> {
        let file_path = self.base_dir.join(file_name);
        fs::read_to_string(&file_path)
            .wrap_err_with(|| format!("Failed to read file at {:?}", &file_path))
    }

    fn write_file(&self, file_name: &str, content: &str) -> Result<()> {
        let file_path = self.base_dir.join(file_name);
        fs::write(&file_path, content)
            .wrap_err_with(|| format!("Failed to write file at {:?}", &file_path))
    }

    fn contains_file(&self, file_name: &str) -> bool {
        self.base_dir.join(file_name).is_file()
    }

    fn debug_location(&self) -> &Utf8PathBuf {
        &self.base_dir
    }

    fn exists(&self) -> bool {
        self.base_dir.exists()
    }
}


pub const DEFAULT_CALIMERO_CHAT_HOME: &str = ".calimero/experiments/chat-p0c";

// pub fn is_default_chat_dir(home: Utf8PathBuf) -> bool {
//     if let Some(home) = dirs::home_dir() {
//         let home = camino::Utf8Path::from_path(&home).expect("invalid home directory");
//         return home.join(DEFAULT_CALIMERO_CHAT_HOME);
//     }

//     Default::default()
// }