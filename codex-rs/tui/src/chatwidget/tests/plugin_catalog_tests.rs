use super::*;
use pretty_assertions::assert_eq;

fn plugin_with_category(mut plugin: PluginSummary, category: &str) -> PluginSummary {
    plugin
        .interface
        .as_mut()
        .expect("test plugin interface")
        .category = Some(category.to_string());
    plugin
}

#[tokio::test]
async fn plugins_popup_replaces_curated_tab_with_required_myratools() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_remote_marketplace(
            "myrarouter",
            "MyraTools",
            vec![
                plugin_with_category(
                    plugins_test_remote_summary(
                        "myrarouter-image",
                        "myrarouter-image",
                        Some("Image Generation"),
                        Some("Generate images through MyraRouter."),
                        /*installed*/ true,
                    ),
                    "MyraTools",
                ),
                plugin_with_category(
                    plugins_test_remote_summary(
                        "myrarouter-web-fetch",
                        "myrarouter-web-fetch",
                        Some("Web Fetch"),
                        Some("Fetch web pages."),
                        /*installed*/ true,
                    ),
                    "MyraTools",
                ),
                plugin_with_category(
                    plugins_test_remote_summary(
                        "myrarouter-web-search",
                        "myrarouter-web-search",
                        Some("Web Search"),
                        Some("Search the web."),
                        /*installed*/ true,
                    ),
                    "MyraTools",
                ),
                plugin_with_category(
                    plugins_test_remote_summary(
                        "mcp-browserbase",
                        "mcp-browserbase",
                        Some("Browserbase"),
                        Some("Automate browsers."),
                        /*installed*/ false,
                    ),
                    "MCPs · Browser automation",
                ),
            ],
        )]),
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    let tab_line = popup
        .lines()
        .find(|line| line.contains("[All Plugins]"))
        .expect("plugins tab line");
    assert_eq!(
        tab_line.trim(),
        "[All Plugins]  Installed (3)  MyraTools  Add Marketplace"
    );

    let myratools = select_plugins_tab_containing(&mut chat, /*width*/ 100, "[MyraTools]");
    assert!(myratools.contains("Image Generation"));
    assert!(myratools.contains("Web Fetch"));
    assert!(myratools.contains("Web Search"));
    assert!(!myratools.contains("Browserbase"));
}

#[tokio::test]
async fn plugins_popup_hides_workspace_and_shared_tabs_but_keeps_local() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![
            plugins_test_remote_marketplace(
                "workspace-directory",
                "Raw Workspace Directory",
                vec![plugins_test_remote_summary(
                    "plugins~Plugin_buildkite",
                    "buildkite",
                    Some("Buildkite"),
                    Some("Workspace CI."),
                    /*installed*/ false,
                )],
            ),
            plugins_test_remote_marketplace(
                "workspace-shared-with-me-private",
                "Raw Shared Private",
                vec![plugins_test_remote_summary(
                    "plugins~Plugin_docs",
                    "docs",
                    Some("Docs"),
                    Some("Shared docs."),
                    /*installed*/ false,
                )],
            ),
            plugins_test_remote_marketplace(
                "workspace-shared-with-me-unlisted",
                "Raw Shared Link",
                vec![plugins_test_remote_summary(
                    "plugins~Plugin_link",
                    "link",
                    Some("Link Share"),
                    Some("Shared by link."),
                    /*installed*/ false,
                )],
            ),
            PluginMarketplaceEntry {
                name: "codex-curated".to_string(),
                path: Some(plugins_test_personal_marketplace_path()),
                interface: Some(MarketplaceInterface {
                    display_name: Some("Personal".to_string()),
                }),
                plugins: vec![plugins_test_summary(
                    "plugin-local-docs",
                    "local-docs",
                    Some("Local Docs"),
                    Some("Local editable docs."),
                    /*installed*/ false,
                    /*enabled*/ true,
                    PluginInstallPolicy::Available,
                )],
            },
        ]),
    );

    let popup = render_bottom_popup(&chat, /*width*/ 120);
    assert!(!popup.contains("Workspace"));
    assert!(!popup.contains("Shared with me"));

    let local = select_plugins_tab_containing(&mut chat, /*width*/ 120, "[Local]");
    assert!(local.contains("Local Docs"));
    assert!(!local.contains("Personal."));
}

#[tokio::test]
async fn plugins_popup_preserves_workspace_tab_across_load_and_detail_navigation() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let workspace_marketplace = plugins_test_remote_marketplace(
        "workspace-directory",
        "Raw Workspace Directory",
        vec![plugins_test_remote_summary(
            "plugins~Plugin_buildkite",
            "buildkite",
            Some("Buildkite"),
            Some("Buildkite pipelines."),
            /*installed*/ false,
        )],
    );
    chat.add_plugins_output();
    let cwd = chat.config.cwd.clone();
    chat.on_plugins_loaded(
        cwd.to_path_buf(),
        Ok(plugins_test_response(vec![
            plugins_test_curated_marketplace(Vec::new()),
        ])),
    );
    let loading_popup =
        select_plugins_tab_containing(&mut chat, /*width*/ 100, "Loading Workspace plugins.");
    assert!(
        loading_popup.contains("Loading Workspace plugins."),
        "expected Workspace loading tab before remote sections resolve, got:\n{loading_popup}"
    );

    chat.on_plugin_remote_sections_loaded(
        cwd.to_path_buf(),
        vec![workspace_marketplace.clone()],
        Vec::new(),
    );
    let workspace_popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        workspace_popup.contains("Workspace.")
            && workspace_popup.contains("Buildkite")
            && !workspace_popup.contains("Loading Workspace plugins."),
        "expected remote section refresh to keep the Workspace tab active, got:\n{workspace_popup}"
    );

    chat.open_plugin_detail_loading_popup("Buildkite");
    chat.open_plugins_list(
        cwd.to_path_buf(),
        plugins_test_response(vec![
            plugins_test_curated_marketplace(Vec::new()),
            workspace_marketplace,
        ]),
    );
    let reopened_popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        reopened_popup.contains("Workspace.") && reopened_popup.contains("Buildkite"),
        "expected Back to plugins to preserve the Workspace tab, got:\n{reopened_popup}"
    );
}

#[tokio::test]
async fn plugins_popup_remote_local_dedupe_prefers_installed_remote_after_mapped_shares() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let remote_plugin_id = "plugins~Plugin_docs";
    let local_summary = PluginSummary {
        remote_plugin_id: Some(remote_plugin_id.to_string()),
        ..plugins_test_summary(
            "plugin-docs",
            "docs",
            Some("Docs"),
            Some("Local curated docs plugin."),
            /*installed*/ false,
            /*enabled*/ true,
            PluginInstallPolicy::Available,
        )
    };
    let cwd = chat.config.cwd.clone();
    render_loaded_plugins_popup(
        &mut chat,
        plugins_test_response(vec![plugins_test_curated_marketplace(vec![local_summary])]),
    );
    chat.on_plugin_remote_sections_loaded(
        cwd.to_path_buf(),
        vec![plugins_test_remote_marketplace(
            "workspace-shared-with-me-private",
            "Shared with me",
            vec![plugins_test_remote_summary(
                remote_plugin_id,
                "docs",
                Some("Docs"),
                Some("Remote installed docs plugin."),
                /*installed*/ true,
            )],
        )],
        Vec::new(),
    );
    let popup = render_bottom_popup(&chat, /*width*/ 100);
    let PluginsCacheState::Ready(response) = &chat.plugins_cache else {
        panic!("expected cached plugins after remote section refresh");
    };
    assert_eq!(
        response
            .marketplaces
            .iter()
            .map(|marketplace| marketplace.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            OPENAI_CURATED_MARKETPLACE_NAME,
            "workspace-shared-with-me-private"
        ]
    );
    let all_plugins_row = popup
        .lines()
        .find(|line| line.contains("Docs"))
        .expect("expected all-plugins row");
    assert!(
        popup.contains("Installed 1 of 1 available plugins."),
        "expected header count to reflect deduped plugin rows, got:\n{popup}"
    );
    assert!(
        all_plugins_row.contains("Installed")
            && !all_plugins_row.contains("Local curated docs plugin."),
        "expected installed remote duplicate to win when local row is not a mapped share, got:\n{all_plugins_row}"
    );
}

#[tokio::test]
async fn plugin_detail_not_installable_plugin_disables_install_action() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Plugins, /*enabled*/ true);

    let summary = plugins_test_summary(
        "plugin-internal",
        "internal",
        Some("Internal"),
        Some("Internal only."),
        /*installed*/ false,
        /*enabled*/ true,
        PluginInstallPolicy::NotAvailable,
    );
    let cwd = chat.config.cwd.clone();
    chat.on_plugins_loaded(
        cwd.to_path_buf(),
        Ok(plugins_test_response(vec![
            plugins_test_curated_marketplace(vec![summary.clone()]),
        ])),
    );
    chat.add_plugins_output();
    chat.on_plugin_detail_loaded(
        cwd.to_path_buf(),
        Ok(PluginReadResponse {
            plugin: plugins_test_detail(summary, Some("Internal only."), &[], &[], &[], &[]),
        }),
    );

    let popup = render_bottom_popup(&chat, /*width*/ 100);
    let install_row = popup
        .lines()
        .find(|line| line.contains("Install plugin"))
        .expect("expected install row");
    assert!(
        install_row.contains("This plugin is not installable from this marketplace."),
        "expected disabled not-installable row, got:\n{install_row}"
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    assert_eq!(
        render_bottom_popup(&chat, /*width*/ 100),
        popup,
        "expected navigation to skip the disabled install row"
    );
}
