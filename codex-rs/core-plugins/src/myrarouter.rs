use crate::remote::RemoteMarketplace;
use crate::remote::RemotePluginServiceConfig;
use crate::remote::RemotePluginSummary;
use anyhow::Context;
use anyhow::Result;
use codex_app_server_protocol::PluginAuthPolicy;
use codex_app_server_protocol::PluginAvailability;
use codex_app_server_protocol::PluginDisabledReason;
use codex_app_server_protocol::PluginInstallPolicy;
use codex_app_server_protocol::PluginInterface;
use codex_login::CodexAuth;
use codex_plugin::PluginId;
use http::Method;
use serde::Deserialize;
use std::time::Duration;

pub const MYRAROUTER_MARKETPLACE_NAME: &str = "myrarouter";
pub const MYRAROUTER_MARKETPLACE_DISPLAY_NAME: &str = "MyraTools";

const REQUIRED_MYRA_TOOL_NAMES: [&str; 4] = [
    "myrarouter-image",
    "myrarouter-web-fetch",
    "myrarouter-web-search",
    "myrarouter-myractx",
];

fn is_required_myra_tool_name(name: &str) -> bool {
    REQUIRED_MYRA_TOOL_NAMES.contains(&name)
}

const MARKETPLACE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
struct CatalogResponse {
    data: Vec<CatalogItem>,
}

#[derive(Debug, Deserialize)]
struct CatalogItem {
    id: String,
    name: String,
    display_name: String,
    description: String,
    #[serde(default)]
    short_description: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    website_url: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    source: Option<String>,
    /// Whether the gateway can actually serve this capability right now. Absent on older
    /// gateways, which published every tool unconditionally -- so absence means available and
    /// this stays backward compatible.
    #[serde(default)]
    available: Option<bool>,
    /// Why it cannot be served, when it cannot. Shown to the user, because the fix is almost
    /// always "configure a provider" rather than anything the CLI can do.
    #[serde(default)]
    unavailable_reason: Option<String>,
}

/// Fetches the catalog exposed by the active MyraRouter model provider.
///
/// Entries are browse-only. MyraTools are native gateway capabilities, MCP
/// entries point to their directory pages, and Skills keep their existing
/// `myra skills install/sync` lifecycle.
pub async fn fetch_marketplace(
    config: &RemotePluginServiceConfig,
    auth: Option<&CodexAuth>,
) -> Result<RemoteMarketplace> {
    let base_url = config.chatgpt_base_url.trim_end_matches('/');
    let url = format!("{base_url}/plugins");
    let mut request = config
        .http_request(Method::GET, &url)
        .timeout(MARKETPLACE_TIMEOUT);
    if let Some(auth) = auth {
        request =
            request.headers(codex_model_provider::auth_provider_from_auth(auth).to_auth_headers());
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("failed to request MyraRouter plugin catalog from {url}"))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("MyraRouter plugin catalog returned {status} from {url}: {body}");
    }
    let catalog: CatalogResponse = serde_json::from_str(&body)
        .with_context(|| format!("failed to decode MyraRouter plugin catalog from {url}"))?;

    let mut plugins = catalog
        .data
        .into_iter()
        .filter_map(catalog_item_to_summary)
        .collect::<Vec<_>>();
    ensure_required_myra_tools(&mut plugins);
    Ok(RemoteMarketplace {
        name: MYRAROUTER_MARKETPLACE_NAME.to_string(),
        display_name: MYRAROUTER_MARKETPLACE_DISPLAY_NAME.to_string(),
        plugins,
    })
}

fn catalog_item_to_summary(item: CatalogItem) -> Option<RemotePluginSummary> {
    let is_required = is_required_myra_tool_name(&item.name);
    let plugin_id = match PluginId::new(item.name.clone(), MYRAROUTER_MARKETPLACE_NAME.to_string())
    {
        Ok(plugin_id) => plugin_id,
        Err(err) => {
            tracing::warn!(plugin_name = %item.name, error = %err, "ignoring invalid MyraRouter plugin catalog entry");
            return None;
        }
    };
    // Absent means available: an older gateway published its whole static catalog and had no way
    // to say otherwise.
    let is_available = item.available.unwrap_or(true);
    let unavailable_reason = item
        .unavailable_reason
        .filter(|reason| !reason.trim().is_empty());

    let short_description = item
        .short_description
        .filter(|description| !description.trim().is_empty())
        .or_else(|| Some(item.description.clone()));
    // The reason rides along in the description because that is what the picker shows. Without it
    // the entry reads as simply broken, when the fix is usually one page away in the dashboard.
    let short_description = match (&unavailable_reason, short_description) {
        (Some(reason), Some(description)) => Some(format!("{description} — {reason}")),
        (Some(reason), None) => Some(reason.clone()),
        (None, description) => description,
    };

    Some(RemotePluginSummary {
        id: plugin_id.as_key(),
        remote_plugin_id: item.id,
        version: item.version,
        local_version: None,
        name: item.name,
        share_context: None,
        installed: is_required,
        installed_at: None,
        // A capability whose provider is not configured must not be offered as working. This is
        // the substantive half of the fix: the gateway now reports availability, and the CLI stops
        // presenting an unavailable tool as one the model can call.
        enabled: is_required && is_available,
        install_policy: if is_required {
            PluginInstallPolicy::InstalledByDefault
        } else {
            PluginInstallPolicy::NotAvailable
        },
        install_policy_source: None,
        must_show_installation_interstitial: None,
        auth_policy: PluginAuthPolicy::OnUse,
        // DisabledByAdmin is the only non-available variant this protocol has, and it is not
        // strictly what happened -- nobody disabled anything, the provider was never configured.
        // Reusing it beats adding a protocol variant plus its TS bindings for one case, and
        // disabled_reason carries the accurate detail: the thing the tool depends on is missing.
        availability: if is_available {
            PluginAvailability::Available
        } else {
            PluginAvailability::DisabledByAdmin
        },
        disabled_reason: if is_available {
            None
        } else {
            Some(PluginDisabledReason::RequiredAppUnavailable)
        },
        eligible_plan_types: None,
        interface: Some(PluginInterface {
            display_name: Some(item.display_name),
            short_description,
            long_description: Some(item.description),
            developer_name: Some(if item.source.as_deref() == Some("openskills") {
                "OpenSkills via MyraRouter".to_string()
            } else {
                MYRAROUTER_MARKETPLACE_DISPLAY_NAME.to_string()
            }),
            category: item.category,
            capabilities: item.capabilities,
            website_url: item.website_url,
            privacy_policy_url: None,
            terms_of_service_url: None,
            default_prompt: None,
            brand_color: Some("#7C3AED".to_string()),
            composer_icon: None,
            composer_icon_url: None,
            logo: None,
            logo_dark: None,
            logo_url: None,
            logo_url_dark: None,
            screenshots: Vec::new(),
            screenshot_urls: Vec::new(),
        }),
        keywords: item.keywords,
    })
}

/// MyraTools are gateway-native capabilities, not downloadable bundles. They
/// must always be present and enabled, even when the router catalog is briefly
/// unavailable or an older router omits one of them.
pub fn required_marketplace() -> RemoteMarketplace {
    let plugins = vec![
        required_tool(
            "myrarouter-image",
            "Image Generation",
            "Generate and edit images through one OpenAI-compatible endpoint.",
            "POST /v1/images/generations",
            "/dashboard/models",
        ),
        required_tool(
            "myrarouter-web-fetch",
            "Web Fetch",
            "Turn a URL into clean markdown, text, or HTML for an agent.",
            "POST /v1/web/fetch",
            "/dashboard/providers?kind=webFetch",
        ),
        required_tool(
            "myrarouter-web-search",
            "Web Search",
            "Search the live web with provider routing and automatic fallback.",
            "POST /v1/search",
            "/dashboard/providers?kind=webSearch",
        ),
        required_tool(
            "myrarouter-myractx",
            "MyraCtx",
            "Find current, version-aware coding documentation through MyraCode.",
            "POST /v1/myractx/search",
            "/dashboard/myractx",
        ),
    ]
    .into_iter()
    .filter_map(catalog_item_to_summary)
    .collect();

    RemoteMarketplace {
        name: MYRAROUTER_MARKETPLACE_NAME.to_string(),
        display_name: MYRAROUTER_MARKETPLACE_DISPLAY_NAME.to_string(),
        plugins,
    }
}

fn required_tool(
    name: &str,
    display_name: &str,
    description: &str,
    capability: &str,
    website_url: &str,
) -> CatalogItem {
    CatalogItem {
        id: name.to_string(),
        name: name.to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        short_description: None,
        category: Some("MyraTools · Required".to_string()),
        version: Some("1".to_string()),
        website_url: Some(website_url.to_string()),
        capabilities: vec![capability.to_string()],
        keywords: vec!["myrarouter".to_string(), "required".to_string()],
        source: Some("myrarouter".to_string()),
        // These placeholders stand in for a gateway that did not answer, so there is nothing to
        // report about availability -- leave it unstated rather than assert either way.
        available: None,
        unavailable_reason: None,
    }
}

fn ensure_required_myra_tools(plugins: &mut Vec<RemotePluginSummary>) {
    let mut ordered = Vec::with_capacity(plugins.len().max(REQUIRED_MYRA_TOOL_NAMES.len()));
    for required in required_marketplace().plugins {
        if let Some(index) = plugins
            .iter()
            .position(|plugin| plugin.name == required.name)
        {
            ordered.push(plugins.remove(index));
        } else {
            ordered.push(required);
        }
    }
    ordered.append(plugins);
    *plugins = ordered;
}

#[cfg(test)]
#[path = "myrarouter_tests.rs"]
mod tests;
