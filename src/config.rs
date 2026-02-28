//! Configuration types for lm-gateway.
//!
//! Config is loaded once at startup from a TOML file and validated before the
//! server opens any ports. Invalid configs are rejected with a clear error
//! rather than silently falling back to defaults.
//!
//! # Example
//! ```toml
//! [gateway]
//! client_port = 8080
//!
//! [backends.ollama]
//! base_url = "http://localhost:11434"
//!
//! [[tiers]]
//! name    = "local:fast"
//! backend = "ollama"
//! model   = "qwen2.5:1.5b"
//!
//! [aliases]
//! "hint:fast" = "local:fast"
//!
//! [profiles.default]
//! mode           = "dispatch"
//! classifier     = "local:fast"
//! max_auto_tier  = "local:fast"
//! ```

use std::{collections::HashMap, path::Path};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Which API protocol a backend speaks.
///
/// lm-gateway normalises all inter-agent traffic to OpenAI's chat-completions
/// schema; each [`Provider`] variant maps to an adapter that handles any
/// necessary request/response translation at the edge.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Standard OpenAI `/v1/chat/completions` protocol.
    /// Also used by LM Studio, vLLM, LocalAI, and many others.
    #[default]
    OpenAI,
    /// OpenRouter — OpenAI-compatible wire format.
    /// Kept as a distinct variant so the router can inject the
    /// `HTTP-Referer` and `X-Title` headers that OpenRouter recommends.
    OpenRouter,
    /// Ollama local inference server. Uses Ollama's OpenAI-compat endpoint
    /// by default; future versions may use the native `/api/chat` path.
    Ollama,
    /// Anthropic Messages API (`/v1/messages`).
    /// Request and response shapes are translated to/from the OpenAI schema.
    Anthropic,
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OpenAI => "openai",
            Self::OpenRouter => "openrouter",
            Self::Ollama => "ollama",
            Self::Anthropic => "anthropic",
        })
    }
}

/// Top-level gateway configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub gateway: GatewayConfig,

    /// Named backends (Ollama, OpenRouter, Anthropic direct, etc.)
    #[serde(default)]
    pub backends: HashMap<String, BackendConfig>,

    /// Routing tiers — ordered ladder used for escalation.
    #[serde(default)]
    pub tiers: Vec<TierConfig>,

    /// Model/alias → tier name mappings.
    ///
    /// Clients send `model = "hint:fast"` — this maps it to the `local:fast` tier.
    #[serde(default)]
    pub aliases: HashMap<String, String>,

    /// Named routing profiles. The `default` profile is used when no client key is matched.
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Self = toml::from_str(&content).context("parsing config TOML")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        // Every tier must reference a known backend
        for tier in &self.tiers {
            anyhow::ensure!(
                self.backends.contains_key(&tier.backend),
                "tier `{}` references unknown backend `{}`",
                tier.name,
                tier.backend
            );
        }

        // Every alias must map to a known tier
        let tier_names: std::collections::HashSet<&str> =
            self.tiers.iter().map(|t| t.name.as_str()).collect();
        for (alias, tier) in &self.aliases {
            anyhow::ensure!(
                tier_names.contains(tier.as_str()),
                "alias `{}` maps to unknown tier `{}`",
                alias,
                tier
            );
        }

        // Every profile classifier must be a known tier
        for (name, profile) in &self.profiles {
            anyhow::ensure!(
                tier_names.contains(profile.classifier.as_str()),
                "profile `{}` classifier references unknown tier `{}`",
                name,
                profile.classifier
            );
        }

        Ok(())
    }

    /// Resolve a model string to a [`TierConfig`], following alias indirection.
    ///
    /// Lookup order:
    /// 1. Try `model` as an alias key → follow to tier name.
    /// 2. Try `model` as a direct tier name.
    /// 3. Return `None` if neither matches.
    pub fn resolve_tier<'a>(&'a self, model: &'a str) -> Option<&'a TierConfig> {
        let tier_name = self.aliases.get(model).map(|s| s.as_str()).unwrap_or(model);
        self.tiers.iter().find(|t| t.name == tier_name)
    }

    /// Return the named profile, falling back to `"default"`.
    ///
    /// Returns `None` only if neither the named profile nor a `"default"` profile exists.
    pub fn profile(&self, name: &str) -> Option<&ProfileConfig> {
        self.profiles.get(name).or_else(|| self.profiles.get("default"))
    }
}

/// Core gateway settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GatewayConfig {
    /// Port for the agent-facing client API (default: 8080).
    #[serde(default = "defaults::client_port")]
    pub client_port: u16,

    /// Port for the admin API + web UI (default: 8081).
    #[serde(default = "defaults::admin_port")]
    pub admin_port: u16,

    /// Number of recent requests to keep in the in-memory traffic log (default: 500).
    #[serde(default = "defaults::traffic_log_capacity")]
    pub traffic_log_capacity: usize,

    /// Log level override (also controlled by `RUST_LOG` env var).
    #[serde(default)]
    pub log_level: Option<String>,

    /// Maximum requests per minute per client IP on the client port.
    ///
    /// Leave unset (or set to 0) to disable rate limiting.
    /// The burst allowance equals half of this value, rounded up,
    /// so `rate_limit_rpm = 60` allows 60 req/min sustained and up to
    /// 30 back-to-back requests before the bucket empties.
    #[serde(default)]
    pub rate_limit_rpm: Option<u32>,

    /// Environment variable whose value is the Bearer token required for all
    /// admin API requests. Leave unset to disable admin authentication (only
    /// recommended when the admin port is strictly firewalled).
    ///
    /// Example: `admin_token_env = "LMG_ADMIN_TOKEN"`.
    #[serde(default)]
    pub admin_token_env: Option<String>,
}

/// A named backend (Ollama instance, OpenRouter, Anthropic direct, etc.).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BackendConfig {
    /// Base URL — must end without a trailing `/v1` (added by the client).
    pub base_url: String,

    /// Environment variable name whose value is the API key.
    ///
    /// Leave unset for keyless local backends (e.g., Ollama with no auth).
    #[serde(default)]
    pub api_key_env: Option<String>,

    /// Request timeout in milliseconds (default: 30 000).
    #[serde(default = "defaults::timeout_ms")]
    pub timeout_ms: u64,

    /// Protocol adapter to use when talking to this backend.
    ///
    /// Defaults to [`Provider::OpenAI`] (passthrough). Set to `"anthropic"`
    /// for direct Anthropic API access, `"ollama"` for local Ollama, or
    /// `"openrouter"` to enable OpenRouter-specific headers.
    #[serde(default)]
    pub provider: Provider,
}

impl BackendConfig {
    /// Resolve the API key from the configured environment variable.
    pub fn api_key(&self) -> Option<String> {
        self.api_key_env
            .as_deref()
            .and_then(|var| std::env::var(var).ok())
    }
}

/// A routing tier — a named combination of backend + model.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TierConfig {
    /// Unique tier name, e.g. `local:fast`, `cloud:economy`.
    pub name: String,

    /// Which backend to use (must exist in `[backends]`).
    pub backend: String,

    /// Model name to send to the backend.
    pub model: String,
}

/// Routing profile — controls routing behaviour for a client.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileConfig {
    /// Routing mode.
    #[serde(default)]
    pub mode: RoutingMode,

    /// Tier used for pre-classification (must be a fast local tier).
    pub classifier: String,

    /// Highest tier auto-escalation can reach without an explicit override.
    pub max_auto_tier: String,

    /// If true, the `cloud:expert` tier (or highest tier) requires an explicit
    /// `"tier": "expert"` field in the request body or a custom header.
    #[serde(default)]
    pub expert_requires_flag: bool,
}

/// How the routing decision is made.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    /// Pre-classify with the classifier tier, then dispatch to the resolved tier.
    ///
    /// Classifier never answers — it only routes. Adds ~200–800 ms latency.
    #[default]
    Dispatch,

    /// Try each tier from cheapest upward. Return the first sufficient response.
    ///
    /// "Sufficient" is determined by heuristics (response length, absence of
    /// refusal phrases). Reduces cost for simple queries.
    Escalate,
}

impl std::fmt::Display for RoutingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Dispatch => "dispatch",
            Self::Escalate => "escalate",
        })
    }
}

mod defaults {
    pub fn client_port() -> u16 { 8080 }
    pub fn admin_port() -> u16 { 8081 }
    pub fn traffic_log_capacity() -> usize { 500 }
    pub fn timeout_ms() -> u64 { 30_000 }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn minimal_config() -> Config {
        toml::from_str(
            r#"
            [backends.ollama]
            base_url = "http://localhost:11434"

            [[tiers]]
            name    = "local:fast"
            backend = "ollama"
            model   = "qwen2.5:1.5b"

            [[tiers]]
            name    = "cloud:economy"
            backend = "ollama"
            model   = "qwen2.5:7b"

            [aliases]
            "hint:fast"  = "local:fast"
            "hint:cloud" = "cloud:economy"

            [profiles.default]
            mode          = "dispatch"
            classifier    = "local:fast"
            max_auto_tier = "cloud:economy"
            "#,
        )
        .expect("minimal config should parse")
    }

    // -----------------------------------------------------------------------
    // Parsing & validation
    // -----------------------------------------------------------------------

    #[test]
    fn parse_example_config() {
        let content = include_str!("../config.example.toml");
        let config: Config = toml::from_str(content).expect("example config should parse");
        config.validate().expect("example config should be valid");
    }

    #[test]
    fn validation_rejects_tier_with_unknown_backend() {
        let mut config = minimal_config();
        config.tiers.push(TierConfig {
            name: "bad:tier".into(),
            backend: "nonexistent".into(),
            model: "x".into(),
        });
        assert!(config.validate().is_err());
    }

    #[test]
    fn validation_rejects_alias_pointing_to_unknown_tier() {
        let mut config = minimal_config();
        config.aliases.insert("bad:alias".into(), "no-such-tier".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn validation_rejects_profile_with_unknown_classifier() {
        let mut config = minimal_config();
        config.profiles.insert(
            "bad".into(),
            ProfileConfig {
                mode: RoutingMode::Dispatch,
                classifier: "no-such-tier".into(),
                max_auto_tier: "local:fast".into(),
                expert_requires_flag: false,
            },
        );
        assert!(config.validate().is_err());
    }

    // -----------------------------------------------------------------------
    // Tier resolution
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_tier_by_direct_name() {
        let config = minimal_config();
        let tier = config.resolve_tier("local:fast");
        assert!(tier.is_some());
        assert_eq!(tier.unwrap().name, "local:fast");
    }

    #[test]
    fn resolve_tier_via_alias() {
        let config = minimal_config();
        let tier = config.resolve_tier("hint:fast");
        assert!(tier.is_some());
        assert_eq!(tier.unwrap().name, "local:fast");
    }

    #[test]
    fn resolve_tier_returns_none_for_unknown() {
        let config = minimal_config();
        assert!(config.resolve_tier("completely:unknown").is_none());
    }

    // -----------------------------------------------------------------------
    // Profile lookup
    // -----------------------------------------------------------------------

    #[test]
    fn profile_returns_named_profile_when_present() {
        let config = minimal_config();
        assert!(config.profile("default").is_some());
    }

    #[test]
    fn profile_falls_back_to_default_for_unknown_name() {
        let config = minimal_config();
        // "nonexistent" doesn't exist, should fall back to "default"
        assert!(config.profile("nonexistent").is_some());
    }

    #[test]
    fn profile_returns_none_when_neither_named_nor_default_exists() {
        let mut config = minimal_config();
        config.profiles.clear();
        assert!(config.profile("anything").is_none());
    }

    // -----------------------------------------------------------------------
    // Routing mode deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn routing_mode_deserializes_from_snake_case() {
        let dispatch: RoutingMode = toml::from_str("mode = \"dispatch\"").unwrap();
        assert_eq!(dispatch, RoutingMode::Dispatch);

        let escalate: RoutingMode = toml::from_str("mode = \"escalate\"").unwrap();
        assert_eq!(escalate, RoutingMode::Escalate);
    }

    #[test]
    fn gateway_defaults_are_applied_when_section_is_minimal() {
        let config: Config = toml::from_str(
            r#"
            [backends.x]
            base_url = "http://x"
            [[tiers]]
            name = "t" ; backend = "x" ; model = "m"
            [profiles.default]
            classifier = "t" ; max_auto_tier = "t"
            "#,
        )
        .expect("should parse");
        assert_eq!(config.gateway.client_port, 8080);
        assert_eq!(config.gateway.admin_port, 8081);
        assert_eq!(config.gateway.traffic_log_capacity, 500);
    }
}
