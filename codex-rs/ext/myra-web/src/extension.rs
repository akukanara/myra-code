use std::sync::Arc;

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

use crate::tool::GatewayWeb;
use crate::tool::WebFetchTool;
use crate::tool::WebSearchTool;

/// The gateway's key-free providers. Both work on a fresh instance with
/// nothing configured, which is what makes offering these tools by default
/// reasonable -- a default that needs an API key is a default that fails.
const DEFAULT_SEARCH_MODEL: &str = "searxng";
const DEFAULT_FETCH_MODEL: &str = "direct";

#[derive(Clone)]
struct MyraWebExtension {
    auth_manager: Arc<AuthManager>,
}

/// Resolved once per thread. `tools()` is handed the stores, not the Config,
/// so whatever it needs from configuration has to be put there first.
#[derive(Clone)]
struct MyraWebConfig {
    base_url: Option<String>,
}

impl MyraWebConfig {
    /// The auth mode is not optional here even though the parameter is.
    /// to_api_provider picks the DEFAULT base URL from it -- the gateway for a
    /// signed-in session, api.openai.com otherwise -- so passing None sends
    /// every request to OpenAI, which answers 404 for these paths. Resolving
    /// the real mode first is what points the tools at the right host.
    fn from_auth(config: &Config, auth: Option<&CodexAuth>) -> Self {
        let base_url = config
            .model_provider
            .to_api_provider(auth.map(CodexAuth::api_auth_mode))
            .ok()
            .map(|provider| provider.base_url)
            .filter(|url| !url.trim().is_empty());
        Self { base_url }
    }
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
                .insert(MyraWebConfig::from_auth(input.config, auth.as_ref()));
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
        vec![
            Arc::new(WebSearchTool {
                gateway: gateway.clone(),
                default_model: DEFAULT_SEARCH_MODEL.to_string(),
            }),
            Arc::new(WebFetchTool {
                gateway,
                default_model: DEFAULT_FETCH_MODEL.to_string(),
            }),
        ]
    }
}

pub fn install(registry: &mut ExtensionRegistryBuilder<Config>, auth_manager: Arc<AuthManager>) {
    let extension = Arc::new(MyraWebExtension { auth_manager });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.tool_contributor(extension);
}
