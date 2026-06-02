use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::Emitter;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::models::CliTool;

// ── Cancellation token ────────────────────────────────────────────────────────

#[derive(Clone)]
struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    fn new() -> Self {
        CancelToken(Arc::new(AtomicBool::new(false)))
    }
    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

static RUNS: OnceLock<Mutex<HashMap<String, CancelToken>>> = OnceLock::new();

fn runs() -> &'static Mutex<HashMap<String, CancelToken>> {
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Launch a CLI tool analysis in the background. Returns a `run_id` immediately.
/// Output is streamed via Tauri event `cli-analysis-<run_id>`.
pub async fn run(
    root: String,
    slug: String,
    tool: CliTool,
    prompt: String,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let cancel = CancelToken::new();
    runs()
        .lock()
        .unwrap()
        .insert(run_id.clone(), cancel.clone());

    let paper_dir = crate::paper::paper_dir(&root, &slug);
    let pdf_path = paper_dir.join("paper.pdf");
    let fulltext_path = paper_dir.join("fulltext.txt");
    let title = crate::paper::read_meta(&root, &slug)
        .map(|m| m.title)
        .unwrap_or_default();

    // Substitute placeholders in every arg.
    let args: Vec<String> = tool
        .args_template
        .iter()
        .map(|arg| {
            fill_placeholders(
                arg,
                &prompt,
                &paper_dir.to_string_lossy(),
                &pdf_path.to_string_lossy(),
                &fulltext_path.to_string_lossy(),
                &title,
            )
        })
        .collect();

    let event_name = format!("cli-analysis-{}", run_id);
    let run_id_bg = run_id.clone();
    let cancel_bg = cancel.clone();

    tokio::spawn(async move {
        let result = execute(
            tool.command.clone(),
            args,
            paper_dir,
            cancel_bg.clone(),
            event_name.clone(),
            app.clone(),
        )
        .await;

        runs().lock().unwrap().remove(&run_id_bg);

        let (exit_code, error_text, cancelled) = match &result {
            Ok(code) => (*code, String::new(), false),
            Err(e) => (-1i32, e.clone(), e == "cancelled"),
        };

        let source = if result.is_err() && !cancelled {
            "error"
        } else {
            "stdout"
        };
        let _ = app.emit(
            &event_name,
            serde_json::json!({
                "chunk":     error_text,
                "source":    source,
                "done":      true,
                "exit_code": exit_code,
                "cancelled": cancelled,
            }),
        );
    });

    Ok(run_id)
}

/// Cancel a running analysis by `run_id`.
pub fn cancel(run_id: &str) -> Result<(), String> {
    let guard = runs().lock().unwrap();
    match guard.get(run_id) {
        Some(token) => {
            token.cancel();
            Ok(())
        }
        None => Err(format!("Run not found or already finished: {run_id}")),
    }
}

// ── Execution ─────────────────────────────────────────────────────────────────

async fn execute(
    command: String,
    args: Vec<String>,
    paper_dir: PathBuf,
    cancel: CancelToken,
    event_name: String,
    app: tauri::AppHandle,
) -> Result<i32, String> {
    let mut child = tokio::process::Command::new(&command)
        .args(&args)
        .current_dir(&paper_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(&command, e))?;

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // Both reader tasks send to a single channel so output is reasonably interleaved.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(String, bool)>();
    let tx2 = tx.clone();

    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            if tx.send((l + "\n", false)).is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            if tx2.send((l + "\n", true)).is_err() {
                break;
            }
        }
    });

    let timeout_dur = std::time::Duration::from_secs(600); // 10-min hard limit
    let start = std::time::Instant::now();

    loop {
        // Check cancellation and timeout before blocking on receive.
        if cancel.is_cancelled() {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err("cancelled".to_string());
        }
        if start.elapsed() >= timeout_dur {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err("Analysis timed out (10-minute limit reached).".to_string());
        }

        match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            Ok(Some((text, is_stderr))) => {
                let _ = app.emit(
                    &event_name,
                    serde_json::json!({
                        "chunk":     text,
                        "source":    if is_stderr { "stderr" } else { "stdout" },
                        "done":      false,
                        "exit_code": serde_json::Value::Null,
                        "cancelled": false,
                    }),
                );
            }
            Ok(None) => break, // both pipes closed — process is done
            Err(_) => {}       // 200ms poll tick, loop to re-check cancel/timeout
        }
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    Ok(status.code().unwrap_or(-1))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn spawn_error(command: &str, e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        format!(
            "'{}' not found. Please install it and ensure it is in PATH, \
             or specify the full path in CLI tool settings.",
            command
        )
    } else {
        format!("Failed to start '{}': {}", command, e)
    }
}

fn fill_placeholders(
    template: &str,
    prompt: &str,
    paper_dir: &str,
    pdf_path: &str,
    fulltext_path: &str,
    title: &str,
) -> String {
    template
        .replace("{prompt}", prompt)
        .replace("{paper_dir}", paper_dir)
        .replace("{pdf_path}", pdf_path)
        .replace("{fulltext_path}", fulltext_path)
        .replace("{title}", title)
}
