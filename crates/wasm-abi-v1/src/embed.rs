use serde_json;

use crate::schema::Manifest;

/// Embed the ABI manifest into the WASM binary as a custom section
/// This macro generates the custom section and exports the get_abi* functions
#[macro_export]
macro_rules! embed_abi {
    ($manifest:expr) => {
        // Serialize the manifest to JSON
        const ABI_JSON: &str = $manifest;

        // Create the custom section with the ABI JSON
        #[cfg_attr(target_os = "macos", link_section = "__DATA,calimero_abi_v1")]
        #[cfg_attr(not(target_os = "macos"), link_section = "calimero_abi_v1")]
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
    let json =
        serde_json::to_string_pretty(manifest).expect("Failed to serialize manifest to JSON");

    let json_literal = proc_macro2::Literal::string(&json);

    quote::quote! {
        embed_abi!(#json_literal);
    }
}

/// Generate the embed code for a given manifest
pub fn generate_embed_code(manifest: &Manifest) -> String {
    let json =
        serde_json::to_string_pretty(manifest).expect("Failed to serialize manifest to JSON");
    let json_bytes = json.as_bytes();
    let json_len = json_bytes.len();

    // Create the byte array as a comma-separated list
    let bytes_list = json_bytes
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "// Generated ABI embed code\n// Create the custom section with the ABI JSON\n#[cfg_attr(target_os = \"macos\", link_section = \"__DATA,calimero_abi_v1\")]\n#[cfg_attr(not(target_os = \"macos\"), link_section = \"calimero_abi_v1\")]\nstatic ABI: [u8; {}] = [{}];\n\n// Export functions to access the ABI at runtime\n#[no_mangle]\npub extern \"C\" fn get_abi_ptr() -> u32 {{\n    ABI.as_ptr() as u32\n}}\n\n#[no_mangle]\npub extern \"C\" fn get_abi_len() -> u32 {{\n    ABI.len() as u32\n}}\n\n#[no_mangle]\npub extern \"C\" fn get_abi() -> u32 {{\n    get_abi_ptr()\n}}",
        json_len, bytes_list
    )
}
