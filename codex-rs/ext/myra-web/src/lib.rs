//! Gateway tools served by the MyraRouter account the CLI is already signed in
//! to.
//!
//! The `web` namespace next door talks to the hosted backend and is only
//! available to an account on it. Against a MyraRouter gateway it is not, and
//! the agent is left with no way to read the internet at all -- it can only
//! shell out to curl and hope the page is readable. The tools close that gap:
//! `web_search` maps onto the gateway's `/v1/search`, `web_fetch` onto
//! `/v1/web/fetch`, and `myractx_search` onto `/v1/myractx/search`, all with
//! the credential the model requests already use.

mod extension;
mod tool;

pub use extension::install;
