use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

/// A page read through the gateway's fetcher.
///
/// Separate from [`crate::web_search::WebSearchItem`] because a fetch is a
/// different act with a different useful detail: one URL and the title behind
/// it, rather than a query and a result set.
#[derive(Debug, Clone, Deserialize, Serialize, TS, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct WebFetchItem {
    pub id: String,
    pub url: String,
    /// The page's own title, known only once the fetch returns.
    pub title: Option<String>,
    pub status: WebFetchStatus,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, TS, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub enum WebFetchStatus {
    InProgress,
    Completed,
    Failed,
}
