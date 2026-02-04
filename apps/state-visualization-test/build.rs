fn main() {
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let app_abi = calimero_wasm_abi::get_abi::<state_visualization_test::VisualizationTest>();
    let state_abi = calimero_wasm_abi::get_state_abi::<state_visualization_test::VisualizationTest>();

    let abi_json = serde_json::to_string_pretty(&app_abi).expect("Failed to serialize ABI");
    let state_json = serde_json::to_string_pretty(&state_abi).expect("Failed to serialize state");

    std::fs::write(format!("{}/abi.json", out_dir), abi_json).expect("Failed to write ABI");
    std::fs::write(format!("{}/state.json", out_dir), state_json).expect("Failed to write state");
}
