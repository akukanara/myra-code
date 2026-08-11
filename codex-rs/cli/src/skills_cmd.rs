use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use codex_core::config::Config;
use codex_core::config::find_myra_home;
use codex_core::skills::remote::RegistrySkill;
use codex_core::skills::remote::install_registry_skill;
use codex_core::skills::remote::list_registry_skills;
use codex_core::skills::remote::validate_skill_id;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_skills::parse_skill_frontmatter_metadata;
use codex_utils_cli::CliConfigOverrides;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

/// User-scope skills live directly under MYRA_HOME/skills. The bundled ones
/// live in a dotted sibling (`.system`) that discovery treats as a separate
/// scope, so it is never a candidate for install or remove.
const SKILLS_DIR: &str = "skills";

#[derive(Debug, Parser)]
#[command(bin_name = "myra skills")]
pub struct SkillsCli {
    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,

    #[command(subcommand)]
    pub subcommand: SkillsSubcommand,
}

#[derive(Debug, clap::Subcommand)]
pub enum SkillsSubcommand {
    /// Show installed skills and what the gateway publishes.
    List(ListArgs),

    /// Install or update one or more skills from the gateway.
    Install(InstallArgs),

    /// Install everything the gateway publishes and update what is already installed.
    Sync(SyncArgs),

    /// Delete an installed skill.
    Remove(RemoveArgs),
}

#[derive(Debug, Parser)]
#[command(
    bin_name = "myra skills list",
    after_help = "Examples:\n  myra skills list\n  myra skills list --installed\n  myra skills list --json"
)]
pub struct ListArgs {
    /// Only what is on this machine. Works offline.
    #[arg(long, conflicts_with = "available")]
    installed: bool,

    /// Only what the gateway publishes.
    #[arg(long, conflicts_with = "installed")]
    available: bool,

    /// Machine-readable output.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Parser)]
#[command(
    bin_name = "myra skills install",
    after_help = "Examples:\n  myra skills install myrarouter\n  myra skills install myrarouter-chat myrarouter-image"
)]
pub struct InstallArgs {
    /// Skill names, as shown by `myra skills list`.
    #[arg(value_name = "NAME", required = true)]
    names: Vec<String>,
}

#[derive(Debug, Parser)]
#[command(bin_name = "myra skills sync")]
pub struct SyncArgs {
    /// Report what would change without writing anything.
    #[arg(long)]
    dry_run: bool,
}

#[derive(Debug, Parser)]
#[command(bin_name = "myra skills remove")]
pub struct RemoveArgs {
    /// Skill name to delete from this machine.
    #[arg(value_name = "NAME")]
    name: String,
}

/// A skill on disk, as far as this command cares: a directory holding a
/// SKILL.md. The description is best-effort -- a skill whose frontmatter does
/// not parse is still installed, and hiding it would be a lie.
#[derive(Debug, Serialize)]
struct InstalledSkill {
    name: String,
    description: Option<String>,
}

#[derive(Debug, Serialize)]
struct ListRow {
    name: String,
    status: &'static str,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<String>,
}

struct SkillsContext {
    codex_home: PathBuf,
    base_url: String,
    auth: Option<CodexAuth>,
}

impl SkillsContext {
    async fn load(overrides: Vec<(String, toml::Value)>) -> Result<Self> {
        let codex_home = find_myra_home().context("failed to resolve MYRA_HOME")?;
        let config = Config::load_with_cli_overrides(overrides)
            .await
            .context("failed to load configuration")?;
        let auth = AuthManager::shared_from_config(&config, /*enable_codex_api_key_env*/ true)
            .await
            .auth()
            .await;

        // The catalog is served by the same host that serves model requests, so
        // the base URL is taken from the active provider rather than configured
        // separately -- pointing the CLI at a different gateway moves both.
        let base_url = config
            .model_provider
            .to_api_provider(auth.as_ref().map(CodexAuth::api_auth_mode))
            .context("failed to resolve the provider endpoint")?
            .base_url;

        Ok(Self {
            codex_home: codex_home.to_path_buf(),
            base_url,
            auth,
        })
    }

    fn skills_root(&self) -> PathBuf {
        self.codex_home.join(SKILLS_DIR)
    }
}

pub async fn run_list(overrides: Vec<(String, toml::Value)>, args: ListArgs) -> Result<()> {
    let ctx = SkillsContext::load(overrides).await?;
    let installed = read_installed_skills(&ctx.skills_root())?;

    // --installed is the offline path: never touch the network for it, so it
    // still works on a plane or against a gateway that is down.
    //
    // None means "the catalog is unknown", which is NOT the same as "the
    // catalog is empty" -- without it every installed skill would be reported
    // as `local`, claiming the gateway does not publish it when we never asked.
    let catalog = if args.installed {
        None
    } else {
        match list_registry_skills(&ctx.base_url, ctx.auth.as_ref()).await {
            Ok(skills) => Some(skills),
            Err(err) if args.available => return Err(err),
            Err(err) => {
                // Listing what is on disk is still useful when the gateway is
                // unreachable, so this degrades instead of failing.
                eprintln!("Could not reach the skill registry: {err}");
                None
            }
        }
    };

    let rows = build_rows(&installed, catalog.as_deref(), args.available);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if rows.is_empty() {
        println!("No skills yet. Run `myra skills sync` to install what the gateway publishes.");
        return Ok(());
    }

    let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let status_width = rows.iter().map(|r| r.status.len()).max().unwrap_or(6);
    for row in &rows {
        println!(
            "{:name_width$}  {:status_width$}  {}",
            row.name,
            row.status,
            truncate(&row.description, 72),
        );
    }
    Ok(())
}

pub async fn run_install(overrides: Vec<(String, toml::Value)>, args: InstallArgs) -> Result<()> {
    let ctx = SkillsContext::load(overrides).await?;
    // Validate every name before writing anything, so a typo in the third
    // argument does not leave the first two half-applied.
    for name in &args.names {
        validate_skill_id(name)?;
    }

    let mut failures = 0usize;
    for name in &args.names {
        match install_registry_skill(&ctx.base_url, &ctx.codex_home, ctx.auth.as_ref(), name).await
        {
            Ok(result) => println!("installed {name} -> {}", result.path.display()),
            Err(err) => {
                failures += 1;
                eprintln!("failed to install {name}: {err}");
            }
        }
    }

    if failures > 0 {
        anyhow::bail!(
            "{failures} of {} skill(s) failed to install",
            args.names.len()
        );
    }
    Ok(())
}

pub async fn run_sync(overrides: Vec<(String, toml::Value)>, args: SyncArgs) -> Result<()> {
    let ctx = SkillsContext::load(overrides).await?;
    let available = list_registry_skills(&ctx.base_url, ctx.auth.as_ref()).await?;
    if available.is_empty() {
        println!("The gateway publishes no skills.");
        return Ok(());
    }

    let installed = read_installed_skills(&ctx.skills_root())?
        .into_iter()
        .map(|s| s.name)
        .collect::<Vec<_>>();

    if args.dry_run {
        for skill in &available {
            let verb = if installed.contains(&skill.id) {
                "update"
            } else {
                "install"
            };
            println!("would {verb} {}", skill.id);
        }
        return Ok(());
    }

    let mut installed_count = 0usize;
    let mut updated_count = 0usize;
    let mut failures = 0usize;
    for skill in &available {
        let existed = installed.contains(&skill.id);
        match install_registry_skill(&ctx.base_url, &ctx.codex_home, ctx.auth.as_ref(), &skill.id)
            .await
        {
            Ok(_) if existed => updated_count += 1,
            Ok(_) => installed_count += 1,
            Err(err) => {
                failures += 1;
                eprintln!("failed to sync {}: {err}", skill.id);
            }
        }
    }

    println!("{installed_count} installed, {updated_count} updated, {failures} failed");
    // Skills this machine has that the gateway no longer publishes are left
    // alone: they may be hand-written, and sync is not a mirror.
    if failures > 0 {
        anyhow::bail!("{failures} skill(s) failed to sync");
    }
    Ok(())
}

pub async fn run_remove(overrides: Vec<(String, toml::Value)>, args: RemoveArgs) -> Result<()> {
    let ctx = SkillsContext::load(overrides).await?;
    validate_skill_id(&args.name)?;

    let target = ctx.skills_root().join(&args.name);
    if !target.is_dir() {
        anyhow::bail!("\"{}\" is not installed", args.name);
    }
    std::fs::remove_dir_all(&target)
        .with_context(|| format!("failed to remove {}", target.display()))?;
    println!("removed {}", args.name);
    Ok(())
}

/// Directories under the user skills root that hold a SKILL.md.
///
/// A missing root is normal on a fresh install and means "none", not an error.
fn read_installed_skills(root: &Path) -> Result<Vec<InstalledSkill>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", root.display()));
        }
    };

    let mut skills = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        // Skips `.system`, which holds the bundled skills this command does
        // not own. Discovery hides every dotted directory, so matching the
        // same rule keeps this list to exactly what the CLI would load.
        if name.starts_with('.') {
            continue;
        }
        let skill_md = entry.path().join("SKILL.md");
        if !skill_md.is_file() {
            continue;
        }
        let description = std::fs::read_to_string(&skill_md)
            .ok()
            .and_then(|contents| parse_skill_frontmatter_metadata(&contents, || name.clone()).ok())
            .map(|parsed| parsed.description);
        skills.push(InstalledSkill { name, description });
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

fn build_rows(
    installed: &[InstalledSkill],
    catalog: Option<&[RegistrySkill]>,
    available_only: bool,
) -> Vec<ListRow> {
    let by_id: BTreeMap<&str, &RegistrySkill> = catalog
        .unwrap_or(&[])
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut rows: BTreeMap<String, ListRow> = BTreeMap::new();

    if !available_only {
        for skill in installed {
            let remote = by_id.get(skill.name.as_str());
            rows.insert(
                skill.name.clone(),
                ListRow {
                    name: skill.name.clone(),
                    // "local" is a claim about the catalog -- that the gateway
                    // does not publish this one, so sync will never touch it.
                    // Only make it when the catalog was actually read.
                    status: match (catalog.is_some(), remote.is_some()) {
                        (true, false) => "local",
                        _ => "installed",
                    },
                    description: skill
                        .description
                        .clone()
                        .or_else(|| remote.map(|r| r.description.clone()))
                        .unwrap_or_default(),
                    category: remote.and_then(|r| r.category.clone()),
                },
            );
        }
    }

    for skill in catalog.unwrap_or(&[]) {
        if rows.contains_key(&skill.id) {
            continue;
        }
        rows.insert(
            skill.id.clone(),
            ListRow {
                name: skill.id.clone(),
                status: "available",
                description: skill.description.clone(),
                category: skill.category.clone(),
            },
        );
    }

    rows.into_values().collect()
}

/// Cut on a character boundary, not a byte one -- a description is free text
/// and routinely contains multi-byte characters.
fn truncate(value: &str, max_chars: usize) -> String {
    let flat = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let head: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(name: &str, description: Option<&str>) -> InstalledSkill {
        InstalledSkill {
            name: name.to_string(),
            description: description.map(str::to_string),
        }
    }

    fn available(id: &str, description: &str) -> RegistrySkill {
        RegistrySkill {
            id: id.to_string(),
            display_name: id.to_string(),
            description: description.to_string(),
            category: Some("Media".to_string()),
            version: None,
            installs: 0,
        }
    }

    #[test]
    fn marks_a_skill_the_gateway_does_not_publish_as_local() {
        let rows = build_rows(
            &[
                installed("hand-written", Some("mine")),
                installed("myrarouter", None),
            ],
            Some(&[available("myrarouter", "from the gateway")]),
            false,
        );
        let statuses: Vec<_> = rows.iter().map(|r| (r.name.as_str(), r.status)).collect();
        assert_eq!(
            statuses,
            vec![("hand-written", "local"), ("myrarouter", "installed")]
        );
    }

    #[test]
    fn falls_back_to_the_registry_description_when_frontmatter_did_not_parse() {
        let rows = build_rows(
            &[installed("myrarouter", None)],
            Some(&[available("myrarouter", "from the gateway")]),
            false,
        );
        assert_eq!(rows[0].description, "from the gateway");
    }

    #[test]
    fn lists_uninstalled_catalog_entries_as_available() {
        let rows = build_rows(&[], Some(&[available("myrarouter-chat", "chat")]), false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "available");
    }

    #[test]
    fn installed_only_hides_the_catalog() {
        let rows = build_rows(&[installed("mine", Some("d"))], None, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "mine");
    }

    #[test]
    fn does_not_call_a_skill_local_when_the_catalog_was_never_read() {
        // --installed skips the fetch, and an unreachable gateway degrades to
        // the same state. Reporting "local" there would claim the gateway does
        // not publish the skill -- a question that was never asked.
        let rows = build_rows(&[installed("myrarouter-chat", Some("d"))], None, false);
        assert_eq!(rows[0].status, "installed");
    }

    #[test]
    fn available_only_hides_local_skills() {
        let rows = build_rows(
            &[installed("mine", Some("d"))],
            Some(&[available("published", "x")]),
            true,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "published");
    }

    #[test]
    fn truncate_cuts_on_a_character_boundary() {
        // A byte-based cut here panics rather than truncating.
        let value = "halo ✅ dunia yang sangat panjang sekali dan terus berlanjut";
        let out = truncate(value, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_collapses_whitespace() {
        assert_eq!(truncate("a\n  b\tc", 40), "a b c");
    }

    #[test]
    fn rejects_names_that_would_escape_the_skills_directory() {
        for name in ["../etc", "a/b", "..", "", "Caps", "-lead"] {
            assert!(
                validate_skill_id(name).is_err(),
                "{name} should be rejected"
            );
        }
        assert!(validate_skill_id("myrarouter-chat").is_ok());
    }
}

// ── Automatic sync ───────────────────────────────────────────────────────────

/// How long a sync is considered fresh. Long enough that the network is not
/// touched on every invocation, short enough that a skill published today is
/// in place by tomorrow.
const AUTO_SYNC_INTERVAL_HOURS: u64 = 6;

/// Startup must not hang on an unreachable gateway, so the whole sync gets one
/// short budget and is abandoned if it overruns. A skipped sync costs nothing;
/// the next run picks it up.
const AUTO_SYNC_BUDGET: Duration = Duration::from_secs(6);

/// Records when the last successful sync ran. A file rather than a config
/// entry: it is state, not something anyone should edit.
const AUTO_SYNC_STAMP: &str = ".myra-autosync";

/// Sync the gateway's catalog before a session starts, at most every few hours.
///
/// Awaited rather than backgrounded, and that is deliberate: install replaces
/// a skill's directory in place, and the session is about to walk that same
/// directory. A background task would be racing the loader for the file it is
/// reading. Bounded by AUTO_SYNC_BUDGET so the cost of being correct here is a
/// few seconds, rarely.
///
/// Never fails the caller. Every outcome except "it worked" is silence: an
/// unreachable gateway, no credentials yet, a read-only home. Someone starting
/// a coding session did not ask about skills, and an error on their first line
/// would be noise, not information.
pub async fn maybe_auto_sync(overrides: Vec<(String, toml::Value)>) {
    if !auto_sync_enabled() {
        return;
    }
    match tokio::time::timeout(AUTO_SYNC_BUDGET, run_auto_sync(overrides)).await {
        Ok(Ok(count)) if count > 0 => {
            tracing::info!("myra skills: synced {count} skill(s) from the gateway");
        }
        Ok(Ok(_)) => {}
        Ok(Err(err)) => tracing::debug!("myra skills: auto-sync skipped: {err}"),
        Err(_) => tracing::debug!("myra skills: auto-sync exceeded its time budget"),
    }
}

/// `MYRA_SKILLS_AUTOSYNC=0` (or `false`/`off`) turns it off. On by default:
/// a catalog nobody has is not a catalog, and the whole point of publishing a
/// skill from the dashboard is that it arrives without anyone running anything.
fn auto_sync_enabled() -> bool {
    match std::env::var("MYRA_SKILLS_AUTOSYNC") {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

async fn run_auto_sync(overrides: Vec<(String, toml::Value)>) -> Result<usize> {
    let ctx = SkillsContext::load(overrides).await?;
    if ctx.auth.is_none() {
        anyhow::bail!("not signed in");
    }

    let stamp = ctx.skills_root().join(AUTO_SYNC_STAMP);
    if recently_synced(&stamp) {
        return Ok(0);
    }

    let available = list_registry_skills(&ctx.base_url, ctx.auth.as_ref()).await?;
    let installed: Vec<String> = read_installed_skills(&ctx.skills_root())?
        .into_iter()
        .map(|skill| skill.name)
        .collect();

    let mut changed = 0usize;
    for skill in &available {
        // Only what is missing. Re-downloading everything on a schedule would
        // overwrite an edit the user made between syncs, every few hours,
        // without them asking -- `myra skills sync` is where that is explicit.
        if installed.contains(&skill.id) {
            continue;
        }
        if install_registry_skill(&ctx.base_url, &ctx.codex_home, ctx.auth.as_ref(), &skill.id)
            .await
            .is_ok()
        {
            changed += 1;
        }
    }

    // Stamped even when nothing changed: the point is that the gateway was
    // asked, not that it had news.
    touch_stamp(&stamp);
    Ok(changed)
}

fn recently_synced(stamp: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(stamp) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|age| age < Duration::from_secs(AUTO_SYNC_INTERVAL_HOURS * 3600))
        // A clock that moved backwards makes elapsed() fail; treat that as
        // fresh rather than syncing on every single start until it settles.
        .unwrap_or(true)
}

fn touch_stamp(stamp: &Path) {
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(stamp, "");
}
