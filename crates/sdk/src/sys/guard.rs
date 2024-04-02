#[cfg(not(target_arch = "wasm32"))]
const _: () = {
    compile_error!(
        "Incompatible target architecture, no polyfill available, only wasm32 is supported."
    );
};
