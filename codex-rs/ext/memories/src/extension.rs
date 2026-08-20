use std::sync::Arc;

use codex_core::config::Config;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::PromptFragment;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolContributor;
use codex_features::Feature;
use codex_otel::MetricsClient;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::local::LocalMemoriesBackend;
use crate::prompts::build_memory_tool_developer_instructions;
use crate::tools;
use crate::vault::VaultMemoriesBackend;

/// Contributes Codex memory read-path prompt context and memory read tools.
#[derive(Clone, Default)]
pub(crate) struct MemoriesExtension {
    metrics_client: Option<MetricsClient>,
}

impl MemoriesExtension {
    fn new(metrics_client: Option<MetricsClient>) -> Self {
        Self { metrics_client }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct MemoriesExtensionConfig {
    pub(crate) enabled: bool,
    pub(crate) dedicated_tools: bool,
    pub(crate) codex_home: AbsolutePathBuf,
    pub(crate) vault: Option<VaultSettings>,
}

/// Where a Personal Memory vault lives, once the user has asked for one.
///
/// `Some` only when every part is present. A half-configured vault resolves to `None` and the
/// filesystem backend is used instead -- deliberately, because the alternative is writing
/// memories somewhere the user did not intend and cannot find.
#[derive(Clone, Debug)]
pub(crate) struct VaultSettings {
    pub(crate) base_url: String,
    pub(crate) api_key: String,
    pub(crate) vault_id: Option<String>,
}

impl VaultSettings {
    fn from_config(config: &Config) -> Option<Self> {
        let memories = &config.memories;
        if !memories.vault {
            return None;
        }
        let base_url = memories.vault_base_url.as_deref()?.trim().to_string();
        if base_url.is_empty() {
            return None;
        }
        // The key is read from the environment by name. Putting a gateway key in a config file
        // is how it ends up in a commit.
        let variable = memories.vault_api_key_env.as_deref().unwrap_or("MYRAROUTER_API_KEY");
        let api_key = std::env::var(variable).ok().filter(|key| !key.trim().is_empty())?;
        Some(Self {
            base_url,
            api_key,
            vault_id: memories.vault_id.clone(),
        })
    }
}

impl MemoriesExtensionConfig {
    fn from_config(config: &Config) -> Self {
        Self {
            enabled: config.features.enabled(Feature::MemoryTool) && config.memories.use_memories,
            dedicated_tools: config.memories.dedicated_tools,
            codex_home: config.codex_home.clone(),
            vault: VaultSettings::from_config(config),
        }
    }
}

impl ContextContributor for MemoriesExtension {
    fn contribute_thread_context<'a>(
        &'a self,
        _session_store: &'a ExtensionData,
        thread_store: &'a ExtensionData,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<PromptFragment>> + Send + 'a>> {
        Box::pin(async move {
            let Some(config) = thread_store.get::<MemoriesExtensionConfig>() else {
                return Vec::new();
            };
            if !config.enabled {
                return Vec::new();
            }

            build_memory_tool_developer_instructions(&config.codex_home)
                .await
                .map(PromptFragment::developer_policy)
                .into_iter()
                .collect()
        })
    }
}

impl ThreadLifecycleContributor<Config> for MemoriesExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            input
                .thread_store
                .insert(MemoriesExtensionConfig::from_config(input.config));
        })
    }
}

impl ConfigContributor<Config> for MemoriesExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        thread_store.insert(MemoriesExtensionConfig::from_config(new_config));
    }
}

impl ToolContributor for MemoriesExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn codex_extension_api::ToolExecutor<codex_extension_api::ToolCall>>> {
        let Some(config) = thread_store.get::<MemoriesExtensionConfig>() else {
            return Vec::new();
        };
        if !config.enabled || !config.dedicated_tools {
            return Vec::new();
        }

        // A configured vault is used if it can actually be opened. If it cannot -- the device is
        // not approved yet, the key was revoked, the gateway is unreachable -- the tools fall
        // back to the local filesystem rather than disappearing, so the model keeps a working
        // memory surface and the reason is reported through the vault's own errors instead.
        if let Some(settings) = config.vault.clone()
            && let Some(backend) = open_vault_backend(&config.codex_home, &settings)
        {
            return tools::memory_tools(backend, self.metrics_client.clone());
        }

        tools::memory_tools(
            LocalMemoriesBackend::from_codex_home(&config.codex_home),
            self.metrics_client.clone(),
        )
    }
}

/// Open the vault for this machine, or return `None` and let the caller fall back.
///
/// Synchronous because `tools()` is: the open is run on a short-lived runtime handle. It is done
/// once per tool-surface build rather than per call, since opening means fetching and decrypting
/// the whole vault.
fn open_vault_backend(
    codex_home: &AbsolutePathBuf,
    settings: &VaultSettings,
) -> Option<VaultMemoriesBackend> {
    let handle = tokio::runtime::Handle::try_current().ok()?;
    let root = codex_home.to_path_buf();
    let settings = settings.clone();
    let opened = tokio::task::block_in_place(move || {
        handle.block_on(async move {
            let api = codex_vault_client::VaultApi::new(
                reqwest::Client::new(),
                &settings.base_url,
                settings.api_key.clone(),
            );
            // The device key is per vault, so "auto" resolves against a placeholder until the
            // server names the vault it chose.
            let vault_key_scope = settings.vault_id.clone().unwrap_or_else(|| "default".to_string());
            let mut identity =
                codex_vault_client::DeviceIdentity::load_or_create(&root, &vault_key_scope).await?;
            codex_vault_client::VaultSession::open(
                api,
                &mut identity,
                settings.vault_id.as_deref(),
                &vault_device_label(),
            )
            .await
        })
    });

    match opened {
        Ok(vault) => Some(VaultMemoriesBackend::new(vault)),
        Err(error) => {
            // Worth a line in the log: a user who asked for a vault and silently got local files
            // would have no way to tell.
            tracing::warn!("Personal Memory vault unavailable, using local memories: {error}");
            None
        }
    }
}

fn vault_device_label() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default();
    if host.trim().is_empty() {
        "MyraCode".to_string()
    } else {
        format!("MyraCode on {host}")
    }
}

/// Installs the memories extension contributors into the extension registry.
pub fn install(
    registry: &mut ExtensionRegistryBuilder<Config>,
    metrics_client: Option<MetricsClient>,
) {
    let extension = Arc::new(MemoriesExtension::new(metrics_client));
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.prompt_contributor(extension.clone());
    registry.tool_contributor(extension);
}
