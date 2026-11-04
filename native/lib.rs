pub mod audio;
pub mod incoming;
pub mod lyrics;
pub mod transfer;
#[cfg(target_arch = "wasm32")]
wit_bindgen::generate!({
    path:"wit",world:"psys-world-v3",generate_all,
    // V3 exports are synchronous functions returning future<T>. Host waits
    // execute through block_on inside that call, without an async-lift export.
});
#[cfg(target_arch = "wasm32")]
mod plugin;
#[cfg(target_arch = "wasm32")]
use plugin::Plugin;
#[cfg(target_arch = "wasm32")]
export!(Plugin);
