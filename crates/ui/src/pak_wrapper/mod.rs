#[cfg(target_family = "wasm")]
#[path = "wasm.rs"]
mod wrapper;

#[cfg(not(target_family = "wasm"))]
#[path = "native.rs"]
mod wrapper;

pub use wrapper::*;
