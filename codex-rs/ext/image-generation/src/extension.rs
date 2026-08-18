use std::sync::Arc;
use std::time::Duration;

use codex_core::config::Config;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadOriginator;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::default_client::create_client;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::backend::CodexImagesBackend;
use crate::tool::ImageGenerationTool;

#[derive(Clone)]
struct ImageGenerationExtension {
    auth_manager: Arc<AuthManager>,
    resolve_save_root: Arc<SaveRootResolver>,
}

type SaveRootResolver = dyn Fn(&Config) -> Option<AbsolutePathBuf> + Send + Sync;

#[derive(Clone)]
struct ImageGenerationExtensionConfig {
    available: bool,
    default_model: String,
    provider: ModelProviderInfo,
    save_root: Option<AbsolutePathBuf>,
}

impl ImageGenerationExtensionConfig {
    /// Resolves the image provider and save root for a thread.
    async fn from_config(
        config: &Config,
        auth: Option<&CodexAuth>,
        resolve_save_root: &SaveRootResolver,
    ) -> Self {
        let discovered_model = image_model_for_provider(&config.model_provider, auth).await;
        let default_model = discovered_model
            .clone()
            .unwrap_or_else(|| IMAGE_MODEL.to_string());
        Self {
            available: config.model_provider.is_openai()
                || config.model_provider.requires_openai_auth
                || config.model_provider.uses_openai_actor_authorization()
                || discovered_model.is_some(),
            default_model,
            provider: config.model_provider.clone(),
            save_root: resolve_save_root(config),
        }
    }
}

const IMAGE_MODEL: &str = "gpt-image-2";
const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(serde::Deserialize)]
struct ImageModelsResponse {
    data: Vec<ImageModel>,
}

#[derive(serde::Deserialize)]
struct ImageModel {
    id: String,
}

/// MyraRouter filters this catalog by the signed-in account's plan. Fall back
/// to the hosted API default when a compatible provider has no image catalog.
async fn image_model_for_provider(
    provider: &ModelProviderInfo,
    auth: Option<&CodexAuth>,
) -> Option<String> {
    let auth = auth?;
    let base_url = provider.to_api_provider(Some(auth.api_auth_mode())).ok()?.base_url;
    let url = format!("{}/models/image", base_url.trim_end_matches('/'));
    let response = create_client()
        .get(&url)
        .timeout(MODEL_CATALOG_TIMEOUT)
        .headers(codex_model_provider::auth_provider_from_auth(auth).to_auth_headers())
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .json::<ImageModelsResponse>()
        .await
        .ok()?
        .data
        .into_iter()
        .map(|model| model.id)
        .find(|model| !model.trim().is_empty())
}

impl ThreadLifecycleContributor<Config> for ImageGenerationExtension {
    /// Seeds image-generation configuration when a thread begins.
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let auth = self.auth_manager.auth().await;
            input
                .thread_store
                .insert(ImageGenerationExtensionConfig::from_config(
                    input.config,
                    auth.as_ref(),
                    self.resolve_save_root.as_ref(),
                )
                .await);
        })
    }
}

impl ConfigContributor<Config> for ImageGenerationExtension {
    /// Refreshes image-generation configuration after thread configuration changes.
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        let previous_config = thread_store
            .get::<ImageGenerationExtensionConfig>()
            .cloned();
        let previous_model = previous_config
            .as_ref()
            .map(|config| config.default_model.clone())
            .unwrap_or_else(|| IMAGE_MODEL.to_string());
        thread_store.insert(ImageGenerationExtensionConfig {
            available: new_config.model_provider.is_openai()
                || new_config.model_provider.requires_openai_auth
                || new_config.model_provider.uses_openai_actor_authorization()
                || previous_config.is_some_and(|config| config.available),
            default_model: previous_model,
            provider: new_config.model_provider.clone(),
            save_root: (self.resolve_save_root)(new_config),
        });
    }
}

impl ToolContributor for ImageGenerationExtension {
    /// Creates the image-generation tool exposed by this installed extension.
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        let Some(config) = thread_store.get::<ImageGenerationExtensionConfig>() else {
            return Vec::new();
        };
        if !config.available {
            return Vec::new();
        }

        vec![Arc::new(ImageGenerationTool::new(
            CodexImagesBackend::new(
                create_model_provider(config.provider.clone(), Some(self.auth_manager.clone())),
                thread_store
                    .get::<ThreadOriginator>()
                    .map(|originator| originator.0.clone()),
            ),
            config.default_model.clone(),
            config.save_root.clone(),
            thread_store.level_id().to_string(),
        ))]
    }
}

/// Installs the standalone image-generation extension contributors.
pub fn install(
    registry: &mut ExtensionRegistryBuilder<Config>,
    auth_manager: Arc<AuthManager>,
    resolve_save_root: impl Fn(&Config) -> Option<AbsolutePathBuf> + Send + Sync + 'static,
) {
    let extension = Arc::new(ImageGenerationExtension {
        auth_manager,
        resolve_save_root: Arc::new(resolve_save_root),
    });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}
