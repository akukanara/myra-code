use std::sync::Arc;
use std::time::Duration;

use codex_core::config::Config;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::default_client::create_client;
use serde::Deserialize;

use crate::tool::GatewayWeb;
use crate::tool::MyraCtxTool;
use crate::tool::WebFetchTool;
use crate::tool::WebSearchTool;

const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct MyraWebExtension {
    auth_manager: Arc<AuthManager>,
}

/// Resolved once per thread. `tools()` is handed the stores, not the Config,
/// so whatever it needs from configuration has to be put there first.
#[derive(Clone)]
struct MyraWebConfig {
    base_url: Option<String>,
    search_model: Option<String>,
    fetch_model: Option<String>,
}

impl MyraWebConfig {
    /// Resolve the authenticated gateway endpoint and its plan-aware web tool
    /// catalog. A static provider id would bypass the router's model contract
    /// and fail whenever an operator names a different web-model combo.
    async fn from_auth(config: &Config, auth: Option<&CodexAuth>) -> Self {
        let base_url = config
            .model_provider
            .to_api_provider(auth.map(CodexAuth::api_auth_mode))
            .ok()
            .map(|provider| provider.base_url)
            .filter(|url| !url.trim().is_empty());
        let (search_model, fetch_model) = match (&base_url, auth) {
            (Some(base_url), Some(auth)) => discover_web_models(base_url, auth).await,
            _ => (None, None),
        };
        Self {
            base_url,
            search_model,
            fetch_model,
        }
    }
}

#[derive(Deserialize)]
struct WebModelsResponse {
    data: Vec<WebModel>,
}

#[derive(Deserialize)]
struct WebModel {
    id: String,
    kind: String,
}

async fn discover_web_models(base_url: &str, auth: &CodexAuth) -> (Option<String>, Option<String>) {
    let url = format!("{}/models/web", base_url.trim_end_matches('/'));
    let response = match create_client()
        .get(&url)
        .timeout(MODEL_CATALOG_TIMEOUT)
        .headers(codex_model_provider::auth_provider_from_auth(auth).to_auth_headers())
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::debug!(status = %response.status(), "myra-web: web model catalog unavailable");
            return (None, None);
        }
        Err(error) => {
            tracing::debug!(%error, "myra-web: could not load web model catalog");
            return (None, None);
        }
    };
    let catalog = match response.json::<WebModelsResponse>().await {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::debug!(%error, "myra-web: invalid web model catalog");
            return (None, None);
        }
    };
    let mut search_model = None;
    let mut fetch_model = None;
    for model in catalog.data {
        match model.kind.as_str() {
            "webSearch" if search_model.is_none() => search_model = Some(model.id),
            "webFetch" if fetch_model.is_none() => fetch_model = Some(model.id),
            _ => {}
        }
    }
    (search_model, fetch_model)
}

impl ThreadLifecycleContributor<Config> for MyraWebExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let auth = self.auth_manager.auth().await;
            input
                .thread_store
                .insert(MyraWebConfig::from_auth(input.config, auth.as_ref()).await);
        })
    }
}

impl ToolContributor for MyraWebExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        // No base URL means no gateway to ask, so the tools are not offered at
        // all. Advertising one that always fails is worse than not having it:
        // the model will keep reaching for it and keep getting an error.
        let Some(base_url) = thread_store
            .get::<MyraWebConfig>()
            .and_then(|config| config.base_url.clone())
        else {
            tracing::debug!("myra-web: no provider base URL; web tools not offered");
            return Vec::new();
        };

        let gateway = GatewayWeb {
            base_url,
            auth_manager: self.auth_manager.clone(),
        };
        let mut tools: Vec<Arc<dyn ToolExecutor<ToolCall>>> = vec![Arc::new(MyraCtxTool {
            gateway: gateway.clone(),
        })];
        if let Some(default_model) = thread_store
            .get::<MyraWebConfig>()
            .and_then(|config| config.search_model.clone())
        {
            tools.push(Arc::new(WebSearchTool {
                gateway: gateway.clone(),
                default_model,
            }));
        } else {
            tracing::debug!("myra-web: no entitled web-search model; tool not offered");
        }
        if let Some(default_model) = thread_store
            .get::<MyraWebConfig>()
            .and_then(|config| config.fetch_model.clone())
        {
            tools.push(Arc::new(WebFetchTool {
                gateway: gateway.clone(),
                default_model,
            }));
        } else {
            tracing::debug!("myra-web: no entitled web-fetch model; tool not offered");
        }
        tools
    }
}

pub fn install(registry: &mut ExtensionRegistryBuilder<Config>, auth_manager: Arc<AuthManager>) {
    let extension = Arc::new(MyraWebExtension { auth_manager });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.tool_contributor(extension);
}
