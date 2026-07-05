//! MOBI / AZW3 parser via the `mobi` crate — best effort. DRM'd books are
//! rejected outright; KF8 (AZW3) support follows whatever the crate can
//! decode. Images use the legacy `recindex` referencing scheme, mapped to the
//! book's image records in order.

use std::collections::HashMap;
use std::path::Path;

use super::{
    extract_body_html, first_heading_text, html_to_text, neutralize_links, EbookMeta,
    EbookTocEntry, ParsedEbook, ResourceData, ResourceEntry,
};

pub fn parse(path: &Path, format: &str) -> Result<ParsedEbook, String> {
    let m = mobi::Mobi::from_path(path)
        .map_err(|e| format!("Cannot parse {} file: {e}", format.to_uppercase()))?;

    if !matches!(m.encryption(), mobi::headers::Encryption::No) {
        return Err("This book is DRM-protected and cannot be imported".to_string());
    }

    let raw = m.content_as_string_lossy();
    if raw.trim().is_empty() {
        return Err(format!(
            "No readable text found in this {} file (unsupported compression or layout)",
            format.to_uppercase()
        ));
    }

    // Image records → resources keyed "rec1", "rec2", … (recindex is 1-based
    // in kindlegen output and images are laid out contiguously).
    let mut resources: HashMap<String, ResourceEntry> = HashMap::new();
    for (i, rec) in m.image_records().iter().enumerate() {
        if let Some(mime) = sniff_image_mime(rec.content) {
            resources.insert(
                format!("rec{}", i + 1),
                ResourceEntry {
                    mime: mime.to_string(),
                    data: ResourceData::Bytes(rec.content.to_vec()),
                },
            );
        }
    }

    let body = extract_body_html(&raw);
    let chunks = split_chapters(&body);

    let mut chapter_html = Vec::with_capacity(chunks.len());
    let mut chapter_titles = Vec::with_capacity(chunks.len());
    let mut toc = Vec::new();
    for chunk in chunks {
        let html = neutralize_links(&rewrite_recindex_images(&chunk, &resources));
        let title = first_heading_text(&html);
        if html_to_text(&html).trim().is_empty() && !html.contains("data-argus-res") {
            continue;
        }
        chapter_html.push(html);
        if let Some(t) = &title {
            toc.push(EbookTocEntry {
                title: t.clone(),
                level: 1,
                chapter: chapter_html.len() as u32,
                anchor: None,
            });
        }
        chapter_titles.push(title);
    }
    if chapter_html.is_empty() {
        return Err("Book content could not be split into readable chapters".to_string());
    }

    let title = {
        let t = m.title();
        if t.trim().is_empty() {
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled").to_string()
        } else {
            t
        }
    };
    let meta = EbookMeta {
        title,
        authors: m.author().into_iter().filter(|a| !a.trim().is_empty()).collect(),
        year: m.publish_date().as_deref().and_then(extract_year),
        language: None,
        publisher: m.publisher().filter(|p| !p.trim().is_empty()),
        identifier: m.isbn().filter(|i| !i.trim().is_empty()),
        description: m.description().filter(|d| !d.trim().is_empty()),
    };

    let chapter_text: Vec<String> = chapter_html.iter().map(|h| html_to_text(h)).collect();

    Ok(ParsedEbook {
        format: format.to_string(),
        book_path: path.to_path_buf(),
        meta,
        toc,
        chapter_html,
        chapter_titles,
        chapter_text,
        resources,
    })
}

/// Split rawml on `<mbp:pagebreak/>`; fall back to splitting before h1/h2
/// headings; else a single chapter.
fn split_chapters(html: &str) -> Vec<String> {
    use regex::Regex;
    static PB_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static H_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let pb_re = PB_RE
        .get_or_init(|| Regex::new(r"(?i)<mbp:pagebreak[^>]*>(\s*</mbp:pagebreak>)?").unwrap());
    let h_re = H_RE.get_or_init(|| Regex::new(r"(?i)<h[12][^>]*>").unwrap());

    let parts: Vec<String> = pb_re.split(html).map(|s| s.to_string()).collect();
    if parts.len() >= 2 {
        return parts;
    }

    let starts: Vec<usize> = h_re.find_iter(html).map(|mt| mt.start()).collect();
    if starts.len() >= 2 {
        let mut out = Vec::with_capacity(starts.len() + 1);
        if starts[0] > 0 {
            out.push(html[..starts[0]].to_string());
        }
        for (i, s) in starts.iter().enumerate() {
            let end = starts.get(i + 1).copied().unwrap_or(html.len());
            out.push(html[*s..end].to_string());
        }
        return out;
    }

    vec![html.to_string()]
}

/// `<img recindex="00005" …>` → `<img data-argus-res="rec5" …>`; images whose
/// record we could not decode are dropped.
fn rewrite_recindex_images(html: &str, resources: &HashMap<String, ResourceEntry>) -> String {
    use regex::Regex;
    static IMG_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static REC_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let img_re = IMG_RE.get_or_init(|| Regex::new(r"(?is)<img\b[^>]*>").unwrap());
    let rec_re = REC_RE
        .get_or_init(|| Regex::new(r#"(?i)\srecindex\s*=\s*["']?0*(\d+)["']?"#).unwrap());

    img_re
        .replace_all(html, |caps: &regex::Captures| {
            let tag = caps.get(0).map(|mt| mt.as_str()).unwrap_or("");
            if let Some(c) = rec_re.captures(tag) {
                let key = format!("rec{}", &c[1]);
                if resources.contains_key(&key) {
                    return rec_re
                        .replace(tag, format!(r#" data-argus-res="{key}""#).as_str())
                        .into_owned();
                }
            }
            String::new()
        })
        .into_owned()
}

fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(&[0x42, 0x4D]) {
        Some("image/bmp")
    } else {
        None
    }
}

fn extract_year(s: &str) -> Option<u32> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        if bytes[i..i + 4].iter().all(|b| b.is_ascii_digit()) {
            if let Ok(y) = s[i..i + 4].parse::<u32>() {
                if (1400..=2200).contains(&y) {
                    return Some(y);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_splitting() {
        let html = "<p>intro</p><mbp:pagebreak/><h1>One</h1><p>a</p><mbp:pagebreak/><h1>Two</h1>";
        let parts = split_chapters(html);
        assert_eq!(parts.len(), 3);

        let by_heading = split_chapters("<p>x</p><h1>A</h1><p>1</p><h1>B</h1><p>2</p>");
        assert_eq!(by_heading.len(), 3);

        assert_eq!(split_chapters("<p>only</p>").len(), 1);
    }

    #[test]
    fn recindex_rewrite() {
        let mut res = HashMap::new();
        res.insert(
            "rec5".to_string(),
            ResourceEntry { mime: "image/png".into(), data: ResourceData::Bytes(vec![]) },
        );
        let out = rewrite_recindex_images(r#"<img recindex="00005" width="10"><img recindex="7">"#, &res);
        assert!(out.contains(r#"data-argus-res="rec5""#));
        assert!(out.contains(r#"width="10""#));
        assert!(!out.contains("rec7"));
    }

    #[test]
    fn mime_sniffing() {
        assert_eq!(sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_image_mime(b"GIF89a"), Some("image/gif"));
        assert_eq!(sniff_image_mime(b"nope"), None);
    }
}
