use crate::models::{CliAnalysisEntry, CliPromptTemplate, CliSettings, CliTool};
use std::path::Path;

const CLI_KEY: &str = "cli_settings";

// ── Persistence ───────────────────────────────────────────────────────────────

pub fn read_settings(root: &str) -> CliSettings {
    let path = Path::new(root).join(".argus").join("config.json");
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let config: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
    serde_json::from_value(config.get(CLI_KEY).cloned().unwrap_or_default()).unwrap_or_default()
}

pub fn write_settings(root: &str, s: &CliSettings) -> Result<(), String> {
    let argus_dir = Path::new(root).join(".argus");
    std::fs::create_dir_all(&argus_dir).map_err(|e| e.to_string())?;
    let path = argus_dir.join("config.json");

    let mut config: serde_json::Value = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    config[CLI_KEY] = serde_json::to_value(s).map_err(|e| e.to_string())?;
    let content = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

// ── Tool detection ────────────────────────────────────────────────────────────

/// Detect known CLI tools in PATH, merge results into persisted settings, return updated list.
pub fn detect_tools(root: &str) -> Vec<CliTool> {
    let known: &[(&str, &str, &[&str])] = &[
        ("claude", "Claude Code", &["-p", "{prompt}"]),
        ("codex", "Codex CLI", &["-q", "{prompt}"]),
    ];

    let mut s = read_settings(root);

    for (cmd, name, default_args) in known {
        let found = probe_which(cmd);
        let version = if found { probe_version(cmd) } else { None };

        // Update existing entry (match by command name) or insert new preset.
        if let Some(existing) = s.tools.iter_mut().find(|t| t.command == *cmd) {
            existing.detected = found;
            existing.version = version;
        } else {
            s.tools.push(CliTool {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.to_string(),
                command: cmd.to_string(),
                args_template: default_args.iter().map(|a| a.to_string()).collect(),
                enabled: false,
                detected: found,
                version,
            });
        }
    }

    let _ = write_settings(root, &s);
    s.tools
}

fn probe_which(cmd: &str) -> bool {
    let which_cmd = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    std::process::Command::new(which_cmd)
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn probe_version(cmd: &str) -> Option<String> {
    let output = std::process::Command::new(cmd)
        .arg("--version")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let text = if stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    } else {
        stdout
    };
    text.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

// ── Tool CRUD ─────────────────────────────────────────────────────────────────

pub fn save_tool(root: &str, tool: CliTool) -> Result<(), String> {
    let mut s = read_settings(root);
    match s.tools.iter_mut().find(|t| t.id == tool.id) {
        Some(existing) => *existing = tool,
        None => s.tools.push(tool),
    }
    write_settings(root, &s)
}

pub fn delete_tool(root: &str, id: &str) -> Result<(), String> {
    let mut s = read_settings(root);
    s.tools.retain(|t| t.id != id);
    write_settings(root, &s)
}

/// Test a tool by running `<command> --version` with a short timeout.
pub async fn test_tool(root: &str, id: &str) -> Result<String, String> {
    let s = read_settings(root);
    let tool = s
        .tools
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| format!("Tool not found: {id}"))?
        .clone();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        tokio::process::Command::new(&tool.command)
            .arg("--version")
            .output(),
    )
    .await
    .map_err(|_| {
        format!(
            "'{}' timed out (not installed or not responding).",
            tool.command
        )
    })?
    .map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "'{}' not found. Please install it or specify the full path.",
                tool.command
            )
        } else {
            format!("Failed to run '{}': {}", tool.command, e)
        }
    })?;

    let stdout = String::from_utf8_lossy(&result.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).trim().to_string();
    let text = if !stdout.is_empty() { stdout } else { stderr };
    if text.is_empty() {
        Ok(format!(
            "{} is available (no version output).",
            tool.command
        ))
    } else {
        Ok(text.lines().next().unwrap_or("").to_string())
    }
}

// ── Prompt templates ──────────────────────────────────────────────────────────

pub fn get_prompt_templates(root: &str) -> Vec<CliPromptTemplate> {
    let templates = read_settings(root).prompt_templates;
    if templates.is_empty() {
        default_templates()
    } else {
        templates
    }
}

pub fn save_prompt_template(root: &str, tpl: CliPromptTemplate) -> Result<(), String> {
    let mut s = read_settings(root);
    if s.prompt_templates.is_empty() {
        s.prompt_templates = default_templates();
    }
    match s.prompt_templates.iter_mut().find(|t| t.id == tpl.id) {
        Some(existing) => *existing = tpl,
        None => s.prompt_templates.push(tpl),
    }
    write_settings(root, &s)
}

pub fn delete_prompt_template(root: &str, id: &str) -> Result<(), String> {
    let mut s = read_settings(root);
    s.prompt_templates.retain(|t| t.id != id);
    write_settings(root, &s)
}

fn default_templates() -> Vec<CliPromptTemplate> {
    vec![
        CliPromptTemplate {
            id: "builtin_methodology".to_string(),
            name: "方法论拆解".to_string(),
            prompt_template: "请阅读论文《{title}》，文件位于 {paper_dir}。\n\n请详细分析：方法/模型架构/核心算法，用中文输出结构化分析。".to_string(),
        },
        CliPromptTemplate {
            id: "builtin_reproduction".to_string(),
            name: "复现要点".to_string(),
            prompt_template: "请阅读论文《{title}》，文件位于 {paper_dir}。\n\n请提炼复现该工作所需的：数据集、模型配置、超参数设置、训练流程和潜在难点，用中文输出。".to_string(),
        },
        CliPromptTemplate {
            id: "builtin_related".to_string(),
            name: "相关工作梳理".to_string(),
            prompt_template: "请阅读论文《{title}》，文件位于 {paper_dir}。\n\n请梳理其相关工作脉络，说明本文与各相关工作的联系与区别，用中文输出。".to_string(),
        },
        CliPromptTemplate {
            id: "builtin_contribution".to_string(),
            name: "创新点与局限".to_string(),
            prompt_template: "请阅读论文《{title}》，文件位于 {paper_dir}。\n\n请提炼核心贡献（创新点）与不足（局限性），用中文输出。".to_string(),
        },
    ]
}

// ── Analysis result storage ───────────────────────────────────────────────────

pub fn save_analysis(root: &str, slug: &str, name: &str, content: &str) -> Result<String, String> {
    let dir = crate::paper::paper_dir(root, slug).join("cli_analysis");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let ts = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    // Build a filesystem-safe suffix from the name (ASCII alphanumeric + dash/underscore only).
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe = safe.trim_matches('_').to_string();
    let suffix = if safe.is_empty() {
        "analysis".to_string()
    } else {
        safe
    };
    let filename = format!("{}_{}.md", ts, suffix);
    let path = dir.join(&filename);

    let header = format!("# {}\n\n> 分析时间：{}\n\n---\n\n", name, ts);
    std::fs::write(&path, header + content).map_err(|e| e.to_string())?;

    // Mark cli_analyzed = true in .status.json
    let mut status = crate::paper::read_status_for(root, slug);
    status.cli_analyzed = true;
    status.last_updated = chrono::Utc::now().to_rfc3339();
    let _ = crate::paper::write_status(root, slug, &status);

    Ok(filename)
}

pub fn list_analyses(root: &str, slug: &str) -> Result<Vec<CliAnalysisEntry>, String> {
    let dir = crate::paper::paper_dir(root, slug).join("cli_analysis");
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut entries: Vec<CliAnalysisEntry> = std::fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("md"))
        .map(|e| {
            let filename = e.file_name().to_string_lossy().to_string();
            let abs_path = e.path().to_string_lossy().to_string();
            // Read display name from the first H1 line of the file.
            let name = std::fs::read_to_string(e.path())
                .ok()
                .and_then(|c| {
                    c.lines()
                        .next()
                        .and_then(|l| l.strip_prefix("# "))
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| filename.trim_end_matches(".md").to_string());
            // Date prefix is the first 10 chars (YYYY-MM-DD).
            let created_at = filename.get(..10).unwrap_or("").to_string();
            CliAnalysisEntry {
                filename,
                name,
                created_at,
                path: abs_path,
            }
        })
        .collect();

    entries.sort_by(|a, b| b.filename.cmp(&a.filename)); // newest first
    Ok(entries)
}

pub fn get_analysis(root: &str, slug: &str, filename: &str) -> Result<String, String> {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("Invalid filename".to_string());
    }
    let path = crate::paper::paper_dir(root, slug)
        .join("cli_analysis")
        .join(filename);
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}
