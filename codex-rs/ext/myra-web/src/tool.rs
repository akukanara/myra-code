use std::sync::Arc;

use codex_extension_api::FunctionCallError;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_extension_api::parse_tool_input_schema_without_compaction;
use codex_login::AuthManager;
use codex_login::default_client::create_client;
use codex_tools::JsonToolOutput;
use codex_tools::ToolExposure;
use serde_json::Value as JsonValue;
use serde_json::json;

pub(crate) const SEARCH_TOOL: &str = "web_search";
pub(crate) const FETCH_TOOL: &str = "web_fetch";
pub(crate) const MYRACTX_TOOL: &str = "myractx_search";

/// Long enough for a federated search to poll several engines, short enough
/// that a wedged upstream does not hold a turn open indefinitely.
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Caps what one call can put into the context window. A fetched page is
/// frequently tens of thousands of characters, and an agent that spends its
/// whole budget on one article cannot then do anything with it.
const DEFAULT_MAX_CHARACTERS: u64 = 20_000;

#[derive(Clone)]
pub(crate) struct GatewayWeb {
    pub(crate) base_url: String,
    pub(crate) auth_manager: Arc<AuthManager>,
}

impl GatewayWeb {
    /// POST a JSON body to a gateway path, with the same credential the model
    /// requests carry. Returns the parsed body, or a message the model can act
    /// on -- a tool error is read by the model, so it says what to do next
    /// rather than only what went wrong.
    async fn post(&self, path: &str, body: JsonValue) -> Result<JsonValue, FunctionCallError> {
        let auth = self.auth_manager.auth().await.ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "Not signed in to the gateway. Ask the user to run `myra login`.".to_string(),
            )
        })?;

        let url = format!("{}/{}", self.base_url.trim_end_matches('/'), path);
        let client = create_client();
        let response = client
            .post(&url)
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .headers(codex_model_provider::auth_provider_from_auth(&auth).to_auth_headers())
            .json(&body)
            .send()
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!("Could not reach {url}: {err}"))
            })?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            // The gateway's own error message is the useful part -- "model not
            // defined", "402 out of Shard" -- so it is passed through rather
            // than replaced with a status code.
            let detail = serde_json::from_str::<JsonValue>(&text)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .or_else(|| v.pointer("/error"))
                        .map(|m| m.as_str().unwrap_or(&m.to_string()).to_string())
                })
                .unwrap_or_else(|| text.chars().take(400).collect());
            return Err(FunctionCallError::RespondToModel(format!(
                "{path} failed ({status}): {detail}"
            )));
        }

        serde_json::from_str(&text).map_err(|err| {
            FunctionCallError::RespondToModel(format!("{path} returned unreadable JSON: {err}"))
        })
    }
}

fn arguments(call: &ToolCall) -> Result<JsonValue, FunctionCallError> {
    serde_json::from_str(call.function_arguments()?).map_err(|err| {
        FunctionCallError::RespondToModel(format!("arguments were not valid JSON: {err}"))
    })
}

fn required_str(args: &JsonValue, key: &str) -> Result<String, FunctionCallError> {
    args.get(key)
        .and_then(JsonValue::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| FunctionCallError::RespondToModel(format!("`{key}` is required")))
}

// ── web_search ───────────────────────────────────────────────────────────────

pub(crate) struct WebSearchTool {
    pub(crate) gateway: GatewayWeb,
    /// Which search model to call when the model does not name one. Defaults
    /// to the built-in SearXNG provider, which needs no API key.
    pub(crate) default_model: String,
}

fn search_schema() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "What to search for." },
            "max_results": {
                "type": "integer",
                "description": "How many results to return. Defaults to 5.",
                "minimum": 1,
                "maximum": 25
            },
            "search_type": {
                "type": "string",
                "description":
                    "Which slice of the web to search. `science` reaches paper archives and \
                     `code` reaches programming sites, which plain web search often buries.",
                "enum": ["web", "news", "science", "code", "images", "videos", "files", "social", "map"]
            },
            "engines": {
                "type": "array",
                "items": { "type": "string" },
                "description":
                    "Specific backends to query, e.g. [\"arxiv\"] for papers or \
                     [\"github\",\"stackoverflow\"] for code. Omit to use the defaults."
            },
            "model": {
                "type": "string",
                "description": "Search provider to use. Omit unless the user names one."
            }
        },
        "required": ["query"],
        "additionalProperties": false
    })
}

impl ToolExecutor<ToolCall> for WebSearchTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(SEARCH_TOOL)
    }

    fn spec(&self) -> ToolSpec {
        let parameters = parse_tool_input_schema_without_compaction(&search_schema())
            .expect("web_search schema should parse");
        ToolSpec::Function(ResponsesApiTool {
            name: SEARCH_TOOL.to_string(),
            description:
                "Search the web and get back titles, URLs and snippets. Use it for anything \
                 that happened after training, for a fact worth checking, or to find a page \
                 to read with web_fetch. Prefer search_type=science for papers and \
                 search_type=code for programming questions."
                    .to_string(),
            strict: false,
            defer_loading: None,
            output_schema: None,
            parameters,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn handle(&self, call: ToolCall) -> codex_extension_api::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args = arguments(&call)?;
            let mut body = json!({
                "model": args.get("model").and_then(JsonValue::as_str)
                    .unwrap_or(&self.default_model),
                "query": required_str(&args, "query")?,
                "max_results": args.get("max_results").and_then(JsonValue::as_u64).unwrap_or(5),
            });
            if let Some(search_type) = args.get("search_type").and_then(JsonValue::as_str) {
                body["search_type"] = json!(search_type);
            }
            if let Some(engines) = args.get("engines").filter(|v| v.is_array()) {
                body["provider_options"] = json!({ "engines": engines });
            }

            let response = self.gateway.post("search", body).await?;
            // Only the fields a model can use. The raw response carries
            // per-result scores, favicons, citation objects and timing, none of
            // which help it answer and all of which cost context.
            let results: Vec<JsonValue> = response
                .get("results")
                .and_then(JsonValue::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            json!({
                                "title": item.get("title").cloned().unwrap_or(JsonValue::Null),
                                "url": item.get("url").cloned().unwrap_or(JsonValue::Null),
                                "snippet": item.get("snippet").cloned().unwrap_or(JsonValue::Null),
                                "published_at": item.get("published_at").cloned()
                                    .unwrap_or(JsonValue::Null),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();

            if results.is_empty() {
                return Ok(Box::new(JsonToolOutput::new(json!({
                    "results": [],
                    "note": "No results. Try different wording, or a different search_type.",
                }))) as Box<dyn ToolOutput>);
            }
            Ok(
                Box::new(JsonToolOutput::new(json!({ "results": results })).with_external_context())
                    as Box<dyn ToolOutput>,
            )
        })
    }
}

// ── web_fetch ────────────────────────────────────────────────────────────────

pub(crate) struct WebFetchTool {
    pub(crate) gateway: GatewayWeb,
    /// The gateway's built-in fetcher: no API key, no per-page cost.
    pub(crate) default_model: String,
}

fn fetch_schema() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "url": { "type": "string", "description": "Absolute http(s) URL of the page to read." },
            "max_characters": {
                "type": "integer",
                "description":
                    "Truncate the page to this many characters. Defaults to 20000. Raise it \
                     only when the answer is likely past that point.",
                "minimum": 500
            },
            "format": {
                "type": "string",
                "description": "markdown keeps headings, links and code blocks; text drops them.",
                "enum": ["markdown", "text"]
            },
            "model": {
                "type": "string",
                "description": "Fetch provider to use. Omit unless the user names one."
            }
        },
        "required": ["url"],
        "additionalProperties": false
    })
}

impl ToolExecutor<ToolCall> for WebFetchTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(FETCH_TOOL)
    }

    fn spec(&self) -> ToolSpec {
        let parameters = parse_tool_input_schema_without_compaction(&fetch_schema())
            .expect("web_fetch schema should parse");
        ToolSpec::Function(ResponsesApiTool {
            name: FETCH_TOOL.to_string(),
            description:
                "Read a web page as markdown, with the navigation and ads stripped out. Use it \
                 on a URL from web_search or one the user gave you. It does not run JavaScript, \
                 so a page that renders client-side may come back nearly empty -- the result \
                 says so when that happens."
                    .to_string(),
            strict: false,
            defer_loading: None,
            output_schema: None,
            parameters,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn handle(&self, call: ToolCall) -> codex_extension_api::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args = arguments(&call)?;
            let body = json!({
                "model": args.get("model").and_then(JsonValue::as_str)
                    .unwrap_or(&self.default_model),
                "url": required_str(&args, "url")?,
                "format": args.get("format").and_then(JsonValue::as_str).unwrap_or("markdown"),
                "max_characters": args.get("max_characters").and_then(JsonValue::as_u64)
                    .unwrap_or(DEFAULT_MAX_CHARACTERS),
            });

            let response = self.gateway.post("web/fetch", body).await?;
            let text = response
                .pointer("/content/text")
                .and_then(JsonValue::as_str)
                .unwrap_or_default();

            let mut output = json!({
                "url": response.get("url").cloned().unwrap_or(JsonValue::Null),
                "title": response.get("title").cloned().unwrap_or(JsonValue::Null),
                "content": text,
            });
            // The "this page needs JavaScript" warning is the difference
            // between the model retrying another way and it concluding the page
            // is empty, so it is forwarded rather than dropped.
            if let Some(warnings) = response.get("warnings").filter(|v| v.is_array()) {
                output["warnings"] = warnings.clone();
            }
            Ok(
                Box::new(JsonToolOutput::new(output).with_external_context())
                    as Box<dyn ToolOutput>,
            )
        })
    }
}

// ── myractx_search ─────────────────────────────────────────────────────────

/// Version-aware coding knowledge collected by MyraRouter. This is separate
/// from web search: it prefers already-indexed documentation and refreshes it
/// only when the gateway has no relevant knowledge yet.
pub(crate) struct MyraCtxTool {
    pub(crate) gateway: GatewayWeb,
}

fn myractx_schema() -> JsonValue {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "The implementation question or API behavior to look up."
            },
            "library": {
                "type": "string",
                "description": "Library name or MyraCtx library id, for example `next.js`, `react`, or `/vercel/next.js`."
            }
        },
        "required": ["query", "library"],
        "additionalProperties": false
    })
}

impl ToolExecutor<ToolCall> for MyraCtxTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(MYRACTX_TOOL)
    }

    fn spec(&self) -> ToolSpec {
        let parameters = parse_tool_input_schema_without_compaction(&myractx_schema())
            .expect("myractx_search schema should parse");
        ToolSpec::Function(ResponsesApiTool {
            name: MYRACTX_TOOL.to_string(),
            description: "Look up current, version-aware coding documentation from MyraCtx. Use this before web search when the task concerns a library, framework, SDK, API, CLI, or cloud service. Provide the library name or its MyraCtx id and a focused implementation question.".to_string(),
            strict: false,
            defer_loading: None,
            output_schema: None,
            parameters,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn handle(&self, call: ToolCall) -> codex_extension_api::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args = arguments(&call)?;
            let response = self
                .gateway
                .post(
                    "myractx/search",
                    json!({
                        "query": required_str(&args, "query")?,
                        "libraryId": required_str(&args, "library")?,
                    }),
                )
                .await?;
            let output = json!({
                "status": response.get("status").cloned().unwrap_or(JsonValue::Null),
                "answer": response.get("answer").cloned().unwrap_or(JsonValue::Null),
                "references": response.get("references").cloned().unwrap_or(JsonValue::Array(vec![])),
            });
            Ok(
                Box::new(JsonToolOutput::new(output).with_external_context())
                    as Box<dyn ToolOutput>,
            )
        })
    }
}
