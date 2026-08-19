use super::*;

#[test]
fn required_catalog_item_maps_to_installed_plugin() {
    let summary = catalog_item_to_summary(CatalogItem {
        id: "web-search".to_string(),
        name: "myrarouter-web-search".to_string(),
        display_name: "Web Search".to_string(),
        description: "Search the live web.".to_string(),
        short_description: None,
        category: Some("MyraTools".to_string()),
        version: Some("1".to_string()),
        website_url: None,
        capabilities: vec!["POST /v1/search".to_string()],
        keywords: vec!["web".to_string()],
        source: Some("myrarouter".to_string()),
        available: None,
        unavailable_reason: None,
    })
    .expect("valid catalog item");

    assert_eq!(summary.id, "myrarouter-web-search@myrarouter");
    assert_eq!(
        summary.install_policy,
        PluginInstallPolicy::InstalledByDefault
    );
    assert!(summary.installed);
    assert!(summary.enabled);
    assert_eq!(
        summary
            .interface
            .and_then(|interface| interface.display_name),
        Some("Web Search".to_string())
    );
}

#[test]
fn required_marketplace_always_contains_all_core_tools() {
    let marketplace = required_marketplace();
    assert_eq!(marketplace.display_name, "MyraTools");
    assert_eq!(marketplace.plugins.len(), 4);
    assert_eq!(
        marketplace
            .plugins
            .iter()
            .map(|plugin| plugin.name.as_str())
            .collect::<Vec<_>>(),
        REQUIRED_MYRA_TOOL_NAMES
    );
    assert!(marketplace.plugins.iter().all(|plugin| {
        plugin.installed
            && plugin.enabled
            && plugin.install_policy == PluginInstallPolicy::InstalledByDefault
    }));
}

#[test]
fn invalid_plugin_name_is_ignored() {
    let summary = catalog_item_to_summary(CatalogItem {
        id: "bad".to_string(),
        name: "bad/name".to_string(),
        display_name: "Bad".to_string(),
        description: "Bad entry".to_string(),
        short_description: None,
        category: None,
        version: None,
        website_url: None,
        capabilities: Vec::new(),
        keywords: Vec::new(),
        source: None,
        available: None,
        unavailable_reason: None,
    });

    assert!(summary.is_none());
}

/// Builder for the availability cases below, so each test states only what it is about.
fn web_search_item(available: Option<bool>, reason: Option<&str>) -> CatalogItem {
    CatalogItem {
        id: "web-search".to_string(),
        name: "myrarouter-web-search".to_string(),
        display_name: "Web Search".to_string(),
        description: "Search the live web.".to_string(),
        short_description: None,
        category: Some("MyraTools".to_string()),
        version: Some("1".to_string()),
        website_url: Some("/dashboard/providers?kind=webSearch".to_string()),
        capabilities: vec!["POST /v1/search".to_string()],
        keywords: vec!["web".to_string()],
        source: Some("myrarouter".to_string()),
        available,
        unavailable_reason: reason.map(str::to_string),
    }
}

#[test]
fn an_older_gateway_that_omits_availability_is_still_treated_as_available() {
    // Gateways before this field published their whole static catalog unconditionally, so absence
    // has to keep meaning "available" or upgrading the CLI would disable every tool.
    let summary = catalog_item_to_summary(web_search_item(None, None)).expect("valid catalog item");
    assert_eq!(summary.availability, PluginAvailability::Available);
    assert_eq!(summary.disabled_reason, None);
    assert!(summary.enabled);
}

#[test]
fn an_unavailable_tool_is_not_offered_as_working() {
    // The point of the whole change: a capability whose provider was never configured must not be
    // presented to the model as one it can call.
    let summary = catalog_item_to_summary(web_search_item(
        Some(false),
        Some("No web-search provider is connected yet."),
    ))
    .expect("valid catalog item");

    assert!(!summary.enabled);
    assert_eq!(summary.availability, PluginAvailability::DisabledByAdmin);
    assert_eq!(
        summary.disabled_reason,
        Some(PluginDisabledReason::RequiredAppUnavailable)
    );
    // Still listed and still installed-by-default: it is one configuration step away, and hiding
    // it would hide the capability rather than the problem.
    assert!(summary.installed);
    assert_eq!(
        summary.install_policy,
        PluginInstallPolicy::InstalledByDefault
    );
}

#[test]
fn the_reason_reaches_the_text_the_picker_shows() {
    let summary = catalog_item_to_summary(web_search_item(
        Some(false),
        Some("No web-search provider is connected yet."),
    ))
    .expect("valid catalog item");
    let short = summary
        .interface
        .and_then(|interface| interface.short_description)
        .expect("short description");
    assert!(short.contains("Search the live web."), "{short}");
    assert!(short.contains("No web-search provider is connected yet."), "{short}");
}

#[test]
fn a_blank_reason_does_not_pad_the_description() {
    let summary = catalog_item_to_summary(web_search_item(Some(false), Some("   ")))
        .expect("valid catalog item");
    let short = summary
        .interface
        .and_then(|interface| interface.short_description)
        .expect("short description");
    assert_eq!(short, "Search the live web.");
    // Blank reason, but still unavailable -- the flag decides, not the prose.
    assert!(!summary.enabled);
}
