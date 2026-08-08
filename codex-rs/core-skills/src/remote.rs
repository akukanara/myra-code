use anyhow::Context;
use anyhow::Result;
use serde::Deserialize;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use codex_login::CodexAuth;
use codex_login::default_client::create_client_without_request_logging;

const REMOTE_SKILLS_API_TIMEOUT: Duration = Duration::from_secs(30);

// Low-level client for the remote skill API. This is intentionally kept around for
// future wiring, but it is not used yet by any active product surface.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSkillScope {
    WorkspaceShared,
    AllShared,
    Personal,
    Example,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSkillProductSurface {
    Chatgpt,
    Codex,
    Api,
    Atlas,
}

fn as_query_scope(scope: RemoteSkillScope) -> Option<&'static str> {
    match scope {
        RemoteSkillScope::WorkspaceShared => Some("workspace-shared"),
        RemoteSkillScope::AllShared => Some("all-shared"),
        RemoteSkillScope::Personal => Some("personal"),
        RemoteSkillScope::Example => Some("example"),
    }
}

fn as_query_product_surface(product_surface: RemoteSkillProductSurface) -> &'static str {
    match product_surface {
        RemoteSkillProductSurface::Chatgpt => "chatgpt",
        RemoteSkillProductSurface::Codex => "codex",
        RemoteSkillProductSurface::Api => "api",
        RemoteSkillProductSurface::Atlas => "atlas",
    }
}

fn ensure_codex_backend_auth(auth: Option<&CodexAuth>) -> Result<&CodexAuth> {
    let Some(auth) = auth else {
        anyhow::bail!("chatgpt authentication required for remote skill scopes");
    };
    if !auth.uses_codex_backend() {
        anyhow::bail!(
            "chatgpt authentication required for remote skill scopes; api key auth is not supported"
        );
    }
    Ok(auth)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSkillSummary {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSkillDownloadResult {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct RemoteSkillsResponse {
    #[serde(rename = "hazelnuts")]
    skills: Vec<RemoteSkill>,
}

#[derive(Debug, Deserialize)]
struct RemoteSkill {
    id: String,
    name: String,
    description: String,
}

pub async fn list_remote_skills(
    chatgpt_base_url: String,
    auth: Option<&CodexAuth>,
    scope: RemoteSkillScope,
    product_surface: RemoteSkillProductSurface,
    enabled: Option<bool>,
) -> Result<Vec<RemoteSkillSummary>> {
    let base_url = chatgpt_base_url.trim_end_matches('/');
    let auth = ensure_codex_backend_auth(auth)?;

    let url = format!("{base_url}/hazelnuts");
    let product_surface = as_query_product_surface(product_surface);
    let mut query_params = vec![("product_surface", product_surface)];
    if let Some(scope) = as_query_scope(scope) {
        query_params.push(("scope", scope));
    }
    if let Some(enabled) = enabled {
        let enabled = if enabled { "true" } else { "false" };
        query_params.push(("enabled", enabled));
    }

    let client = create_client_without_request_logging();
    let request = client
        .get(&url)
        .timeout(REMOTE_SKILLS_API_TIMEOUT)
        .query(&query_params)
        .headers(codex_model_provider::auth_provider_from_auth(auth).to_auth_headers());
    let response = request
        .send()
        .await
        .with_context(|| format!("Failed to send request to {url}"))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("Request failed with status {status} from {url}: {body}");
    }

    let parsed: RemoteSkillsResponse =
        serde_json::from_str(&body).context("Failed to parse skills response")?;

    Ok(parsed
        .skills
        .into_iter()
        .map(|skill| RemoteSkillSummary {
            id: skill.id,
            name: skill.name,
            description: skill.description,
        })
        .collect())
}

pub async fn export_remote_skill(
    chatgpt_base_url: String,
    codex_home: PathBuf,
    auth: Option<&CodexAuth>,
    skill_id: &str,
) -> Result<RemoteSkillDownloadResult> {
    let auth = ensure_codex_backend_auth(auth)?;

    let client = create_client_without_request_logging();
    let base_url = chatgpt_base_url.trim_end_matches('/');
    let url = format!("{base_url}/hazelnuts/{skill_id}/export");
    let request = client
        .get(&url)
        .timeout(REMOTE_SKILLS_API_TIMEOUT)
        .headers(codex_model_provider::auth_provider_from_auth(auth).to_auth_headers());

    let response = request
        .send()
        .await
        .with_context(|| format!("Failed to send download request to {url}"))?;

    let status = response.status();
    let body = response.bytes().await.context("Failed to read download")?;
    if !status.is_success() {
        let body_text = String::from_utf8_lossy(&body);
        anyhow::bail!("Download failed with status {status} from {url}: {body_text}");
    }

    if !is_zip_payload(&body) {
        anyhow::bail!("Downloaded remote skill payload is not a zip archive");
    }

    let output_dir = codex_home.join("skills").join(skill_id);
    tokio::fs::create_dir_all(&output_dir)
        .await
        .context("Failed to create downloaded skills directory")?;

    let zip_bytes = body.to_vec();
    let output_dir_clone = output_dir.clone();
    let prefix_candidates = vec![skill_id.to_string()];
    tokio::task::spawn_blocking(move || {
        extract_zip_to_dir(zip_bytes, &output_dir_clone, &prefix_candidates)
    })
    .await
    .context("Zip extraction task failed")??;

    Ok(RemoteSkillDownloadResult {
        id: skill_id.to_string(),
        path: output_dir,
    })
}

fn safe_join(base: &Path, name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                anyhow::bail!("Invalid file path in remote skill payload: {name}");
            }
        }
    }
    Ok(base.join(path))
}

fn is_zip_payload(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
}

fn extract_zip_to_dir(
    bytes: Vec<u8>,
    output_dir: &Path,
    prefix_candidates: &[String],
) -> Result<()> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).context("Failed to open zip archive")?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).context("Failed to read zip entry")?;
        if file.is_dir() {
            continue;
        }
        let raw_name = file.name().to_string();
        let normalized = normalize_zip_name(&raw_name, prefix_candidates);
        let Some(normalized) = normalized else {
            continue;
        };
        let file_path = safe_join(output_dir, &normalized)?;
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create parent dir for {normalized}"))?;
        }
        let mut out = std::fs::File::create(&file_path)
            .with_context(|| format!("Failed to create file {normalized}"))?;
        std::io::copy(&mut file, &mut out)
            .with_context(|| format!("Failed to write skill file {normalized}"))?;
    }
    Ok(())
}

fn normalize_zip_name(name: &str, prefix_candidates: &[String]) -> Option<String> {
    let mut trimmed = name.trim_start_matches("./");
    for prefix in prefix_candidates {
        if prefix.is_empty() {
            continue;
        }
        let prefix = format!("{prefix}/");
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            trimmed = rest;
            break;
        }
    }
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ── MyraRouter skill registry ────────────────────────────────────────────────
//
// The gateway publishes its skill catalog on the same base URL the model
// requests already go to, so `myra skills` needs no separate host, key or
// login: whatever authenticates a turn authenticates this.
//
//   GET {base}/skills                 -> { "object": "list", "data": [ ... ] }
//   GET {base}/skills/{id}/export     -> application/zip, rooted at {id}/
//
// Deliberately NOT `ensure_codex_backend_auth`: a gateway API key is a
// first-class credential here, unlike on the hosted backend above, and
// refusing it would lock out every API-key user for no reason.

/// One entry in `GET {base}/skills`. Metadata only -- the instructions live in
/// the archive, so listing the catalog does not pull down every skill's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrySkill {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub category: Option<String>,
    pub version: Option<String>,
    pub installs: u64,
}

#[derive(Debug, Deserialize)]
struct RegistryListResponse {
    data: Vec<RegistrySkillPayload>,
}

#[derive(Debug, Deserialize)]
struct RegistrySkillPayload {
    id: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    installs: Option<u64>,
}

/// reqwest is not a direct dependency of this crate, so its types are never
/// named here -- the header map is only ever built inline at the call site.
fn ensure_registry_auth(auth: Option<&CodexAuth>) -> Result<&CodexAuth> {
    let Some(auth) = auth else {
        anyhow::bail!("not signed in -- run `myra login` first");
    };
    Ok(auth)
}

/// Every skill the gateway publishes, in catalog order.
pub async fn list_registry_skills(
    base_url: &str,
    auth: Option<&CodexAuth>,
) -> Result<Vec<RegistrySkill>> {
    let base_url = base_url.trim_end_matches('/');
    let url = format!("{base_url}/skills");

    let client = create_client_without_request_logging();
    let response = client
        .get(&url)
        .timeout(REMOTE_SKILLS_API_TIMEOUT)
        .headers(
            codex_model_provider::auth_provider_from_auth(ensure_registry_auth(auth)?)
                .to_auth_headers(),
        )
        .send()
        .await
        .with_context(|| format!("Failed to reach the skill registry at {url}"))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("Skill registry returned {status} from {url}: {body}");
    }

    let parsed: RegistryListResponse =
        serde_json::from_str(&body).context("Failed to parse the skill registry response")?;

    Ok(parsed
        .data
        .into_iter()
        .map(|skill| RegistrySkill {
            display_name: skill.display_name.unwrap_or_else(|| skill.id.clone()),
            description: skill.description.unwrap_or_default(),
            category: skill.category,
            version: skill.version,
            installs: skill.installs.unwrap_or(0),
            id: skill.id,
        })
        .collect())
}

/// Download one skill and unpack it into `{codex_home}/skills/{id}`.
///
/// Replaces the directory outright rather than merging into it: an install of a
/// skill already present IS the update path, and merging would leave a file
/// deleted upstream on disk forever. The cost is that local edits to a catalog
/// skill do not survive an update -- documented in docs/skills.md, since there
/// is no way to tell an edit apart from a stale file after the fact.
pub async fn install_registry_skill(
    base_url: &str,
    codex_home: &Path,
    auth: Option<&CodexAuth>,
    skill_id: &str,
) -> Result<RemoteSkillDownloadResult> {
    validate_skill_id(skill_id)?;
    let base_url = base_url.trim_end_matches('/');
    let url = format!("{base_url}/skills/{skill_id}/export");

    let client = create_client_without_request_logging();
    let response = client
        .get(&url)
        .timeout(REMOTE_SKILLS_API_TIMEOUT)
        .headers(
            codex_model_provider::auth_provider_from_auth(ensure_registry_auth(auth)?)
                .to_auth_headers(),
        )
        .send()
        .await
        .with_context(|| format!("Failed to download {skill_id} from {url}"))?;

    let status = response.status();
    let body = response.bytes().await.context("Failed to read download")?;
    if status.as_u16() == 404 {
        anyhow::bail!("No published skill named \"{skill_id}\"");
    }
    if !status.is_success() {
        let body_text = String::from_utf8_lossy(&body);
        anyhow::bail!("Download of {skill_id} failed with status {status}: {body_text}");
    }
    if !is_zip_payload(&body) {
        anyhow::bail!("The payload for {skill_id} is not a zip archive");
    }

    let output_dir = codex_home.join("skills").join(skill_id);
    if tokio::fs::try_exists(&output_dir).await.unwrap_or(false) {
        tokio::fs::remove_dir_all(&output_dir)
            .await
            .with_context(|| format!("Failed to replace the existing {skill_id} skill"))?;
    }
    tokio::fs::create_dir_all(&output_dir)
        .await
        .context("Failed to create the skills directory")?;

    let zip_bytes = body.to_vec();
    let output_dir_clone = output_dir.clone();
    let prefix_candidates = vec![skill_id.to_string()];
    tokio::task::spawn_blocking(move || {
        extract_zip_to_dir(zip_bytes, &output_dir_clone, &prefix_candidates)
    })
    .await
    .context("Zip extraction task failed")??;

    Ok(RemoteSkillDownloadResult {
        id: skill_id.to_string(),
        path: output_dir,
    })
}

/// A skill id becomes a path segment and a URL segment, so it is checked before
/// it reaches either. `safe_join` guards the archive's own entry names; this
/// guards the name the caller typed.
pub fn validate_skill_id(skill_id: &str) -> Result<()> {
    let ok = !skill_id.is_empty()
        && skill_id.len() <= 64
        && skill_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !skill_id.starts_with('-');
    if !ok {
        anyhow::bail!(
            "\"{skill_id}\" is not a valid skill name (lowercase letters, digits and hyphens)"
        );
    }
    Ok(())
}
