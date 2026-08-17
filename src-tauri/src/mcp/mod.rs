//! A read-only MCP server exposing the literature library to external agents.
//!
//! # Shape
//!
//! This is a **stdio** server. The MCP client (Claude Code, Claude Desktop,
//! Codex) launches `Argus --mcp-stdio` as a subprocess and speaks
//! newline-delimited JSON-RPC over its stdin/stdout. There is no network
//! listener, no port to configure, and no token to manage — the operating
//! system's process boundary is the whole transport.
//!
//! It reads the library folder directly, so it works whether or not the Argus
//! window is open. The only control is the `mcp_enabled` switch below: with it
//! off, the server refuses to serve anything.
//!
//! # Concurrency with the running app
//!
//! Reads are safe alongside a running Argus. The JSON files are written
//! atomically (`fsutil::atomic_write_str` writes a `.tmp` then renames), and the
//! SQLite caches are opened in WAL mode, so a concurrent reader never sees a
//! torn file.
//!
//! Everything reachable from here goes through `tools`, which is where the
//! guarantee that AI provider config and API keys stay unreachable lives.

pub mod agent;
pub mod client;
mod server;
mod tools;

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri_plugin_store::StoreExt;

/// CLI flag that runs this instead of the GUI.
pub const STDIO_FLAG: &str = "--mcp-stdio";

const ENABLED_KEY: &str = "mcp_enabled";
/// Where the app records the library it has open. The stdio process reads the
/// same key so it serves whatever library the user last worked in.
const LAST_LIBRARY_KEY: &str = "last_library";

// ── Settings ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSettings {
    pub enabled: bool,
}

impl Default for McpSettings {
    fn default() -> Self {
        // Off by default. Exposing the user's whole library to other programs is
        // not something to switch on for them.
        McpSettings { enabled: false }
    }
}

pub fn read_settings(app: &tauri::AppHandle) -> McpSettings {
    let Ok(store) = app.store("settings.json") else {
        return McpSettings::default();
    };
    McpSettings {
        enabled: store
            .get(ENABLED_KEY)
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

pub fn write_settings(app: &tauri::AppHandle, settings: &McpSettings) -> Result<(), String> {
    let store = app
        .store("settings.json")
        .map_err(|e| format!("open settings store: {e}"))?;
    store.set(ENABLED_KEY, serde_json::json!(settings.enabled));
    store.save().map_err(|e| format!("save settings: {e}"))
}

// ── Library resolution ───────────────────────────────────────────────────────

/// Resolves the library to serve.
///
/// Kept as a trait so `server` has no idea where the path came from, which lets
/// tests drive the real tool surface against a temporary folder.
pub trait LibrarySource: Send + Sync + 'static {
    /// The library to serve, or the reason there is none — which the client
    /// shows to the user verbatim, so "switched off" and "nothing open" must
    /// not be reported as the same thing.
    fn root(&self) -> Result<String, String>;
}

/// Reads the library path the app recorded, on every call.
///
/// Per call rather than once at startup: the user may switch libraries in Argus
/// while an agent is connected, and the next tool call should follow them there.
struct StoredLibrary;

impl LibrarySource for StoredLibrary {
    fn root(&self) -> Result<String, String> {
        // Re-checked here, not just before `serve_stdio`. A switch the user can
        // only apply by quitting whatever they already connected is not a
        // switch; an agent that was running when they turned MCP off would keep
        // full read access to the library until it happened to disconnect.
        if !enabled_on_disk() {
            return Err("The Argus MCP server has been switched off. \
                        Turn it back on in Argus → Settings → MCP."
                .to_string());
        }
        // The env var is an escape hatch for pointing a second agent at a
        // different library without touching the app's own state.
        if let Ok(path) = std::env::var("ARGUS_LIBRARY") {
            if !path.is_empty() {
                return Ok(path);
            }
        }
        read_app_store()
            .and_then(|s| s.get(LAST_LIBRARY_KEY)?.as_str().map(|s| s.to_string()))
            .ok_or_else(|| {
                "No library is currently open in Argus. Ask the user to open one, then retry."
                    .to_string()
            })
    }
}

/// The bundle identifier the Tauri store writes under.
const IDENTIFIER: &str = "com.argus.app";

/// Locate `settings.json` without a Tauri application.
///
/// This process is a plain CLI invocation of the same binary — there is no app
/// to ask for its config directory — so the path Tauri would compute is
/// reproduced here.
fn app_store_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .map(|h| h.join("Library").join("Application Support"));

    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(std::path::PathBuf::from);

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|h| h.join(".config"))
        });

    base.map(|b| b.join(IDENTIFIER).join("settings.json"))
}

fn read_app_store() -> Option<serde_json::Map<String, serde_json::Value>> {
    let raw = std::fs::read_to_string(app_store_path()?).ok()?;
    match serde_json::from_str(&raw).ok()? {
        serde_json::Value::Object(o) => Some(o),
        _ => None,
    }
}

/// Whether the user has switched the endpoint on, read from disk.
///
/// The GUI writes this key; the stdio process reads it. That keeps the off
/// switch inside Argus even though nothing here depends on Argus running.
fn enabled_on_disk() -> bool {
    read_app_store()
        .and_then(|s| s.get(ENABLED_KEY).and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

// ── Client configuration ─────────────────────────────────────────────────────

/// Ready-to-paste config for the clients the user is likely to connect.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfig {
    /// Absolute path to this binary, which the client launches.
    pub executable: String,
    /// Shell command for `claude mcp add`.
    pub claude_code: String,
    /// Where Claude Desktop keeps its config on this platform.
    pub desktop_config_path: String,
    /// JSON block to merge into that file.
    pub desktop_snippet: String,
    /// Path to Codex's config file.
    pub codex_config_path: String,
    /// TOML block to merge into that file.
    pub codex_snippet: String,
}

/// Where Claude Desktop keeps its config, written the way documentation does.
///
/// Deliberately *not* expanded to an absolute path. `$HOME` would put the user's
/// account name on screen, and this panel is the first thing people screenshot
/// when asking for help.
fn desktop_config_path() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "%APPDATA%\\Claude\\claude_desktop_config.json"
    }
    #[cfg(target_os = "macos")]
    {
        "~/Library/Application Support/Claude/claude_desktop_config.json"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "~/.config/Claude/claude_desktop_config.json"
    }
}

/// Path to the running executable, which is what a client must launch.
///
/// Resolved at runtime rather than hardcoded: the app may be installed
/// anywhere, and during development it is the `target/debug` binary.
fn executable_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Argus".to_string())
}

fn desktop_snippet(exe: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "argus": {
                "command": exe,
                "args": [STDIO_FLAG]
            }
        }
    }))
    .unwrap_or_default()
}

/// Codex reads TOML rather than JSON, but wants the same command and args.
fn codex_snippet(exe: &str) -> String {
    // Escape backslashes so a Windows path stays a valid TOML basic string.
    let escaped = exe.replace('\\', "\\\\");
    format!(
        "[mcp_servers.argus]\ncommand = \"{escaped}\"\nargs = [\"{STDIO_FLAG}\"]\n"
    )
}

pub fn client_config() -> ClientConfig {
    let exe = executable_path();
    ClientConfig {
        // Quote the path: application bundles live under paths with spaces.
        claude_code: format!("claude mcp add argus \"{exe}\" {STDIO_FLAG}"),
        desktop_config_path: desktop_config_path().to_string(),
        desktop_snippet: desktop_snippet(&exe),
        codex_config_path: "~/.codex/config.toml".to_string(),
        codex_snippet: codex_snippet(&exe),
        executable: exe,
    }
}

// ── stdio server ─────────────────────────────────────────────────────────────

/// Serve MCP over stdin/stdout until the client disconnects. Returns an exit code.
pub fn run_stdio() -> i32 {
    // A terminal on stdin means someone ran the flag by hand. Explain the mode
    // rather than silently waiting for JSON-RPC that will never arrive.
    if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprintln!(
            "Argus MCP server (stdio).\n\n\
             This is launched by an MCP client, not run directly. Add it with:\n\n  \
             {}\n\n\
             or see Argus → Settings → MCP for the Claude Desktop config.",
            client_config().claude_code
        );
        return 2;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[mcp] cannot start runtime: {e}");
            return 1;
        }
    };

    runtime.block_on(async {
        if !enabled_on_disk() {
            // Exit rather than serving an empty tool list: the client shows the
            // startup failure, and the message says how to fix it.
            eprintln!(
                "[mcp] The Argus MCP server is switched off. Enable it in \
                 Argus → Settings → MCP."
            );
            return 1;
        }

        match serve_stdio(Arc::new(StoredLibrary)).await {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("[mcp] {e}");
                1
            }
        }
    })
}

async fn serve_stdio(library: Arc<dyn LibrarySource>) -> Result<(), String> {
    use rmcp::ServiceExt;

    let service = server::ArgusMcpServer::new(library)
        .serve(rmcp::transport::io::stdio())
        .await
        .map_err(|e| format!("failed to start MCP server: {e}"))?;

    service
        .waiting()
        .await
        .map_err(|e| format!("MCP server stopped: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_by_default() {
        assert!(
            !McpSettings::default().enabled,
            "the server must not be on without consent"
        );
    }

    /// The path shown to the user has to be the file Claude Desktop reads — and
    /// must not carry their account name, since this panel gets screenshotted.
    #[test]
    fn desktop_config_path_is_generic_and_correct() {
        let p = desktop_config_path();
        assert!(p.ends_with("claude_desktop_config.json"), "{p}");
        assert!(p.contains("Claude"), "{p}");

        if let Ok(home) = std::env::var("HOME") {
            assert!(!p.contains(&home), "the path leaks the home directory: {p}");
        }
        if let Ok(user) = std::env::var("USER") {
            assert!(
                user.is_empty() || !p.contains(&user),
                "the path leaks the account name: {p}"
            );
        }
    }

    /// Both snippets must launch the same binary with the same flag, or one of
    /// the two clients silently gets a different server.
    #[test]
    fn client_configs_agree_on_how_to_launch() {
        let exe = "/Applications/Argus.app/Contents/MacOS/Argus";
        let snippet = desktop_snippet(exe);
        let parsed: serde_json::Value = serde_json::from_str(&snippet).unwrap();
        let server = &parsed["mcpServers"]["argus"];

        assert_eq!(server["command"], exe);
        assert_eq!(server["args"], serde_json::json!([STDIO_FLAG]));
        // No `env`, no `url`, no token: a stdio server needs none of it.
        assert!(server.get("env").is_none(), "stdio config needs no env");
        assert!(server.get("url").is_none(), "stdio config needs no url");
    }

    /// A bundle path contains spaces, so the shell command has to quote it.
    #[test]
    fn claude_code_command_quotes_the_executable() {
        let cfg = ClientConfig {
            executable: "/Applications/My App/Argus".into(),
            claude_code: format!(
                "claude mcp add argus \"{}\" {STDIO_FLAG}",
                "/Applications/My App/Argus"
            ),
            desktop_config_path: desktop_config_path().into(),
            desktop_snippet: desktop_snippet("/Applications/My App/Argus"),
            codex_config_path: "~/.codex/config.toml".into(),
            codex_snippet: codex_snippet("/Applications/My App/Argus"),
        };
        assert!(
            cfg.claude_code.contains("\"/Applications/My App/Argus\""),
            "an unquoted path with spaces would be split into two arguments: {}",
            cfg.claude_code
        );
    }

    /// Codex must launch the same binary the other two do, and its TOML has to
    /// survive a path with backslashes (Windows) or spaces (app bundles).
    #[test]
    fn codex_snippet_is_valid_toml_for_awkward_paths() {
        let snippet = codex_snippet("/Applications/My App/Argus");
        assert!(snippet.contains("[mcp_servers.argus]"));
        assert!(snippet.contains("command = \"/Applications/My App/Argus\""));
        assert!(snippet.contains(&format!("args = [\"{STDIO_FLAG}\"]")));
        assert!(!snippet.contains("url"), "Codex config must not carry a url");

        let windows = codex_snippet("C:\\Program Files\\Argus\\Argus.exe");
        assert!(
            windows.contains("C:\\\\Program Files\\\\Argus\\\\Argus.exe"),
            "backslashes must be escaped for TOML: {windows}"
        );
    }

    #[test]
    fn store_path_targets_the_apps_own_settings() {
        let p = app_store_path().expect("no store path");
        assert!(p.ends_with(format!("{IDENTIFIER}/settings.json")), "{p:?}");
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use std::path::PathBuf;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    struct FixedLibrary(Option<String>);
    impl LibrarySource for FixedLibrary {
        fn root(&self) -> Result<String, String> {
            self.0
                .clone()
                .ok_or_else(|| "No library is currently open in Argus.".to_string())
        }
    }

    /// A throwaway library laid out the way `paper.rs` expects, with one paper
    /// and one conversation carrying fields that must be redacted.
    struct TempLibrary(PathBuf);

    impl TempLibrary {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("argus-mcp-stdio-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            let paper = dir.join("papers").join("attention-2017");
            std::fs::create_dir_all(paper.join("notes")).unwrap();
            std::fs::create_dir_all(dir.join(".argus")).unwrap();

            std::fs::write(
                paper.join("meta.json"),
                serde_json::json!({
                    "id": "paper-uuid-1",
                    "title": "Attention Is All You Need",
                    "authors": ["Ashish Vaswani"],
                    "year": 2017,
                    "doi": null,
                    "arxiv_id": "1706.03762",
                    "venue": "NeurIPS 2017",
                    "tags": ["transformer"],
                    "added_at": "2026-01-01T00:00:00+00:00",
                    "original_filename": "1706.03762.pdf",
                    "reading_status": "read",
                    "abstract": "We propose the Transformer."
                })
                .to_string(),
            )
            .unwrap();
            std::fs::write(
                paper.join("fulltext.txt"),
                "The dominant sequence transduction models are based on recurrent networks.",
            )
            .unwrap();
            std::fs::write(
                paper.join("ai_conversations.json"),
                serde_json::json!([{
                    "id": "conv_1",
                    "title": "Why self-attention?",
                    "slug": "attention-2017",
                    "createdAt": "2026-05-01T00:00:00Z",
                    "updatedAt": "2026-05-02T00:00:00Z",
                    "nodes": [
                        { "id": "n1", "role": "user", "content": "Why does it help?",
                          "createdAt": "2026-05-01T00:00:00Z",
                          "attachments": [{ "id": "a1", "name": "fig3.png", "type": "image",
                                            "dataUrl": "data:image/png;base64,LEAKCANARY" }] },
                        { "id": "n2", "role": "assistantGroup", "content": null,
                          "createdAt": "2026-05-01T00:01:00Z",
                          "answers": [{ "id": "ans1", "content": "It removes the recurrence bottleneck.",
                                        "modelName": "some-model",
                                        "providerId": "PROVIDER-CANARY",
                                        "costUsd": 0.0042,
                                        "reasoningContent": "REASONING-CANARY",
                                        "createdAt": "2026-05-01T00:01:00Z" }] }
                    ]
                }])
                .to_string(),
            )
            .unwrap();

            TempLibrary(dir)
        }

        fn path(&self) -> String {
            self.0.to_string_lossy().to_string()
        }
    }

    impl Drop for TempLibrary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Drive the real server over an in-memory duplex — the same code path
    /// `run_stdio` uses, minus the process boundary.
    ///
    /// Requests are queued into the pipe *before* `serve` is called: for a
    /// server, `serve` completes the initialize handshake before it returns, so
    /// waiting on it first would deadlock against an empty pipe.
    async fn exchange(
        library: Option<String>,
        requests: Vec<serde_json::Value>,
    ) -> Vec<serde_json::Value> {
        use rmcp::ServiceExt;
        use std::time::Duration;

        let (client_side, server_side) = tokio::io::duplex(1 << 20);
        let (read_half, mut write_half) = tokio::io::split(client_side);

        let expected = requests.iter().filter(|r| r.get("id").is_some()).count();
        for req in &requests {
            write_half
                .write_all(format!("{req}\n").as_bytes())
                .await
                .unwrap();
        }
        write_half.flush().await.unwrap();

        let (server_read, server_write) = tokio::io::split(server_side);
        let service = server::ArgusMcpServer::new(Arc::new(FixedLibrary(library)))
            .serve((server_read, server_write))
            .await
            .expect("server failed to start");
        let handle = tokio::spawn(async move {
            let _ = service.waiting().await;
        });

        let mut lines = BufReader::new(read_half).lines();
        let mut out = Vec::new();
        while out.len() < expected {
            // Bounded so a regression fails the test instead of hanging CI.
            match tokio::time::timeout(Duration::from_secs(10), lines.next_line()).await {
                Ok(Ok(Some(line))) if !line.trim().is_empty() => {
                    out.push(serde_json::from_str(&line).expect("server emitted non-JSON"))
                }
                Ok(Ok(Some(_))) => continue,
                _ => break,
            }
        }
        drop(write_half);
        handle.abort();
        out
    }

    fn initialize() -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "argus-test", "version": "0.0.0" }
            }
        })
    }

    fn initialized() -> serde_json::Value {
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    }

    #[tokio::test]
    async fn handshake_reports_this_server() {
        let out = exchange(None, vec![initialize()]).await;
        assert_eq!(out.len(), 1, "no response to initialize: {out:?}");
        assert_eq!(out[0]["result"]["serverInfo"]["name"], "argus");
        assert!(out[0]["result"]["instructions"].is_string());
    }

    #[tokio::test]
    async fn tools_list_survives_the_wire() {
        let out = exchange(
            None,
            vec![
                initialize(),
                initialized(),
                serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            ],
        )
        .await;

        let listing = out
            .iter()
            .find(|m| m["id"] == 1)
            .unwrap_or_else(|| panic!("no tools/list response: {out:?}"));
        let tools = listing["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 17, "unexpected tool count");

        // The two schema rules that made every tool vanish when broken.
        let wire = listing["result"].to_string();
        assert!(!wire.contains("$ref"), "a dangling $ref reached the client");
        assert!(!wire.contains("$defs"), "$defs reached the client");
        for tool in tools {
            assert_eq!(
                tool["outputSchema"]["type"], "object",
                "tool '{}' has a non-object outputSchema",
                tool["name"]
            );
        }
    }

    #[tokio::test]
    async fn reads_a_real_library() {
        let lib = TempLibrary::new("read");
        let out = exchange(
            Some(lib.path()),
            vec![
                initialize(),
                initialized(),
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": "list_papers", "arguments": {} }
                }),
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "get_paper_fulltext",
                                "arguments": { "slug": "attention-2017", "limit": 20 } }
                }),
            ],
        )
        .await;

        let page = out.iter().find(|m| m["id"] == 1).expect("no list_papers response");
        let structured = &page["result"]["structuredContent"];
        assert_eq!(structured["total"], 1, "{page}");
        assert_eq!(structured["papers"][0]["slug"], "attention-2017");
        // A listing has to say what a paper is *about*, not just name it, or the
        // reader must open every one to find the relevant few.
        assert_eq!(
            structured["papers"][0]["paper_abstract"], "We propose the Transformer.",
            "the abstract did not survive the wire: {structured}"
        );

        let slice = out.iter().find(|m| m["id"] == 2).expect("no fulltext response");
        let text = &slice["result"]["structuredContent"];
        assert_eq!(text["returned"], 20, "limit was not honoured: {text}");
        assert_eq!(text["has_more"], true);
    }

    #[tokio::test]
    async fn conversations_are_redacted_on_the_wire() {
        let lib = TempLibrary::new("redact");
        let out = exchange(
            Some(lib.path()),
            vec![
                initialize(),
                initialized(),
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": "get_conversation",
                                "arguments": { "conversation_id": "conv_1",
                                               "slug": "attention-2017" } }
                }),
            ],
        )
        .await;

        let detail = out.iter().find(|m| m["id"] == 1).expect("no response");
        let rendered = detail.to_string();
        for canary in ["LEAKCANARY", "PROVIDER-CANARY", "REASONING-CANARY", "0.0042"] {
            assert!(!rendered.contains(canary), "'{canary}' leaked: {rendered}");
        }

        let messages = &detail["result"]["structuredContent"]["messages"];
        assert_eq!(messages[0]["attachments"][0], "fig3.png");
        assert_eq!(messages[1]["model"], "some-model");
    }

    #[tokio::test]
    async fn a_missing_library_is_explained_not_swallowed() {
        let out = exchange(
            None,
            vec![
                initialize(),
                initialized(),
                serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": "list_papers", "arguments": {} }
                }),
            ],
        )
        .await;

        let reply = out.iter().find(|m| m["id"] == 1).expect("no response");
        assert!(
            reply.to_string().contains("No library is currently open"),
            "unhelpful response with no library: {reply}"
        );
    }
}
