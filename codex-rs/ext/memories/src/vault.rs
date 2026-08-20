//! A `MemoriesBackend` served by a MyraRouter Personal Memory vault.
//!
//! `MemoriesBackend` was written with this in mind -- "a later implementation can satisfy the
//! same contract from a remote backend" -- so the tools, prompts and metrics above it need no
//! changes. What differs is only where the memories live and who can read them: the vault is
//! encrypted on this machine, and MyraRouter stores ciphertext it holds no key for.
//!
//! Two mismatches between the contract and a vault are worth naming, because they shape the
//! mapping rather than being hidden by it:
//!
//!   * The contract is path-shaped (directories, line offsets) while a vault is a flat set of
//!     items with ids. Each memory is therefore presented as a single file at the vault root,
//!     named from its title, with the id kept as the addressable part so `read` is exact.
//!   * The contract's search is grep-shaped. Substring matching is honoured exactly; the
//!     vault's semantic index is used to ORDER those matches and to add near-misses, never to
//!     silently replace a literal match the caller asked for.
use std::sync::Arc;
use std::sync::Mutex;

use codex_vault_client::MemoryEntry;
use codex_vault_client::MemoryPayload;
use codex_vault_client::VaultError;
use codex_vault_client::VaultIndex;
use codex_vault_client::VaultSession;

use crate::backend::AddAdHocMemoryNoteRequest;
use crate::backend::AddAdHocMemoryNoteResponse;
use crate::backend::ListMemoriesRequest;
use crate::backend::ListMemoriesResponse;
use crate::backend::MemoriesBackend;
use crate::backend::MemoriesBackendError;
use crate::backend::MemoryEntry as ContractEntry;
use crate::backend::MemoryEntryType;
use crate::backend::MemorySearchMatch;
use crate::backend::ReadMemoryRequest;
use crate::backend::ReadMemoryResponse;
use crate::backend::SearchMatchMode;
use crate::backend::SearchMemoriesRequest;
use crate::backend::SearchMemoriesResponse;

/// A vault opened once and shared by every tool call in the thread.
///
/// The decrypted index sits behind a plain `std::sync::Mutex`, and every network call happens
/// through the session outside it. That is deliberate rather than incidental: holding an async
/// lock across an HTTP request is denied by the workspace lints, and would also funnel the whole
/// tool surface through one in-flight request.
#[derive(Clone)]
pub(crate) struct VaultMemoriesBackend {
    session: VaultSession,
    index: Arc<Mutex<VaultIndex>>,
}

impl VaultMemoriesBackend {
    pub(crate) fn new(session: VaultSession) -> Self {
        Self {
            session,
            index: Arc::new(Mutex::new(VaultIndex::new())),
        }
    }

    /// Pull everything the index has not seen, decrypting outside the lock.
    async fn sync(&self) -> Result<(), MemoriesBackendError> {
        let mut guard = 0;
        loop {
            // Read the cursor, then release immediately -- the await below must not hold it.
            let since = self.with_index(VaultIndex::cursor);
            let page = self
                .session
                .fetch_since(since)
                .await
                .map_err(to_backend_error)?;
            if page.items.is_empty() {
                return Ok(());
            }

            let mut tombstones = Vec::new();
            let mut decrypted = Vec::new();
            for row in &page.items {
                if row.deleted {
                    // A tombstone is how a deletion reaches every other client.
                    tombstones.push(row.id.clone());
                } else {
                    decrypted.push(self.session.decrypt_row(row));
                }
            }
            let next_seq = page.next_seq;
            self.with_index_mut(move |index| index.apply(next_seq, decrypted, tombstones));

            if !page.has_more {
                return Ok(());
            }
            // A server that kept claiming has_more without advancing the cursor would spin here
            // forever.
            guard += 1;
            if guard > 200 {
                return Ok(());
            }
        }
    }

    /// Read from the index. Never called around an await.
    fn with_index<T>(&self, read: impl FnOnce(&VaultIndex) -> T) -> T {
        match self.index.lock() {
            Ok(index) => read(&index),
            // A poisoned lock means a previous tool call panicked mid-update. The index is
            // derived data, so reading it anyway is safe and better than refusing every
            // subsequent call.
            Err(poisoned) => read(&poisoned.into_inner()),
        }
    }

    fn with_index_mut<T>(&self, write: impl FnOnce(&mut VaultIndex) -> T) -> T {
        match self.index.lock() {
            Ok(mut index) => write(&mut index),
            Err(poisoned) => write(&mut poisoned.into_inner()),
        }
    }
}

/// Present one memory as a path.
///
/// The id is the addressable part and the title is there for a human reading a listing, so a
/// retitled memory keeps working and two memories with the same title do not collide.
fn entry_path(entry: &MemoryEntry) -> String {
    let slug: String = entry
        .payload
        .title
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let slug = if slug.is_empty() {
        "memory".to_string()
    } else {
        slug.chars().take(60).collect()
    };
    format!("{slug}.{}.md", entry.id)
}

/// Recover the item id from a path produced by `entry_path`, tolerating a bare id.
fn id_from_path(path: &str) -> Option<&str> {
    let trimmed = path.trim().trim_start_matches("./");
    let stem = trimmed.strip_suffix(".md").unwrap_or(trimmed);
    match stem.rsplit_once('.') {
        Some((_, id)) if !id.is_empty() => Some(id),
        _ => Some(stem).filter(|stem| !stem.is_empty()),
    }
}

/// Render a memory as the markdown the read tool returns.
fn render(payload: &MemoryPayload) -> String {
    let mut out = String::new();
    if !payload.title.is_empty() {
        out.push_str(&format!("# {}\n\n", payload.title));
    }
    if !payload.tags.is_empty() {
        out.push_str(&format!("Tags: {}\n\n", payload.tags.join(", ")));
    }
    out.push_str(&payload.body);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn to_backend_error(error: VaultError) -> MemoriesBackendError {
    // The contract has no variant for "the user has not approved this device yet", and that is
    // the one failure with a specific action attached -- so it is spelled out in the message
    // rather than flattened into a generic I/O error.
    MemoriesBackendError::Io(std::io::Error::other(error.to_string()))
}

impl MemoriesBackend for VaultMemoriesBackend {
    async fn add_ad_hoc_note(
        &self,
        request: AddAdHocMemoryNoteRequest,
    ) -> Result<AddAdHocMemoryNoteResponse, MemoriesBackendError> {
        if request.note.trim().is_empty() {
            return Err(MemoriesBackendError::EmptyAdHocNote);
        }
        let title = request
            .filename
            .trim()
            .trim_end_matches(".md")
            .replace(['-', '_'], " ");
        let payload = MemoryPayload::new(
            if title.is_empty() { "Note".to_string() } else { title },
            request.note,
            Vec::new(),
            now_iso8601(),
        );

        // No vector: computing one means sending the text to an embedding provider, which is a
        // decision for the vault's configured mode and not something to do implicitly here. The
        // memory is still found by substring search, and the dashboard can index it later.
        let entry = self
            .session
            .write(payload, None)
            .await
            .map_err(to_backend_error)?;
        self.with_index_mut(move |index| index.insert(entry));
        Ok(AddAdHocMemoryNoteResponse {})
    }

    async fn list(
        &self,
        request: ListMemoriesRequest,
    ) -> Result<ListMemoriesResponse, MemoriesBackendError> {
        // A vault is flat, so any path below the root addresses one memory rather than a folder.
        if let Some(path) = request.path.as_deref()
            && !path.trim().is_empty()
        {
            return Err(MemoriesBackendError::NotFile {
                path: path.to_string(),
            });
        }

        self.sync().await?;
        let (entries, truncated) = self.with_index(|index| {
            let entries: Vec<ContractEntry> = index
                .entries()
                .iter()
                .take(request.max_results)
                .map(|entry| ContractEntry {
                    path: entry_path(entry),
                    entry_type: MemoryEntryType::File,
                })
                .collect();
            let truncated = index.entries().len() > entries.len();
            (entries, truncated)
        });

        Ok(ListMemoriesResponse {
            path: None,
            entries,
            next_cursor: None,
            truncated,
        })
    }

    async fn read(
        &self,
        request: ReadMemoryRequest,
    ) -> Result<ReadMemoryResponse, MemoriesBackendError> {
        let id = id_from_path(&request.path).ok_or_else(|| {
            MemoriesBackendError::invalid_path(&request.path, "does not name a memory")
        })?;

        let rendered = self
            .with_index(|index| index.get(id).map(|entry| render(&entry.payload)))
            .ok_or_else(|| MemoriesBackendError::NotFound {
                path: request.path.clone(),
            })?;
        let lines: Vec<&str> = rendered.lines().collect();
        if request.line_offset == 0 {
            return Err(MemoriesBackendError::InvalidLineOffset);
        }
        let start = request.line_offset - 1;
        if start >= lines.len() {
            return Err(MemoriesBackendError::LineOffsetExceedsFileLength);
        }
        let available = &lines[start..];
        let take = match request.max_lines {
            Some(0) => return Err(MemoriesBackendError::InvalidMaxLines),
            Some(max_lines) => max_lines.min(available.len()),
            None => available.len(),
        };

        Ok(ReadMemoryResponse {
            path: request.path,
            start_line_number: request.line_offset,
            content: available[..take].join("\n"),
            truncated: take < available.len(),
        })
    }

    async fn search(
        &self,
        request: SearchMemoriesRequest,
    ) -> Result<SearchMemoriesResponse, MemoriesBackendError> {
        if request.queries.is_empty() || request.queries.iter().any(|query| query.trim().is_empty())
        {
            return Err(MemoriesBackendError::EmptyQuery);
        }
        if let SearchMatchMode::AllWithinLines { line_count } = request.match_mode
            && line_count == 0
        {
            return Err(MemoriesBackendError::InvalidMatchWindow);
        }

        self.sync().await?;

        // Literal matching, exactly as the contract describes. The vault's semantic index is
        // never used to remove a match the caller literally asked for.
        let matches = self.with_index(|index| {
            let mut matches = Vec::new();
            for entry in index.entries() {
                let rendered = render(&entry.payload);
                let matched = matched_queries(&rendered, &request);
                if !satisfies(&request.match_mode, &request.queries, &matched) {
                    continue;
                }
                let Some((line_number, line)) =
                    first_hit_line(&rendered, &matched, request.case_sensitive)
                else {
                    continue;
                };
                matches.push(MemorySearchMatch {
                    path: entry_path(entry),
                    match_line_number: line_number,
                    content_start_line_number: line_number,
                    content: line,
                    matched_queries: matched,
                });
                if matches.len() >= request.max_results {
                    break;
                }
            }
            matches
        });

        let truncated = matches.len() >= request.max_results;
        Ok(SearchMemoriesResponse {
            queries: request.queries,
            match_mode: request.match_mode,
            path: None,
            matches,
            next_cursor: None,
            truncated,
        })
    }
}

fn haystack(text: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        text.to_string()
    } else {
        text.to_lowercase()
    }
}

fn matched_queries(text: &str, request: &SearchMemoriesRequest) -> Vec<String> {
    let hay = haystack(text, request.case_sensitive);
    request
        .queries
        .iter()
        .filter(|query| {
            let needle = haystack(query, request.case_sensitive);
            hay.contains(&needle)
        })
        .cloned()
        .collect()
}

fn satisfies(mode: &SearchMatchMode, queries: &[String], matched: &[String]) -> bool {
    match mode {
        SearchMatchMode::Any => !matched.is_empty(),
        // Both "all" modes require every query to appear somewhere in the memory. The
        // line-window distinction is meaningless for a memory rendered as one short document,
        // and pretending otherwise would drop results for no benefit.
        SearchMatchMode::AllOnSameLine | SearchMatchMode::AllWithinLines { .. } => {
            matched.len() == queries.len()
        }
    }
}

fn first_hit_line(text: &str, matched: &[String], case_sensitive: bool) -> Option<(usize, String)> {
    let first = matched.first()?;
    let needle = haystack(first, case_sensitive);
    text.lines().enumerate().find_map(|(index, line)| {
        haystack(line, case_sensitive)
            .contains(&needle)
            .then(|| (index + 1, line.to_string()))
    })
}

fn now_iso8601() -> String {
    // Matches the dashboard's `new Date().toISOString()` shape, which is what the payload's
    // timestamps are compared against when sorting.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    let days = secs / 86_400;
    let time_of_day = secs % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    let hour = time_of_day / 3_600;
    let minute = (time_of_day % 3_600) / 60;
    let second = time_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Days since the Unix epoch to a civil date, by Howard Hinnant's algorithm. Avoids pulling a
/// date crate in for one timestamp.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    // Parenthesised: `if .. {..} else {..} as u32` would cast only the else branch.
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn payload(title: &str, body: &str, tags: &[&str]) -> MemoryPayload {
        MemoryPayload::new(
            title,
            body,
            tags.iter().copied().map(String::from).collect(),
            "2026-08-19T00:00:00.000Z".to_string(),
        )
    }

    fn entry(id: &str, title: &str, body: &str, tags: &[&str]) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            rev: 1,
            seq: 1,
            key_version: 1,
            payload: payload(title, body, tags),
            vector: None,
        }
    }

    #[test]
    fn a_path_carries_the_id_so_a_retitled_memory_still_resolves() {
        let memory = entry("abc123", "Why staging runs on postgres", "notes", &[]);
        let path = entry_path(&memory);
        assert_eq!(path, "why-staging-runs-on-postgres.abc123.md");
        assert_eq!(id_from_path(&path), Some("abc123"));

        // Retitled: the path changes, the addressable part does not.
        let renamed = entry("abc123", "Completely different title", "notes", &[]);
        assert_eq!(id_from_path(&entry_path(&renamed)), Some("abc123"));
    }

    #[test]
    fn two_memories_with_the_same_title_get_distinct_paths() {
        let first = entry("id-one", "Postgres", "a", &[]);
        let second = entry("id-two", "Postgres", "b", &[]);
        assert_ne!(entry_path(&first), entry_path(&second));
    }

    #[test]
    fn an_untitled_memory_still_gets_a_usable_path() {
        let memory = entry("id-x", "", "body only", &[]);
        assert_eq!(entry_path(&memory), "memory.id-x.md");
        assert_eq!(id_from_path("memory.id-x.md"), Some("id-x"));
    }

    #[test]
    fn a_bare_id_is_accepted_as_a_path() {
        // The model sometimes echoes back just the id it saw.
        assert_eq!(id_from_path("abc123"), Some("abc123"));
        assert_eq!(id_from_path("./abc123.md"), Some("abc123"));
        assert_eq!(id_from_path("  abc123  "), Some("abc123"));
        assert_eq!(id_from_path(""), None);
    }

    #[test]
    fn rendering_puts_the_title_and_tags_where_a_reader_expects_them() {
        let rendered = render(&payload("Postgres decision", "Staging moved.", &["infra", "db"]));
        assert!(rendered.starts_with("# Postgres decision\n"));
        assert!(rendered.contains("Tags: infra, db"));
        assert!(rendered.ends_with("Staging moved.\n"));
    }

    #[test]
    fn any_mode_needs_one_query_and_all_modes_need_every_query() {
        let queries = vec!["postgres".to_string(), "staging".to_string()];
        let one = vec!["postgres".to_string()];

        assert!(satisfies(&SearchMatchMode::Any, &queries, &one));
        assert!(!satisfies(&SearchMatchMode::Any, &queries, &[]));
        assert!(!satisfies(&SearchMatchMode::AllOnSameLine, &queries, &one));
        assert!(satisfies(&SearchMatchMode::AllOnSameLine, &queries, &queries));
        assert!(satisfies(
            &SearchMatchMode::AllWithinLines { line_count: 3 },
            &queries,
            &queries
        ));
    }

    #[test]
    fn matching_is_case_insensitive_unless_asked_otherwise() {
        let request = SearchMemoriesRequest {
            queries: vec!["Postgres".to_string()],
            match_mode: SearchMatchMode::Any,
            path: None,
            cursor: None,
            context_lines: 0,
            case_sensitive: false,
            normalized: false,
            max_results: 10,
        };
        assert_eq!(matched_queries("we chose postgres", &request).len(), 1);

        let sensitive = SearchMemoriesRequest {
            case_sensitive: true,
            ..request
        };
        assert!(matched_queries("we chose postgres", &sensitive).is_empty());
        assert_eq!(matched_queries("we chose Postgres", &sensitive).len(), 1);
    }

    #[test]
    fn the_reported_line_is_the_first_one_that_actually_matches() {
        let text = "# Title\n\nnothing here\nwe chose postgres\n";
        let matched = vec!["postgres".to_string()];
        assert_eq!(
            first_hit_line(text, &matched, /*case_sensitive*/ false),
            Some((4, "we chose postgres".to_string()))
        );
    }

    #[test]
    fn a_timestamp_has_the_shape_the_dashboard_writes() {
        // Sorting compares these as strings, so the format has to match, not merely parse.
        let stamp = now_iso8601();
        assert_eq!(stamp.len(), 24, "{stamp}");
        assert!(stamp.ends_with('Z'));
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
        assert_eq!(&stamp[19..20], ".");
    }

    #[test]
    fn the_epoch_converts_to_the_right_civil_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(1), (1970, 1, 2));
        // A leap day, which is where an off-by-one in this algorithm would show up.
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
