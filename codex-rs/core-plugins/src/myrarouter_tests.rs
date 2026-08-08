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
    assert_eq!(marketplace.plugins.len(), 3);
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
    });

    assert!(summary.is_none());
}
