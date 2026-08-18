use base64::Engine;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn compose_success_url_uses_local_page_by_default() {
    let LoginSuccessRedirect::Local(url) = compose_success_url(
        /*port*/ 1455,
        DEFAULT_ISSUER,
        "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnt9fQ.sig",
        "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnt9fQ.sig",
            /*myra_streamlined_login*/ false,
        &LoginSuccessPage::default(),
    ) else {
        panic!("expected local success redirect");
    };
    let url = Url::parse(&url).expect("success URL should parse");

    assert_eq!(url.host_str(), Some("localhost"));
    assert_eq!(url.path(), "/success");
    for key in ["id_token", "org_id", "project_id", "plan_type"] {
        assert_eq!(
            url.query_pairs()
                .find(|(query_key, _)| query_key == key),
            None,
            "{key} must not be exposed in the browser callback URL"
        );
    }
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "myra_streamlined_login"),
        None
    );
}

#[test]
fn compose_success_url_uses_streamlined_local_page_when_requested() {
    let LoginSuccessRedirect::Local(url) = compose_success_url(
        /*port*/ 1455,
        DEFAULT_ISSUER,
        "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnt9fQ.sig",
        "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnt9fQ.sig",
        /*myra_streamlined_login*/ true,
        &LoginSuccessPage::default(),
    ) else {
        panic!("expected local success redirect");
    };
    let url = Url::parse(&url).expect("success URL should parse");

    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "myra_streamlined_login")
            .map(|(_, value)| value.into_owned()),
        Some("true".to_string())
    );
}

#[test]
fn compose_success_url_uses_hosted_page_when_requested() {
    assert_eq!(
        compose_success_url(
            /*port*/ 1455,
            DEFAULT_ISSUER,
            "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnt9fQ.sig",
            "e30.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnt9fQ.sig",
        /*myra_streamlined_login*/ false,
            &LoginSuccessPage::Hosted {
                url: Url::parse(MYRAROUTER_DASHBOARD_URL)
                    .expect("MyraRouter dashboard URL should parse"),
                app_brand: LoginSuccessPageBrand::Chatgpt,
            },
        ),
        LoginSuccessRedirect::Hosted(MYRAROUTER_DASHBOARD_URL.to_string())
    );
}

#[test]
fn compose_success_url_keeps_setup_on_local_page() {
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let payload = encode(
        serde_json::to_string(&json!({
            "https://api.openai.com/auth": {
                "completed_platform_onboarding": false,
                "is_org_owner": true,
                "organization_id": "org_123",
                "project_id": "proj_123",
            }
        }))
        .expect("payload should serialize")
        .as_bytes(),
    );
    let access_payload = encode(
        serde_json::to_string(&json!({
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "team",
            }
        }))
        .expect("payload should serialize")
        .as_bytes(),
    );
    let id_token = format!("e30.{payload}.sig");
    let LoginSuccessRedirect::Local(url) = compose_success_url(
        /*port*/ 1455,
        DEFAULT_ISSUER,
        &id_token,
        &format!("e30.{access_payload}.sig"),
        /*myra_streamlined_login*/ true,
        &LoginSuccessPage::Hosted {
            url: Url::parse(MYRAROUTER_DASHBOARD_URL)
                .expect("MyraRouter dashboard URL should parse"),
            app_brand: LoginSuccessPageBrand::Codex,
        },
    ) else {
        panic!("expected local success redirect");
    };
    let url = Url::parse(&url).expect("success URL should parse");

    assert_eq!(url.host_str(), Some("localhost"));
    assert_eq!(url.path(), "/success");
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "platform_url")
            .map(|(_, value)| value.into_owned()),
        Some(MYRAROUTER_DASHBOARD_URL.to_string())
    );
    assert_eq!(
        url.query_pairs()
            .find(|(key, _)| key == "needs_setup")
            .map(|(_, value)| value.into_owned()),
        Some("true".to_string())
    );
}
