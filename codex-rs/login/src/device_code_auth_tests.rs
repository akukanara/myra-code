use super::*;
use pretty_assertions::assert_eq;

#[test]
fn device_code_prompt_renders_phishing_warning() {
    let prompt = device_code_prompt("https://example.com/device", "ABCD-EFGH");

    assert!(prompt.contains(
        "\x1b[90mContinue only if you started this login in Myra. If a website or another person gave you this code, cancel.\x1b[0m"
    ));
}

#[test]
fn deserialize_user_code_resp_supports_integer_and_string_interval() {
    let json_int = r#"{"device_auth_id":"id1","user_code":"code1","interval":5}"#;
    let resp_int: UserCodeResp = serde_json::from_str(json_int).expect("deserialize int interval");
    assert_eq!(resp_int.interval, 5);
    assert_eq!(resp_int.user_code, "code1");

    let json_str = r#"{"device_auth_id":"id2","usercode":"code2","interval":"10"}"#;
    let resp_str: UserCodeResp = serde_json::from_str(json_str).expect("deserialize str interval");
    assert_eq!(resp_str.interval, 10);
    assert_eq!(resp_str.user_code, "code2");
}
