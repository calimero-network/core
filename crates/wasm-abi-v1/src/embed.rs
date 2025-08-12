use crate::schema::Manifest;
use serde_json;

/// Embed the ABI manifest into the WASM binary as a custom section
/// This macro generates the custom section and exports the get_abi* functions
#[macro_export]
macro_rules! embed_abi {
    ($manifest:expr) => {
        // Serialize the manifest to JSON
        const ABI_JSON: &str = $manifest;
        
        // Create the custom section with the ABI JSON
        #[link_section = "calimero_abi_v1"]
        static ABI: [u8; ABI_JSON.len()] = *ABI_JSON.as_bytes();
        
        // Export functions to access the ABI at runtime
        #[no_mangle]
        pub extern "C" fn get_abi_ptr() -> u32 {
            ABI.as_ptr() as u32
        }
        
        #[no_mangle]
        pub extern "C" fn get_abi_len() -> u32 {
            ABI.len() as u32
        }
        
        #[no_mangle]
        pub extern "C" fn get_abi() -> u32 {
            get_abi_ptr()
        }
    };
}

/// Embed a Manifest struct into the WASM binary
pub fn embed(manifest: &Manifest) -> proc_macro2::TokenStream {
    let json = serde_json::to_string_pretty(manifest)
        .expect("Failed to serialize manifest to JSON");
    
    let json_literal = proc_macro2::Literal::string(&json);
    
    quote::quote! {
        embed_abi!(#json_literal);
    }
}

/// Generate the embed code for a given manifest
pub fn generate_embed_code(manifest: &Manifest) -> String {
    let json = serde_json::to_string_pretty(manifest)
        .expect("Failed to serialize manifest to JSON");
    
    format!(
        "embed_abi!(r#\"{}\"#);",
        json
    )
} 