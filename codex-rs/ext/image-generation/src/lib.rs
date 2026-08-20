mod artifact;
mod backend;
mod extension;
mod tool;

pub use extension::install;

/// Flat tool name, matching the other MyraRouter gateway tools (`web_search`,
/// `web_fetch`, `myractx_search`). It was a `image_gen`/`imagegen` namespace
/// tool while the hosted backend owned image generation; the gateway serves it
/// as an ordinary client-executed function, so it no longer needs a namespace
/// -- nor a provider that supports namespace tools at all.
pub(crate) const MYRA_IMAGEN_TOOL: &str = "myra_imagen";
