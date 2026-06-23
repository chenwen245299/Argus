//! M10: Canvas enhancements — suggest edges, auto layout, export image.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use serde::Deserialize;

use crate::models::{Canvas, CanvasNode, ChatMessage, NodePosition, PaperMeta, SuggestedEdge};
use crate::{ai_manager, canvas, llm};

// ── Paper meta helpers ────────────────────────────────────────────────────────

/// Scan papers/ directory and load the PaperMeta whose `id` == paper_id.
fn load_paper_meta_by_id(root: &str, paper_id: &str) -> Option<PaperMeta> {
    let papers_dir = Path::new(root).join("papers");
    for entry in std::fs::read_dir(&papers_dir).ok()?.flatten() {
        let meta_path = entry.path().join("meta.json");
        if let Ok(data) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<PaperMeta>(&data) {
                if meta.id == paper_id {
                    return Some(meta);
                }
            }
        }
    }
    None
}

// ── Semantic Scholar types ────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct SsPaperDetail {
    references: Option<Vec<SsRef>>,
    citations: Option<Vec<SsRef>>,
}

#[derive(Deserialize)]
struct SsRef {
    #[serde(rename = "externalIds", default)]
    external_ids: Option<SsExternalIds>,
    title: Option<String>,
}

#[derive(Deserialize)]
struct SsExternalIds {
    #[serde(rename = "ArXiv")]
    arxiv: Option<String>,
    #[serde(rename = "DOI")]
    doi: Option<String>,
}

// ── suggest_canvas_edges ──────────────────────────────────────────────────────

pub async fn suggest_canvas_edges(
    root: &str,
    canvas_id: &str,
    source: &str,
    ai_provider_id: Option<String>,
    ai_model_id: Option<String>,
) -> Result<Vec<SuggestedEdge>, String> {
    let cv = canvas::get_canvas(root, canvas_id)?;
    if cv.nodes.len() < 2 {
        return Ok(vec![]);
    }

    // Load all paper metas for canvas nodes
    let metas: Vec<PaperMeta> = cv
        .nodes
        .iter()
        .filter_map(|n| load_paper_meta_by_id(root, &n.paper_id))
        .collect();

    if metas.len() < 2 {
        return Ok(vec![]);
    }

    // Build set of existing edges (from_paper_id, to_paper_id)
    let node_to_paper: HashMap<&str, &str> = cv
        .nodes
        .iter()
        .map(|n| (n.node_id.as_str(), n.paper_id.as_str()))
        .collect();
    let existing: HashSet<(String, String)> = cv
        .edges
        .iter()
        .filter_map(|e| {
            let from = node_to_paper.get(e.from_node_id.as_str())?.to_string();
            let to = node_to_paper.get(e.to_node_id.as_str())?.to_string();
            Some((from, to))
        })
        .collect();

    let mut all: Vec<SuggestedEdge> = Vec::new();

    if source == "citations" || source == "all" {
        all.extend(suggest_from_citations(&metas).await);
    }
    if source == "similarity" || source == "all" {
        all.extend(suggest_from_similarity(&metas));
    }
    if source == "ai" || source == "all" {
        match suggest_from_ai(
            root,
            &metas,
            ai_provider_id.as_deref(),
            ai_model_id.as_deref(),
        )
        .await
        {
            Ok(sug) => all.extend(sug),
            Err(e) if source == "ai" => return Err(e),
            _ => {} // "all" mode: skip AI on error
        }
    }

    // Remove self-loops and already existing edges
    all.retain(|s| {
        s.from_paper_id != s.to_paper_id
            && !existing.contains(&(s.from_paper_id.clone(), s.to_paper_id.clone()))
    });

    // Deduplicate (from, to): keep entry with highest score per pair
    let mut dedup: HashMap<(String, String), SuggestedEdge> = HashMap::new();
    for s in all {
        let key = (s.from_paper_id.clone(), s.to_paper_id.clone());
        let new_score = s.score.unwrap_or(0.0);
        let keep = match dedup.get(&key) {
            Some(prev) => new_score > prev.score.unwrap_or(0.0),
            None => true,
        };
        if keep {
            dedup.insert(key, s);
        }
    }

    let mut result: Vec<SuggestedEdge> = dedup.into_values().collect();
    result.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(result)
}

// ── Source A: Semantic Scholar citations ──────────────────────────────────────

async fn suggest_from_citations(metas: &[PaperMeta]) -> Vec<SuggestedEdge> {
    let client = match reqwest::Client::builder()
        .user_agent("Argus/1.0")
        .timeout(std::time::Duration::from_secs(12))
        .build()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    // Build lookup maps: identifier → paper_id
    let mut arxiv_map: HashMap<String, String> = HashMap::new(); // lowercase arxiv_id → paper_id
    let mut doi_map: HashMap<String, String> = HashMap::new(); // lowercase doi → paper_id
    let mut title_map: HashMap<String, String> = HashMap::new(); // normalized title → paper_id

    for meta in metas {
        if let Some(a) = &meta.arxiv_id {
            arxiv_map.insert(a.to_lowercase(), meta.id.clone());
        }
        if let Some(d) = &meta.doi {
            doi_map.insert(d.to_lowercase(), meta.id.clone());
        }
        title_map.insert(normalize_title(&meta.title), meta.id.clone());
    }

    let mut suggestions = Vec::new();

    for meta in metas {
        // Build Semantic Scholar query identifier
        let ss_id = if let Some(a) = &meta.arxiv_id {
            format!("ArXiv:{}", a)
        } else if let Some(d) = &meta.doi {
            format!("DOI:{}", d)
        } else {
            continue; // No identifier to query
        };

        let url = format!(
            "https://api.semanticscholar.org/graph/v1/paper/{}?fields=references,citations",
            ss_id
        );

        let detail: SsPaperDetail = match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => resp.json().await.unwrap_or_default(),
            _ => continue,
        };

        // References = papers this paper cites → they are upstream of meta.id
        if let Some(refs) = &detail.references {
            for r in refs {
                if let Some(ref_id) = match_ss_ref(r, &arxiv_map, &doi_map, &title_map) {
                    if ref_id != meta.id {
                        suggestions.push(SuggestedEdge {
                            from_paper_id: ref_id,
                            to_paper_id: meta.id.clone(),
                            source: "citations".to_string(),
                            reason: Some(format!("「{}」引用了该论文", meta.title)),
                            score: Some(1.0),
                        });
                    }
                }
            }
        }

        // Citations = papers that cite this paper → meta.id is upstream of them
        if let Some(cits) = &detail.citations {
            for c in cits {
                if let Some(cit_id) = match_ss_ref(c, &arxiv_map, &doi_map, &title_map) {
                    if cit_id != meta.id {
                        suggestions.push(SuggestedEdge {
                            from_paper_id: meta.id.clone(),
                            to_paper_id: cit_id,
                            source: "citations".to_string(),
                            reason: Some(format!("「{}」被下游论文引用", meta.title)),
                            score: Some(1.0),
                        });
                    }
                }
            }
        }

        // Polite delay between API calls
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    suggestions
}

fn match_ss_ref(
    r: &SsRef,
    arxiv_map: &HashMap<String, String>,
    doi_map: &HashMap<String, String>,
    title_map: &HashMap<String, String>,
) -> Option<String> {
    if let Some(ext) = &r.external_ids {
        if let Some(a) = &ext.arxiv {
            if let Some(id) = arxiv_map.get(&a.to_lowercase()) {
                return Some(id.clone());
            }
        }
        if let Some(d) = &ext.doi {
            if let Some(id) = doi_map.get(&d.to_lowercase()) {
                return Some(id.clone());
            }
        }
    }
    if let Some(t) = &r.title {
        if let Some(id) = title_map.get(&normalize_title(t)) {
            return Some(id.clone());
        }
    }
    None
}

fn normalize_title(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Source B: TF-IDF similarity + year ordering ───────────────────────────────

fn suggest_from_similarity(metas: &[PaperMeta]) -> Vec<SuggestedEdge> {
    const THRESHOLD: f32 = 0.18;

    // Build text corpus: title + authors + venue
    let texts: Vec<(String, i64)> = metas
        .iter()
        .map(|m| {
            let text = format!(
                "{} {} {}",
                m.title,
                m.authors.join(" "),
                m.venue.as_deref().unwrap_or("")
            );
            let year = m.year.map(|y| y as i64).unwrap_or(0);
            (text, year)
        })
        .collect();

    // Compute TF for each document
    let tf_list: Vec<HashMap<String, f32>> =
        texts.iter().map(|(text, _)| compute_tf(text)).collect();

    // Compute IDF
    let n = tf_list.len() as f32;
    let mut df: HashMap<String, f32> = HashMap::new();
    for tf in &tf_list {
        for term in tf.keys() {
            *df.entry(term.clone()).or_insert(0.0) += 1.0;
        }
    }
    let idf: HashMap<String, f32> = df
        .into_iter()
        .map(|(k, v)| (k, (n / v).ln() + 1.0))
        .collect();

    // Compute TF-IDF vectors
    let tfidf: Vec<HashMap<String, f32>> = tf_list
        .iter()
        .map(|tf| {
            tf.iter()
                .map(|(k, v)| (k.clone(), v * idf.get(k).copied().unwrap_or(1.0)))
                .collect()
        })
        .collect();

    let mut suggestions = Vec::new();

    for i in 0..metas.len() {
        for j in (i + 1)..metas.len() {
            let sim = cosine_similarity(&tfidf[i], &tfidf[j]);
            if sim < THRESHOLD {
                continue;
            }

            let year_i = texts[i].1;
            let year_j = texts[j].1;

            let (upstream_idx, downstream_idx) = if year_i <= year_j { (i, j) } else { (j, i) };

            suggestions.push(SuggestedEdge {
                from_paper_id: metas[upstream_idx].id.clone(),
                to_paper_id: metas[downstream_idx].id.clone(),
                source: "similarity".to_string(),
                reason: Some(format!(
                    "语义相似度 {:.0}%，按发表时间推断引用关系",
                    sim * 100.0
                )),
                score: Some(sim),
            });
        }
    }

    suggestions.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    suggestions
}

fn compute_tf(text: &str) -> HashMap<String, f32> {
    const STOPWORDS: &[&str] = &[
        "the", "a", "an", "and", "or", "of", "in", "is", "are", "for", "to", "with", "on", "by",
        "from", "that", "this", "it", "as", "at", "be", "was", "were", "have", "has", "but", "not",
        "we",
    ];
    let words: Vec<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2 && !STOPWORDS.contains(w))
        .map(|w| w.to_string())
        .collect();
    let total = words.len() as f32;
    if total == 0.0 {
        return HashMap::new();
    }
    let mut counts: HashMap<String, f32> = HashMap::new();
    for word in words {
        *counts.entry(word).or_insert(0.0) += 1.0 / total;
    }
    counts
}

fn cosine_similarity(a: &HashMap<String, f32>, b: &HashMap<String, f32>) -> f32 {
    let dot: f32 = a
        .iter()
        .filter_map(|(k, v)| b.get(k).map(|bv| v * bv))
        .sum();
    let norm_a: f32 = a.values().map(|v| v * v).sum::<f32>().sqrt();
    let norm_b: f32 = b.values().map(|v| v * v).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

// ── Source C: AI inference ────────────────────────────────────────────────────

async fn suggest_from_ai(
    root: &str,
    metas: &[PaperMeta],
    provider_id: Option<&str>,
    model_id: Option<&str>,
) -> Result<Vec<SuggestedEdge>, String> {
    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, provider_id, model_id)?;

    let papers_text = metas
        .iter()
        .map(|m| {
            format!(
                "- ID: {}\n  Title: {}\n  Year: {}\n  Authors: {}\n  Venue: {}",
                m.id,
                m.title,
                m.year
                    .map(|y| y.to_string())
                    .as_deref()
                    .unwrap_or("unknown"),
                m.authors.join(", "),
                m.venue.as_deref().unwrap_or("unknown"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        r#"Analyze these academic papers and identify likely citation/influence relationships between them. A directed edge from Paper A to Paper B means A is upstream (older/foundational) and B builds on A.

Papers:
{}

Return ONLY a valid JSON array. Each element must have exactly these keys:
- "from_paper_id": upstream paper's ID string
- "to_paper_id": downstream paper's ID string
- "reason": one sentence in Chinese explaining the relationship
- "score": confidence float 0.0-1.0

Only include high-confidence relationships. Return [] if unsure. No markdown, no explanation, just the JSON array."#,
        papers_text
    );

    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt.into(),
    }];

    let response = llm::chat_completion(&provider, &api_key, &model, &messages, "canvas").await?;

    // Extract JSON array from response
    let json_start = response.find('[').unwrap_or(0);
    let json_end = response.rfind(']').map(|i| i + 1).unwrap_or(json_start);
    let json_str = if json_start < json_end {
        &response[json_start..json_end]
    } else {
        "[]"
    };

    #[derive(Deserialize)]
    struct AiEdge {
        from_paper_id: String,
        to_paper_id: String,
        reason: Option<String>,
        score: Option<f32>,
    }

    let ai_edges: Vec<AiEdge> = serde_json::from_str(json_str).unwrap_or_default();

    // Validate paper IDs exist in our meta list
    let valid_ids: HashSet<&str> = metas.iter().map(|m| m.id.as_str()).collect();

    Ok(ai_edges
        .into_iter()
        .filter(|e| {
            valid_ids.contains(e.from_paper_id.as_str())
                && valid_ids.contains(e.to_paper_id.as_str())
        })
        .map(|e| SuggestedEdge {
            from_paper_id: e.from_paper_id,
            to_paper_id: e.to_paper_id,
            source: "ai".to_string(),
            reason: e.reason,
            score: e.score,
        })
        .collect())
}

// ── compute_canvas_layout ─────────────────────────────────────────────────────

pub fn compute_canvas_layout(
    root: &str,
    canvas_id: &str,
    layout: &str,
    direction: &str,
) -> Result<Vec<NodePosition>, String> {
    let cv = canvas::get_canvas(root, canvas_id)?;
    if cv.nodes.is_empty() {
        return Ok(vec![]);
    }

    match layout {
        "timeline" => timeline_layout(&cv, direction, root),
        "topological" => topological_layout(&cv, direction, root),
        _ => Err(format!("Unknown layout: {layout}")),
    }
}

// Timeline: sort nodes by publication year, place in columns (or rows for vertical)
fn timeline_layout(cv: &Canvas, direction: &str, root: &str) -> Result<Vec<NodePosition>, String> {
    let mut nodes_years: Vec<(&CanvasNode, i64)> = cv
        .nodes
        .iter()
        .map(|n| {
            let year = load_paper_year(root, &n.paper_id);
            (n, year)
        })
        .collect();

    nodes_years.sort_by_key(|(_, y)| *y);

    // Group by year
    let mut year_groups: Vec<(i64, Vec<&CanvasNode>)> = Vec::new();
    for (node, year) in &nodes_years {
        match year_groups.last_mut() {
            Some(last) if last.0 == *year => last.1.push(node),
            _ => year_groups.push((*year, vec![node])),
        }
    }

    let col_gap = 280.0f64;
    let row_gap = 180.0f64;
    let start_x = 60.0;
    let start_y = 60.0;

    let mut positions = Vec::new();
    for (col_idx, (_, nodes)) in year_groups.iter().enumerate() {
        for (row_idx, node) in nodes.iter().enumerate() {
            let (x, y) = if direction == "vertical" {
                (
                    start_x + row_idx as f64 * col_gap,
                    start_y + col_idx as f64 * row_gap,
                )
            } else {
                (
                    start_x + col_idx as f64 * col_gap,
                    start_y + row_idx as f64 * row_gap,
                )
            };
            positions.push(NodePosition {
                node_id: node.node_id.clone(),
                x,
                y,
            });
        }
    }
    Ok(positions)
}

// Topological: Kahn's BFS layering based on directed edges
fn topological_layout(
    cv: &Canvas,
    direction: &str,
    root: &str,
) -> Result<Vec<NodePosition>, String> {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut in_degree: HashMap<&str, i32> = HashMap::new();

    for node in &cv.nodes {
        adj.entry(node.node_id.as_str()).or_default();
        in_degree.entry(node.node_id.as_str()).or_insert(0);
    }
    for edge in &cv.edges {
        adj.entry(edge.from_node_id.as_str())
            .or_default()
            .push(edge.to_node_id.as_str());
        *in_degree.entry(edge.to_node_id.as_str()).or_insert(0) += 1;
    }

    // Kahn's algorithm with year-based tie-breaking
    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(&id, _)| id)
        .collect();

    let mut layers: Vec<Vec<&str>> = Vec::new();
    let mut visited: HashSet<&str> = HashSet::new();

    while !queue.is_empty() {
        let layer: Vec<&str> = queue.drain(..).collect();
        for &id in &layer {
            visited.insert(id);
            if let Some(children) = adj.get(id) {
                for &child in children {
                    if !visited.contains(child) {
                        let d = in_degree.entry(child).or_insert(0);
                        *d -= 1;
                        if *d <= 0 {
                            queue.push_back(child);
                        }
                    }
                }
            }
        }
        layers.push(layer);
    }

    // Remaining nodes (in cycles): add as final layer
    let remaining: Vec<&str> = cv
        .nodes
        .iter()
        .filter(|n| !visited.contains(n.node_id.as_str()))
        .map(|n| n.node_id.as_str())
        .collect();
    if !remaining.is_empty() {
        layers.push(remaining);
    }

    // Sort each layer by year for consistent layout
    let node_year: HashMap<&str, i64> = cv
        .nodes
        .iter()
        .map(|n| (n.node_id.as_str(), load_paper_year(root, &n.paper_id)))
        .collect();

    for layer in &mut layers {
        layer.sort_by_key(|id| node_year.get(id).copied().unwrap_or(0));
    }

    let col_gap = 280.0f64;
    let row_gap = 180.0f64;
    let start_x = 60.0;
    let start_y = 60.0;

    let mut positions = Vec::new();
    for (col_idx, layer) in layers.iter().enumerate() {
        // Center nodes vertically within each column
        let layer_count = layer.len();
        let total_height = (layer_count.saturating_sub(1)) as f64 * row_gap;
        let offset = total_height / 2.0;

        for (row_idx, &node_id) in layer.iter().enumerate() {
            let (x, y) = if direction == "vertical" {
                (
                    start_x + row_idx as f64 * col_gap,
                    start_y + col_idx as f64 * row_gap,
                )
            } else {
                (
                    start_x + col_idx as f64 * col_gap,
                    start_y + row_idx as f64 * row_gap - offset,
                )
            };
            positions.push(NodePosition {
                node_id: node_id.to_string(),
                x,
                y,
            });
        }
    }
    Ok(positions)
}

fn load_paper_year(root: &str, paper_id: &str) -> i64 {
    if paper_id.is_empty() {
        return 0;
    }
    let papers_dir = Path::new(root).join("papers");
    if let Ok(entries) = std::fs::read_dir(&papers_dir) {
        for entry in entries.flatten() {
            let meta_path = entry.path().join("meta.json");
            if let Ok(data) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<PaperMeta>(&data) {
                    if meta.id == paper_id {
                        return meta.year.map(|y| y as i64).unwrap_or(0);
                    }
                }
            }
        }
    }
    0
}

// ── export_canvas_image ───────────────────────────────────────────────────────

pub fn export_canvas_image(
    format: &str,
    image_data: &str,
    save_path: &str,
) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    // Create parent directory if needed
    if let Some(parent) = Path::new(save_path).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Cannot create directory: {e}"))?;
        }
    }

    match format {
        "png" => {
            // Strip data URL prefix if present: "data:image/png;base64,..."
            let b64 = if let Some(pos) = image_data.find(',') {
                &image_data[pos + 1..]
            } else {
                image_data
            };
            let bytes = STANDARD
                .decode(b64)
                .map_err(|e| format!("Invalid base64: {e}"))?;
            std::fs::write(save_path, bytes).map_err(|e| format!("Write PNG failed: {e}"))?;
        }
        "svg" => {
            std::fs::write(save_path, image_data.as_bytes())
                .map_err(|e| format!("Write SVG failed: {e}"))?;
        }
        _ => return Err(format!("Unknown format: {format}")),
    }

    Ok(save_path.to_string())
}
