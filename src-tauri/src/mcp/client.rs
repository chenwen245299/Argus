//! The other direction: Argus as an MCP *client*.
//!
//! `server.rs` hands the library to other people's agents. This module hands
//! other people's tools to the agent inside Argus, so 智能问答 can reach a web
//! search, a code sandbox, or whatever else the user has configured, alongside
//! its own library tools.
//!
//! # Shape
//!
//! Each configured server is a stdio subprocess Argus launches, exactly the way
//! Claude Desktop launches Argus. A connection lives for one answer: opened when
//! the agent loop starts, closed when it returns. Holding processes open across
//! idle time would leave the user with a handful of node processes running for
//! a chat window they stopped using.
//!
//! # Naming
//!
//! Tools reach the model under `prefix__tool`, where the prefix comes from the
//! server's name. Two reasons: the model can see which server it is calling, and
//! a server whose tool is called `search` cannot shadow one of ours. Providers
//! constrain function names to `[A-Za-z0-9_-]{1,64}`, which `namespaced`
//! enforces — a name the provider rejects fails the *whole* request, not just
//! that one tool.
//!
//! # Trust
//!
//! A configured server is an arbitrary program launched with the user's
//! privileges. That is inherent to MCP's stdio transport and true of every
//! client that speaks it; the protection is that the list is empty until the
//! user puts something in it, and each entry can be switched off without being
//! deleted.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::RoleClient;
use serde::{Deserialize, Serialize};
use tauri_plugin_store::StoreExt;

use super::agent::AgentTool;

const SERVERS_KEY: &str = "mcp_external_servers";
const ROUNDS_KEY: &str = "agent_max_rounds";
const KEEP_WARM_KEY: &str = "agent_keep_cache_warm";
const SYSTEM_PROMPT_KEY: &str = "agent_system_prompt";

/// A server that refuses to start must not hold the answer hostage.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A single tool call. Generous — some servers fetch from the network — but
/// finite, because the agent loop cannot be cancelled from inside a hung await.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Upper bound on configured servers. Every enabled one is a process spawned per
/// answer, so this is a guard against a config that makes the app unusable.
pub const MAX_SERVERS: usize = 20;

/// Provider limit on a function name.
const MAX_TOOL_NAME: usize = 64;

// ── Configuration ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalServer {
    pub id: String,
    /// Shown in the UI and used to derive the tool prefix.
    pub name: String,
    /// Executable to launch. Absolute paths are safest — a GUI app inherits a
    /// minimal `PATH` (see `augmented_path`).
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl ExternalServer {
    fn is_runnable(&self) -> bool {
        !self.command.trim().is_empty()
    }
}

/// Everything the 智能问答 settings tab owns.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettings {
    /// Tool rounds the model may take per answer.
    pub max_rounds: usize,
    /// Whether to hold the last conversation's prompt cache open between
    /// questions. See `crate::cache_keepalive` for what it costs.
    #[serde(default = "yes")]
    pub keep_cache_warm: bool,
    /// The agent's system prompt. Empty means "use the built-in default", so
    /// clearing the box returns to it rather than leaving the model with no
    /// instructions at all.
    #[serde(default)]
    pub system_prompt: String,
    pub servers: Vec<ExternalServer>,
}

impl Default for AgentSettings {
    fn default() -> Self {
        AgentSettings {
            max_rounds: crate::copilot::DEFAULT_AGENT_ROUNDS,
            keep_cache_warm: true,
            system_prompt: String::new(),
            servers: Vec::new(),
        }
    }
}

pub fn read_settings(app: &tauri::AppHandle) -> AgentSettings {
    let Ok(store) = app.store("settings.json") else {
        return AgentSettings::default();
    };
    let servers = store
        .get(SERVERS_KEY)
        .and_then(|v| serde_json::from_value::<Vec<ExternalServer>>(v).ok())
        .unwrap_or_default();
    let max_rounds = store
        .get(ROUNDS_KEY)
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(crate::copilot::DEFAULT_AGENT_ROUNDS);
    AgentSettings {
        max_rounds: crate::copilot::clamp_agent_rounds(Some(max_rounds)),
        keep_cache_warm: store
            .get(KEEP_WARM_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        system_prompt: store
            .get(SYSTEM_PROMPT_KEY)
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default(),
        servers,
    }
}

pub fn write_settings(app: &tauri::AppHandle, settings: &AgentSettings) -> Result<(), String> {
    let store = app
        .store("settings.json")
        .map_err(|e| format!("open settings store: {e}"))?;
    let servers: Vec<ExternalServer> = settings
        .servers
        .iter()
        .filter(|s| s.is_runnable())
        .take(MAX_SERVERS)
        .cloned()
        .collect();
    store.set(
        SERVERS_KEY,
        serde_json::to_value(&servers).map_err(|e| format!("serialize servers: {e}"))?,
    );
    store.set(
        ROUNDS_KEY,
        serde_json::json!(crate::copilot::clamp_agent_rounds(Some(settings.max_rounds))),
    );
    store.set(KEEP_WARM_KEY, serde_json::json!(settings.keep_cache_warm));
    // Stored trimmed: trailing whitespace would change the prompt bytes and so
    // the cached prefix, for no visible difference.
    store.set(
        SYSTEM_PROMPT_KEY,
        serde_json::json!(settings.system_prompt.trim()),
    );
    store.save().map_err(|e| format!("save settings: {e}"))
}

// ── Tool naming ──────────────────────────────────────────────────────────────

/// Reduce a server name to something usable as the leading part of a function
/// name. Non-ASCII names (which most of this app's users will type) collapse to
/// nothing, so the caller falls back to a positional label.
fn prefix_from(name: &str) -> Option<String> {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let slug = slug.trim_matches('_').to_string();
    let slug: String = slug.chars().take(20).collect();
    let slug = slug.trim_matches('_').to_string();
    (!slug.is_empty()).then_some(slug)
}

/// `prefix__tool`, sanitized and bounded, and unique within `taken`.
fn namespaced(prefix: &str, tool: &str, taken: &mut HashSet<String>) -> String {
    let raw = format!("{prefix}__{tool}");
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let base: String = cleaned.chars().take(MAX_TOOL_NAME).collect();
    let mut candidate = base.clone();
    let mut n = 2;
    // Truncation can collide two distinct long tool names; disambiguate rather
    // than dropping one of them silently.
    while !taken.insert(candidate.clone()) {
        let suffix = format!("_{n}");
        let keep = MAX_TOOL_NAME.saturating_sub(suffix.len());
        candidate = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
        n += 1;
    }
    candidate
}

// ── Connecting ───────────────────────────────────────────────────────────────

/// A server that could not be reached, reported rather than swallowed: the user
/// configured it and deserves to know it did not load.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerFailure {
    pub name: String,
    pub error: String,
}

/// One tool a probe found, with the only property that matters at a glance:
/// whether it can change anything.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbedTool {
    pub name: String,
    pub read_only: bool,
}

/// What a probe found, for the "测试" button in settings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub ok: bool,
    pub tools: Vec<ProbedTool>,
    pub error: Option<String>,
}

struct Route {
    /// Index into `ToolBridge::services`.
    service: usize,
    /// The name the far side knows the tool by, without our prefix.
    tool: String,
    /// Server name, for the UI.
    server: String,
}

/// Live connections to every enabled external server, for the span of one answer.
pub struct ToolBridge {
    services: Vec<RunningService<RoleClient, ()>>,
    routes: HashMap<String, Route>,
    tools: Vec<AgentTool>,
    failures: Vec<ServerFailure>,
}

impl ToolBridge {
    /// An empty bridge — the common case, since the server list starts empty.
    pub fn none() -> Self {
        ToolBridge {
            services: Vec::new(),
            routes: HashMap::new(),
            tools: Vec::new(),
            failures: Vec::new(),
        }
    }

    /// Launch every enabled server and collect its tools.
    ///
    /// A server that fails to start, or whose tools cannot be listed, is
    /// recorded in `failures` and skipped. One broken entry must not cost the
    /// user the tools that do work.
    pub async fn connect(servers: &[ExternalServer]) -> Self {
        let mut bridge = ToolBridge::none();
        let mut taken: HashSet<String> = HashSet::new();

        for (i, cfg) in servers
            .iter()
            .filter(|s| s.enabled && s.is_runnable())
            .take(MAX_SERVERS)
            .enumerate()
        {
            let prefix = prefix_from(&cfg.name).unwrap_or_else(|| format!("mcp{}", i + 1));
            match connect_one(cfg).await {
                Ok((service, tools)) => {
                    let idx = bridge.services.len();
                    for tool in tools {
                        let original = tool.name.to_string();
                        let name = namespaced(&prefix, &original, &mut taken);
                        let read_only = super::agent::declares_read_only(&tool);
                        bridge.tools.push(AgentTool {
                            description: tool
                                .description
                                .map(|d| d.to_string())
                                .unwrap_or_else(|| format!("{} · {original}", cfg.name)),
                            input_schema: object_schema((*tool.input_schema).clone()),
                            name: name.clone(),
                            read_only,
                        });
                        bridge.routes.insert(
                            name,
                            Route {
                                service: idx,
                                tool: original,
                                server: cfg.name.clone(),
                            },
                        );
                    }
                    bridge.services.push(service);
                }
                Err(e) => bridge.failures.push(ServerFailure {
                    name: cfg.name.clone(),
                    error: e,
                }),
            }
        }
        bridge
    }

    pub fn tools(&self) -> &[AgentTool] {
        &self.tools
    }

    pub fn failures(&self) -> &[ServerFailure] {
        &self.failures
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Whether `name` belongs to an external server rather than the library.
    pub fn handles(&self, name: &str) -> bool {
        self.routes.contains_key(name)
    }

    /// Which server a namespaced tool came from, for the UI trail.
    pub fn server_of(&self, name: &str) -> Option<&str> {
        self.routes.get(name).map(|r| r.server.as_str())
    }

    pub async fn call(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let route = self
            .routes
            .get(name)
            .ok_or_else(|| format!("unknown tool: {name}"))?;
        let service = self
            .services
            .get(route.service)
            .ok_or_else(|| format!("server for '{name}' is no longer connected"))?;

        let mut params = CallToolRequestParams::new(route.tool.clone());
        params.arguments = args.as_object().cloned();

        let result = tokio::time::timeout(CALL_TIMEOUT, service.call_tool(params))
            .await
            .map_err(|_| format!("'{name}' timed out after {}s", CALL_TIMEOUT.as_secs()))?
            .map_err(|e| format!("{name}: {e}"))?;

        if result.is_error.unwrap_or(false) {
            return Err(text_of(&result).unwrap_or_else(|| format!("{name} reported an error")));
        }
        if let Some(structured) = result.structured_content {
            return Ok(structured);
        }
        match text_of(&result) {
            // Most servers return JSON as text; parse it so the model sees
            // structure rather than an escaped string.
            Some(text) => Ok(serde_json::from_str(&text)
                .unwrap_or_else(|_| serde_json::Value::String(text))),
            None => Ok(serde_json::to_value(&result.content).unwrap_or(serde_json::Value::Null)),
        }
    }

    /// Close every child process. Skipping this leaves orphans behind for the
    /// rest of the app's lifetime, one set per answer.
    pub async fn shutdown(self) {
        for service in self.services {
            let _ = service.cancel().await;
        }
    }
}

fn text_of(result: &rmcp::model::CallToolResult) -> Option<String> {
    let rendered = serde_json::to_value(&result.content).ok()?;
    let parts: Vec<String> = rendered
        .as_array()?
        .iter()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()).map(str::to_string))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Providers reject a function whose parameter schema is not an object, and the
/// rejection takes the whole request with it — so a server that declares
/// something else gets a permissive object instead of poisoning the turn.
fn object_schema(schema: serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    if schema.get("type").and_then(|v| v.as_str()) == Some("object") {
        return serde_json::Value::Object(schema);
    }
    serde_json::json!({ "type": "object", "properties": {} })
}

async fn connect_one(
    cfg: &ExternalServer,
) -> Result<(RunningService<RoleClient, ()>, Vec<rmcp::model::Tool>), String> {
    use rmcp::ServiceExt;

    let mut command = tokio::process::Command::new(cfg.command.trim());
    command.args(cfg.args.iter().filter(|a| !a.is_empty()));
    for (k, v) in &cfg.env {
        if !k.is_empty() {
            command.env(k, v);
        }
    }
    command.env("PATH", augmented_path());
    // Killing the parent must not leave the child running.
    command.kill_on_drop(true);

    let transport = rmcp::transport::TokioChildProcess::new(command)
        .map_err(|e| format!("cannot launch `{}`: {e}", cfg.command))?;

    let service = tokio::time::timeout(CONNECT_TIMEOUT, ().serve(transport))
        .await
        .map_err(|_| {
            format!(
                "no response within {}s — is `{}` an MCP server?",
                CONNECT_TIMEOUT.as_secs(),
                cfg.command
            )
        })?
        .map_err(|e| format!("handshake failed: {e}"))?;

    let tools = match tokio::time::timeout(CONNECT_TIMEOUT, service.list_all_tools()).await {
        Ok(Ok(tools)) => tools,
        Ok(Err(e)) => {
            let _ = service.cancel().await;
            return Err(format!("cannot list tools: {e}"));
        }
        Err(_) => {
            let _ = service.cancel().await;
            return Err("timed out listing tools".to_string());
        }
    };
    Ok((service, tools))
}

/// `PATH` for the child, widened past what a GUI app inherits.
///
/// Launched from Finder or the Dock, a macOS app gets `/usr/bin:/bin:/usr/sbin:
/// /sbin` and nothing else — so `npx`, `uvx` and `bunx`, which is what nearly
/// every MCP server is distributed as, are simply not found. Absolute paths
/// always work; this makes the common case work too.
fn augmented_path() -> String {
    let mut dirs: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    let mut extra = vec![
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        for rel in [".local/bin", ".bun/bin", ".cargo/bin", ".volta/bin"] {
            extra.push(home.join(rel).to_string_lossy().to_string());
        }
    }
    for dir in extra {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.join(":")
}

/// Start a server, list its tools, shut it down. Backs the settings "测试" button.
pub async fn probe(cfg: &ExternalServer) -> ProbeResult {
    if !cfg.is_runnable() {
        return ProbeResult {
            ok: false,
            tools: Vec::new(),
            error: Some("命令为空".to_string()),
        };
    }
    match connect_one(cfg).await {
        Ok((service, tools)) => {
            let found = tools
                .iter()
                .map(|t| ProbedTool {
                    name: t.name.to_string(),
                    read_only: super::agent::declares_read_only(t),
                })
                .collect();
            let _ = service.cancel().await;
            ProbeResult {
                ok: true,
                tools: found,
                error: None,
            }
        }
        Err(e) => ProbeResult {
            ok: false,
            tools: Vec::new(),
            error: Some(e),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_the_provider_would_reject_never_reaches_it() {
        let mut taken = HashSet::new();
        let name = namespaced("web search", "fetch page!", &mut taken);
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "illegal characters survived: {name}"
        );
        assert!(name.len() <= MAX_TOOL_NAME, "too long: {name}");
    }

    #[test]
    fn long_names_are_cut_to_the_limit_and_stay_unique() {
        let mut taken = HashSet::new();
        let prefix = "s";
        let long = "a".repeat(200);
        let first = namespaced(prefix, &long, &mut taken);
        let second = namespaced(prefix, &long, &mut taken);
        assert_eq!(first.len(), MAX_TOOL_NAME);
        assert!(second.len() <= MAX_TOOL_NAME);
        assert_ne!(first, second, "truncation collapsed two tools into one name");
    }

    /// Most of this app's users name things in Chinese, which sanitizes to
    /// nothing — the prefix has to come from somewhere else then.
    #[test]
    fn a_non_ascii_server_name_yields_no_prefix() {
        assert_eq!(prefix_from("网页搜索"), None);
        assert_eq!(prefix_from("Web Search"), Some("web_search".to_string()));
        assert_eq!(prefix_from("---"), None);
    }

    /// The prefix is what stops an external `search_papers` from shadowing ours.
    #[test]
    fn external_tools_cannot_collide_with_library_tools() {
        let mut taken = HashSet::new();
        let name = namespaced("other", "search_papers", &mut taken);
        assert_ne!(name, "search_papers");
        assert!(name.starts_with("other__"));
    }

    #[test]
    fn a_non_object_schema_is_replaced_rather_than_passed_on() {
        let mut array = serde_json::Map::new();
        array.insert("type".into(), serde_json::json!("array"));
        assert_eq!(object_schema(array)["type"], "object");

        let mut obj = serde_json::Map::new();
        obj.insert("type".into(), serde_json::json!("object"));
        obj.insert("properties".into(), serde_json::json!({"q": {"type": "string"}}));
        assert!(object_schema(obj)["properties"]["q"].is_object(), "a valid schema was discarded");
    }

    #[test]
    fn the_default_configuration_has_nothing_in_it() {
        let s = AgentSettings::default();
        assert!(
            s.servers.is_empty(),
            "no external program may run without the user adding it"
        );
        assert_eq!(s.max_rounds, crate::copilot::DEFAULT_AGENT_ROUNDS);
    }

    #[tokio::test]
    async fn an_empty_configuration_connects_to_nothing() {
        let bridge = ToolBridge::connect(&[]).await;
        assert!(bridge.is_empty());
        assert!(bridge.failures().is_empty());
    }

    #[tokio::test]
    async fn a_disabled_server_is_never_launched() {
        let bridge = ToolBridge::connect(&[ExternalServer {
            id: "1".into(),
            name: "nope".into(),
            command: "/definitely/not/a/program".into(),
            args: vec![],
            env: BTreeMap::new(),
            enabled: false,
        }])
        .await;
        assert!(bridge.is_empty());
        assert!(
            bridge.failures().is_empty(),
            "a disabled server was launched anyway"
        );
    }

    #[tokio::test]
    async fn a_broken_server_is_reported_not_fatal() {
        let bridge = ToolBridge::connect(&[ExternalServer {
            id: "1".into(),
            name: "broken".into(),
            command: "/definitely/not/a/program".into(),
            args: vec![],
            env: BTreeMap::new(),
            enabled: true,
        }])
        .await;
        assert!(bridge.is_empty());
        assert_eq!(bridge.failures().len(), 1);
        assert_eq!(bridge.failures()[0].name, "broken");
    }

    /// End-to-end proof that this client can drive a real MCP server over a
    /// real pipe: it launches Argus's own stdio server and lists its tools.
    ///
    /// Ignored by default — it needs a built binary and the endpoint switched
    /// on. Run with `cargo test -- --ignored live_probe`.
    #[tokio::test]
    #[ignore]
    async fn live_probe_against_our_own_stdio_server() {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| Some(p.parent()?.parent()?.join("Argus")))
            .expect("cannot locate the Argus binary");
        assert!(exe.exists(), "build first: {exe:?}");

        let result = probe(&ExternalServer {
            id: "self".into(),
            name: "Argus itself".into(),
            command: exe.to_string_lossy().to_string(),
            args: vec![super::super::STDIO_FLAG.to_string()],
            env: BTreeMap::new(),
            enabled: true,
        })
        .await;

        assert!(result.ok, "probe failed: {:?}", result.error);
        let listing = result
            .tools
            .iter()
            .find(|t| t.name == "list_papers")
            .unwrap_or_else(|| panic!("the server answered but not with our tools: {:?}", result.tools));
        assert!(
            listing.read_only,
            "the read-only annotation did not survive the wire"
        );
    }

    #[test]
    fn the_child_path_keeps_what_it_was_given_and_adds_more() {
        let path = augmented_path();
        for dir in std::env::var("PATH").unwrap_or_default().split(':') {
            if !dir.is_empty() {
                assert!(path.contains(dir), "dropped {dir} from PATH");
            }
        }
        assert!(path.contains("/usr/local/bin"), "{path}");
    }
}

#[cfg(test)]
mod wire_tests {
    /// The settings tab reads these key names directly; a rename here would
    /// leave it showing defaults with no error.
    #[test]
    fn the_settings_payload_uses_the_names_the_ui_reads() {
        let view = crate::commands::AgentSettingsView {
            settings: super::AgentSettings::default(),
            builtin_tool_count: 17,
            default_system_prompt: crate::copilot::DEFAULT_AGENT_SYSTEM_PROMPT,
            min_rounds: 1,
            max_rounds_limit: 50,
            max_servers: 20,
        };
        let json = serde_json::to_value(&view).unwrap();
        for key in [
            "maxRounds",
            "servers",
            "builtinToolCount",
            "minRounds",
            "maxRoundsLimit",
            "maxServers",
        ] {
            assert!(json.get(key).is_some(), "missing '{key}' in {json}");
        }

        // And the round-trip the save command performs.
        for key in ["keepCacheWarm", "systemPrompt", "defaultSystemPrompt"] {
            assert!(json.get(key).is_some(), "missing '{key}' in {json}");
        }

        let sent = serde_json::json!({ "maxRounds": 12, "keepCacheWarm": false,
            "systemPrompt": "Answer in haiku.", "servers": [
            { "id": "a", "name": "Web", "command": "npx", "args": ["-y", "x"],
              "env": { "K": "v" }, "enabled": true }
        ]});
        let parsed: super::AgentSettings = serde_json::from_value(sent).unwrap();
        assert_eq!(parsed.max_rounds, 12);
        assert_eq!(parsed.servers[0].command, "npx");
        assert_eq!(parsed.servers[0].env.get("K").map(String::as_str), Some("v"));
        assert!(!parsed.keep_cache_warm);
        assert_eq!(parsed.system_prompt, "Answer in haiku.");
    }
}
