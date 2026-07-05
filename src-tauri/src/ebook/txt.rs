//! Plain-text book parser: encoding detection (UTF-8/UTF-16/GB18030) +
//! heading-based chapter splitting.

use std::path::Path;

use super::{EbookMeta, EbookTocEntry, ParsedEbook};

pub fn parse(path: &Path) -> Result<ParsedEbook, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Read TXT: {e}"))?;
    let text = decode_text(&bytes);
    if text.trim().is_empty() {
        return Err("TXT file is empty".to_string());
    }

    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();

    let chapters = split_chapters(&text);
    let mut chapter_html = Vec::with_capacity(chapters.len());
    let mut chapter_titles = Vec::with_capacity(chapters.len());
    let mut chapter_text = Vec::with_capacity(chapters.len());
    let mut toc = Vec::new();
    for (i, (ch_title, body)) in chapters.into_iter().enumerate() {
        chapter_html.push(text_to_html(&body));
        chapter_text.push(body);
        if let Some(t) = &ch_title {
            toc.push(EbookTocEntry {
                title: t.clone(),
                level: 1,
                chapter: (i + 1) as u32,
                anchor: None,
            });
        }
        chapter_titles.push(ch_title);
    }

    Ok(ParsedEbook {
        format: "txt".to_string(),
        book_path: path.to_path_buf(),
        meta: EbookMeta { title, ..Default::default() },
        toc,
        chapter_html,
        chapter_titles,
        chapter_text,
        resources: Default::default(),
    })
}

/// UTF-8 (BOM or strict) → UTF-16 (BOM) → GB18030. Chinese TXT novels are
/// overwhelmingly GBK/GB18030 when they are not UTF-8.
fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode(&bytes[2..]).0.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode(&bytes[2..]).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

/// Split on chapter-heading lines. Returns `[(title?, body)]`; a single
/// title-less chapter when fewer than 2 headings are found. The heading line
/// itself stays at the top of its chapter body (readers expect to see it).
fn split_chapters(text: &str) -> Vec<(Option<String>, String)> {
    use regex::Regex;
    static HEAD_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let head_re = HEAD_RE.get_or_init(|| {
        Regex::new(
            r"(?x)^\s*(
              第\s*[〇零一二三四五六七八九十百千万0-9０-９]+\s*[章节卷回部集篇][^\n]{0,50}
            | (?i:chapter)\s+[0-9IVXLC]+[^\n]{0,50}
            | \#{1,3}\s+\S[^\n]{0,80}
            )\s*$",
        )
        .unwrap()
    });

    const MAX_CHAPTERS: usize = 3000;
    let lines: Vec<&str> = text.lines().collect();
    let mut marks: Vec<(usize, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if marks.len() >= MAX_CHAPTERS {
            break;
        }
        if let Some(c) = head_re.captures(line) {
            let title = c[1].trim().trim_start_matches('#').trim().to_string();
            marks.push((i, title));
        }
    }
    if marks.len() < 2 {
        return vec![(None, text.to_string())];
    }

    let mut out: Vec<(Option<String>, String)> = Vec::with_capacity(marks.len() + 1);
    if marks[0].0 > 0 {
        let head = lines[..marks[0].0].join("\n");
        if !head.trim().is_empty() {
            out.push((None, head));
        }
    }
    for (mi, (start, title)) in marks.iter().enumerate() {
        let end = marks.get(mi + 1).map(|(s, _)| *s).unwrap_or(lines.len());
        out.push((Some(title.clone()), lines[*start..end].join("\n")));
    }
    out
}

/// Every non-empty line becomes a paragraph (the norm for CJK novels).
fn text_to_html(text: &str) -> String {
    let mut html = String::with_capacity(text.len() + text.len() / 8);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        html.push_str("<p>");
        for ch in line.chars() {
            match ch {
                '&' => html.push_str("&amp;"),
                '<' => html.push_str("&lt;"),
                '>' => html.push_str("&gt;"),
                c => html.push(c),
            }
        }
        html.push_str("</p>\n");
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_chinese_chapters() {
        let text = "书名简介\n\n第一章 起点\n正文一。\n\n第二章 转折\n正文二。\n";
        let ch = split_chapters(text);
        assert_eq!(ch.len(), 3);
        assert_eq!(ch[0].0, None);
        assert_eq!(ch[1].0.as_deref(), Some("第一章 起点"));
        assert!(ch[1].1.contains("正文一。"));
        assert_eq!(ch[2].0.as_deref(), Some("第二章 转折"));
    }

    #[test]
    fn single_chapter_when_no_headings() {
        let ch = split_chapters("just some text\nwith lines\n");
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].0, None);
    }

    #[test]
    fn gb18030_fallback() {
        let (encoded, _, _) = encoding_rs::GB18030.encode("第一章 中文测试");
        let decoded = decode_text(&encoded);
        assert_eq!(decoded, "第一章 中文测试");
    }

    #[test]
    fn html_escapes() {
        assert_eq!(text_to_html("a<b>&c"), "<p>a&lt;b&gt;&amp;c</p>\n");
    }
}
