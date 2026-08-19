//! HTTP calls to MyraRouter's Personal Memory endpoints.
//!
//! Authenticated with the caller's existing model API key, which is why MyraCode needs no
//! second credential for this. The key proves WHICH ACCOUNT; the enrolled device id proves
//! WHICH DEVICE within it, and a device only ever reaches its own wrapped key.
//!
//! Nothing in this file can read a memory. Every payload that crosses it is either
//! non-secret metadata or a blob that was encrypted, or wrapped, elsewhere.
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("could not reach MyraRouter: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("MyraRouter refused the request ({status}): {message}")]
    Refused {
        status: u16,
        code: Option<String>,
        message: String,
    },
    #[error("MyraRouter returned a response this version does not understand: {0}")]
    Malformed(String),
    #[error("{base_url} is not a usable base URL")]
    BadBaseUrl { base_url: String },
}

impl ApiError {
    /// The machine-readable code, where the server sent one. Callers branch on
    /// `pending_approval` and `revoked` in particular -- both are ordinary states with
    /// specific things for a user to do, not failures to retry.
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Refused { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    pub fn is_pending_approval(&self) -> bool {
        self.code() == Some("pending_approval")
    }

    pub fn is_revoked(&self) -> bool {
        self.code() == Some("revoked")
    }
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub is_default: bool,
    pub key_version: u32,
    #[serde(default)]
    pub item_count: u64,
    #[serde(default)]
    pub embed: Option<EmbedDescriptor>,
    #[serde(default)]
    pub device_status: Option<String>,
    #[serde(default)]
    pub has_key_for_this_device: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedDescriptor {
    pub model: String,
    pub dim: u32,
    pub quant: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultListResponse {
    vaults: Vec<VaultSummary>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrollResponse {
    pub device_id: String,
    pub vault_id: String,
    pub vault_name: String,
    pub fingerprint: String,
    /// Shown to the user once, so they can type it into the dashboard. Only its hash is
    /// stored server-side, which is what makes the approval step meaningful.
    pub pairing_code: String,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WrappedKeyResponse {
    pub vault_id: String,
    pub key_version: u32,
    pub vault_key_version: u32,
    pub alg: String,
    pub wrapped_key: String,
    pub wrap_nonce: String,
    pub ephemeral_pub_jwk: Option<PublicJwk>,
    /// The salt this envelope's AAD was built from. Without it the AAD cannot be rebuilt,
    /// so the right private key still would not unwrap.
    #[serde(default)]
    pub aad_salt: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PublicJwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub y: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemRow {
    pub id: String,
    pub seq: u64,
    pub rev: u32,
    #[serde(default)]
    pub cipher: Option<String>,
    #[serde(default)]
    pub nonce: Option<String>,
    #[serde(default)]
    pub vec: Option<String>,
    #[serde(default)]
    pub vec_nonce: Option<String>,
    pub key_version: u32,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPage {
    pub items: Vec<ItemRow>,
    pub next_seq: u64,
    pub has_more: bool,
    pub vault_seq: u64,
    pub key_version: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemUpload {
    pub id: String,
    pub cipher: String,
    pub nonce: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vec_nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushOutcome {
    pub id: String,
    pub ok: bool,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub seq: Option<u64>,
    #[serde(default)]
    pub rev: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResponse {
    pub results: Vec<PushOutcome>,
    pub vault_seq: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedResponse {
    pub vectors: Vec<Vec<f32>>,
    pub model: String,
    pub dim: u32,
}

/// A client for one MyraRouter instance and one account.
#[derive(Clone)]
pub struct VaultApi {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl VaultApi {
    pub fn new(http: reqwest::Client, base_url: &str, api_key: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    async fn send<T: for<'de> Deserialize<'de>>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let response = request
            .bearer_auth(&self.api_key)
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            let parsed: Option<ErrorBody> = serde_json::from_str(&body).ok();
            return Err(ApiError::Refused {
                status: status.as_u16(),
                code: parsed.as_ref().and_then(|body| body.code.clone()),
                message: parsed
                    .and_then(|body| body.error)
                    .unwrap_or_else(|| status.to_string()),
            });
        }
        serde_json::from_str(&body).map_err(|error| ApiError::Malformed(error.to_string()))
    }

    /// Register this device's public key and get a pairing code to show the user.
    ///
    /// Grants nothing on its own: a pending device has no wrapped key. The code exists so
    /// a public key the user did not enrol has nothing to offer at the approval step.
    pub async fn enroll(
        &self,
        vault_id: Option<&str>,
        label: &str,
        jwk: &PublicJwk,
    ) -> Result<EnrollResponse, ApiError> {
        let mut body = serde_json::json!({
            "kind": "cli",
            "label": label,
            "publicKeyJwk": jwk,
        });
        if let Some(vault_id) = vault_id {
            body["vaultId"] = serde_json::Value::String(vault_id.to_string());
        }
        self.send(
            self.http
                .post(self.url("/api/v1/myractx/memory/enroll"))
                .json(&body),
        )
        .await
    }

    pub async fn vaults(&self, device_id: Option<&str>) -> Result<Vec<VaultSummary>, ApiError> {
        let mut request = self.http.get(self.url("/api/v1/myractx/memory/vaults"));
        if let Some(device_id) = device_id {
            request = request.query(&[("deviceId", device_id)]);
        }
        let response: VaultListResponse = self.send(request).await?;
        Ok(response.vaults)
    }

    /// This device's wrapped copy of the vault key.
    ///
    /// Fetched per session and never cached to disk. That is what makes revocation real:
    /// the user deletes this row and the next run has nothing to unwrap.
    pub async fn wrapped_key(
        &self,
        vault_id: &str,
        device_id: &str,
    ) -> Result<WrappedKeyResponse, ApiError> {
        self.send(
            self.http
                .get(self.url("/api/v1/myractx/memory/key"))
                .query(&[("vaultId", vault_id), ("deviceId", device_id)]),
        )
        .await
    }

    pub async fn pull(
        &self,
        vault_id: &str,
        device_id: &str,
        since: u64,
    ) -> Result<ItemPage, ApiError> {
        self.send(
            self.http
                .get(self.url("/api/v1/myractx/memory/items"))
                .query(&[
                    ("vaultId", vault_id),
                    ("deviceId", device_id),
                    ("since", &since.to_string()),
                ]),
        )
        .await
    }

    pub async fn push(
        &self,
        vault_id: &str,
        device_id: &str,
        items: Vec<ItemUpload>,
    ) -> Result<PushResponse, ApiError> {
        self.send(
            self.http
                .post(self.url("/api/v1/myractx/memory/items"))
                .json(&serde_json::json!({
                    "vaultId": vault_id,
                    "deviceId": device_id,
                    "items": items,
                })),
        )
        .await
    }
}
