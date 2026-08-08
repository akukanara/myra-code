//! Web search and page fetching, served by the gateway the CLI is already
//! signed in to.
//!
//! The `web` namespace next door talks to the hosted backend and is only
//! available to an account on it. Against a MyraRouter gateway it is not, and
//! the agent is left with no way to read the internet at all -- it can only
//! shell out to curl and hope the page is readable. These two tools close that:
//! `web_search` maps onto the gateway's `/v1/search`, `web_fetch` onto
//! `/v1/web/fetch`, both with the credential the model requests already use.

mod extension;
mod tool;

pub use extension::install;
