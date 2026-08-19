//! Commit-attribution instructions, dormant under Myra.
//!
//! `install` is a no-op (see the note on it), so nothing here is reachable
//! from a normal build -- but the machinery and its tests are kept intact so
//! attribution can be switched back on without rewriting it, and so upstream
//! changes to it keep applying. The lint would otherwise fire on every item
//! in the crate.
#![allow(dead_code)]

mod policy;
mod world_state;

use std::sync::Arc;
use std::time::Instant;

use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::WorldStateContributionInput;
use codex_extension_api::WorldStateSectionContribution;
use codex_http_client::HttpClientFactory;
use codex_login::AuthManager;

use crate::policy::GitAttributionPolicy;
use crate::policy::GitAttributionRetry;
use crate::policy::POLICY_RETRY_DELAY;
use crate::policy::auth_generation;
use crate::policy::cached_attribution_policy;
use crate::policy::resolve_attribution_policy;
use crate::policy::retry_deferred;
use crate::world_state::git_attribution_world_state_section;

/// Contributes model instructions for agent-created git commits and pull requests.
#[derive(Clone)]
struct GitAttributionExtension {
    auth_manager: Arc<AuthManager>,
    base_url: String,
    http_client_factory: HttpClientFactory,
}

impl ContextContributor for GitAttributionExtension {
    fn contribute_world_state<'a>(
        &'a self,
        input: WorldStateContributionInput<'a>,
    ) -> ExtensionFuture<'a, Vec<WorldStateSectionContribution>> {
        Box::pin(async move {
            // Attribution is optional prompt metadata. A backend that rejects this
            // endpoint can rotate auth while the request is in flight; retrying on
            // every generation change used to keep world-state construction in an
            // unbounded loop and prevented the actual model request from starting.
            const MAX_AUTH_GENERATION_RETRIES: usize = 2;
            let mut generation_retries = 0;
            let enabled = loop {
                let current_auth_generation = auth_generation(self.auth_manager.as_ref());
                let policy = match cached_attribution_policy(
                    input.thread_store,
                    input.turn_store,
                    current_auth_generation,
                ) {
                    Some(policy) => policy,
                    None if retry_deferred(input.thread_store, current_auth_generation) => {
                        GitAttributionPolicy {
                            auth_generation: current_auth_generation,
                            enabled: false,
                        }
                    }
                    None => {
                        match resolve_attribution_policy(
                            &self.auth_manager,
                            &self.base_url,
                            &self.http_client_factory,
                        )
                        .await
                        {
                            Ok(Some(policy)) => {
                                input.thread_store.insert(policy.clone());
                                policy
                            }
                            Ok(None) => {
                                let policy = GitAttributionPolicy {
                                    auth_generation: current_auth_generation,
                                    enabled: false,
                                };
                                input.turn_store.insert(policy.clone());
                                policy
                            }
                            Err(_) => {
                                let auth_generation = auth_generation(self.auth_manager.as_ref());
                                if auth_generation == current_auth_generation {
                                    input.thread_store.insert(GitAttributionRetry {
                                        auth_generation,
                                        retry_at: Instant::now() + POLICY_RETRY_DELAY,
                                    });
                                }
                                GitAttributionPolicy {
                                    auth_generation: current_auth_generation,
                                    enabled: false,
                                }
                            }
                        }
                    }
                };
                if policy.auth_generation == auth_generation(self.auth_manager.as_ref()) {
                    break policy.enabled;
                }
                generation_retries += 1;
                if generation_retries >= MAX_AUTH_GENERATION_RETRIES {
                    break false;
                }
            };
            vec![git_attribution_world_state_section(enabled)]
        })
    }
}

/// Installs the git-attribution contributor into the extension registry.
pub fn install<C: Sync>(
    _registry: &mut ExtensionRegistryBuilder<C>,
    _auth_manager: Arc<AuthManager>,
    _base_url: String,
    _http_client_factory: HttpClientFactory,
) {
    // Myra does not use the hosted Codex commit-attribution policy. Registering
    // this contributor would probe the ChatGPT backend before every first turn;
    // Myra device credentials are (correctly) rejected there, and the auth recovery
    // path can delay or block the actual model request. Keep attribution disabled;
    // normal git author configuration remains untouched.
}

#[cfg(test)]
#[path = "git_attribution_tests.rs"]
mod tests;
