//! FictionBook 2 (.fb2 / .fb2.zip) parser. FB2 is a single strict-XML file:
//! `<description>` (metadata), one or more `<body>` elements holding nested
//! `<section>`s, and base64 `<binary>` blobs for images.

use std::collections::HashMap;
use std::path::Path;

use super::{
    html_to_text, EbookMeta, EbookTocEntry, ParsedEbook, ResourceData, ResourceEntry,
};

pub fn parse(path: &Path) -> Result<ParsedEbook, String> {
    let xml = load_fb2_xml(path)?;
    let doc = roxmltree::Document::parse(&xml).map_err(|e| format!("FB2 XML parse: {e}"))?;
    let root = doc.root_element();

    let meta = parse_description(&root, path);

    // Binary resources (images), keyed by their id.
    let mut resources: HashMap<String, ResourceEntry> = HashMap::new();
    for bin in root.children().filter(|n| is_el(n, "binary")) {
        let Some(id) = bin.attribute("id") else { continue };
        let mime = bin.attribute("content-type").unwrap_or("image/jpeg").to_string();
        let b64: String = bin.text().unwrap_or("").split_whitespace().collect();
        use base64::Engine;
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
            resources.insert(
                id.to_string(),
                ResourceEntry { mime, data: ResourceData::Bytes(bytes) },
            );
        }
    }

    // First body = the book; extra bodies (notes/comments) are appended as
    // trailing chapters so footnote content is not silently lost.
    let bodies: Vec<roxmltree::Node> = root.children().filter(|n| is_el(n, "body")).collect();
    if bodies.is_empty() {
        return Err("FB2 has no <body>".to_string());
    }

    let mut chapter_html: Vec<String> = Vec::new();
    let mut chapter_titles: Vec<Option<String>> = Vec::new();
    let mut toc: Vec<EbookTocEntry> = Vec::new();

    for (bi, body) in bodies.iter().enumerate() {
        let sections: Vec<roxmltree::Node> =
            body.children().filter(|n| is_el(n, "section")).collect();
        if sections.is_empty() {
            // Body without sections — treat the whole body as one chapter.
            let html = render_children(*body, &resources);
            if !html.trim().is_empty() {
                chapter_html.push(html);
                chapter_titles.push(body_title(body, bi));
            }
            continue;
        }
        for sec in sections {
            let title = section_title(&sec);
            let html = render_children(sec, &resources);
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
    }
    if chapter_html.is_empty() {
        return Err("FB2 contains no readable content".to_string());
    }

    let chapter_text: Vec<String> = chapter_html.iter().map(|h| html_to_text(h)).collect();

    Ok(ParsedEbook {
        format: "fb2".to_string(),
        book_path: path.to_path_buf(),
        meta,
        toc,
        chapter_html,
        chapter_titles,
        chapter_text,
        resources,
    })
}

fn load_fb2_xml(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = if name.ends_with(".zip") {
        let file = std::fs::File::open(path).map_err(|e| format!("Open FB2 zip: {e}"))?;
        let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
            .map_err(|e| format!("FB2 zip: {e}"))?;
        let inner = archive
            .file_names()
            .find(|n| n.to_ascii_lowercase().ends_with(".fb2"))
            .map(|s| s.to_string())
            .ok_or_else(|| "No .fb2 entry inside the zip".to_string())?;
        let mut zf = archive.by_name(&inner).map_err(|e| e.to_string())?;
        super::read_capped(&mut zf)?
    } else {
        std::fs::read(path).map_err(|e| format!("Read FB2: {e}"))?
    };
    // FB2 default encoding is declared in the XML prolog; the common non-UTF-8
    // case is windows-1251 (Russian).
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_lowercase();
    let text = if head.contains("windows-1251") || head.contains("cp1251") {
        encoding_rs::WINDOWS_1251.decode(&bytes).0.into_owned()
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    // roxmltree rejects encoding declarations it did not decode itself.
    Ok(strip_encoding_decl(&text))
}

fn strip_encoding_decl(xml: &str) -> String {
    use regex::Regex;
    static DECL_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let decl_re =
        DECL_RE.get_or_init(|| Regex::new(r#"(?i)\s*encoding\s*=\s*("[^"]*"|'[^']*')"#).unwrap());
    match xml.find("?>") {
        Some(end) => {
            let (prolog, rest) = xml.split_at(end);
            format!("{}{}", decl_re.replace(prolog, ""), rest)
        }
        None => xml.to_string(),
    }
}

fn is_el(n: &roxmltree::Node, name: &str) -> bool {
    n.is_element() && n.tag_name().name() == name
}

fn parse_description(root: &roxmltree::Node, path: &Path) -> EbookMeta {
    let mut meta = EbookMeta::default();
    let desc = root.children().find(|n| is_el(n, "description"));
    if let Some(desc) = desc {
        if let Some(ti) = desc.children().find(|n| is_el(n, "title-info")) {
            meta.title = child_text(&ti, "book-title").unwrap_or_default();
            meta.language = child_text(&ti, "lang");
            for a in ti.children().filter(|n| is_el(n, "author")) {
                let name = ["first-name", "middle-name", "last-name"]
                    .iter()
                    .filter_map(|f| child_text(&a, f))
                    .collect::<Vec<_>>()
                    .join(" ");
                let name = if name.trim().is_empty() {
                    child_text(&a, "nickname").unwrap_or_default()
                } else {
                    name
                };
                if !name.trim().is_empty() {
                    meta.authors.push(name.trim().to_string());
                }
            }
            if let Some(ann) = ti.children().find(|n| is_el(n, "annotation")) {
                let text = collect_text(&ann);
                if !text.is_empty() {
                    meta.description = Some(text);
                }
            }
            if let Some(d) = child_text(&ti, "date") {
                meta.year = d
                    .chars()
                    .collect::<String>()
                    .split(|c: char| !c.is_ascii_digit())
                    .find(|s| s.len() == 4)
                    .and_then(|s| s.parse().ok());
            }
        }
        if let Some(pi) = desc.children().find(|n| is_el(n, "publish-info")) {
            meta.publisher = child_text(&pi, "publisher");
            if meta.year.is_none() {
                meta.year = child_text(&pi, "year").and_then(|y| y.trim().parse().ok());
            }
        }
    }
    if meta.title.trim().is_empty() {
        meta.title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .trim_end_matches(".fb2")
            .to_string();
    }
    meta
}

fn child_text(n: &roxmltree::Node, name: &str) -> Option<String> {
    n.children()
        .find(|c| is_el(c, name))
        .map(|c| collect_text(&c))
        .filter(|s| !s.is_empty())
}

fn collect_text(n: &roxmltree::Node) -> String {
    let mut s = String::new();
    for d in n.descendants().filter(|d| d.is_text()) {
        if let Some(t) = d.text() {
            s.push_str(t);
            s.push(' ');
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn section_title(sec: &roxmltree::Node) -> Option<String> {
    sec.children()
        .find(|n| is_el(n, "title"))
        .map(|t| collect_text(&t))
        .filter(|s| !s.is_empty())
}

fn body_title(body: &roxmltree::Node, index: usize) -> Option<String> {
    body.attribute("name")
        .map(|s| s.to_string())
        .or_else(|| {
            body.children()
                .find(|n| is_el(n, "title"))
                .map(|t| collect_text(&t))
        })
        .filter(|s| !s.is_empty())
        .or(if index > 0 { Some("Notes".to_string()) } else { None })
}

/// Render an FB2 node's children as simple HTML.
fn render_children(node: roxmltree::Node, resources: &HashMap<String, ResourceEntry>) -> String {
    let mut html = String::new();
    for child in node.children() {
        render_node(&child, resources, &mut html, 2);
    }
    html
}

fn render_node(
    n: &roxmltree::Node,
    resources: &HashMap<String, ResourceEntry>,
    html: &mut String,
    heading_level: u8,
) {
    if n.is_text() {
        if let Some(t) = n.text() {
            push_escaped(html, t);
        }
        return;
    }
    if !n.is_element() {
        return;
    }
    match n.tag_name().name() {
        "title" => {
            let lvl = heading_level.clamp(2, 4);
            html.push_str(&format!("<h{lvl}>"));
            push_escaped(html, &collect_text(n));
            html.push_str(&format!("</h{lvl}>\n"));
        }
        "subtitle" => {
            html.push_str("<h4>");
            push_escaped(html, &collect_text(n));
            html.push_str("</h4>\n");
        }
        "p" | "v" | "text-author" => {
            html.push_str("<p>");
            for c in n.children() {
                render_inline(&c, resources, html);
            }
            html.push_str("</p>\n");
        }
        "empty-line" => html.push_str("<br>\n"),
        "image" => render_image(n, resources, html),
        "epigraph" | "cite" | "annotation" => {
            html.push_str("<blockquote>\n");
            for c in n.children() {
                render_node(&c, resources, html, heading_level + 1);
            }
            html.push_str("</blockquote>\n");
        }
        "poem" | "stanza" => {
            for c in n.children() {
                render_node(&c, resources, html, heading_level + 1);
            }
        }
        "section" => {
            // Nested section inside a chapter: keep content, deepen headings.
            for c in n.children() {
                render_node(&c, resources, html, heading_level + 1);
            }
        }
        "table" => {
            // Rare in FB2; flatten rows to paragraphs.
            for tr in n.descendants().filter(|d| is_el(d, "tr")) {
                html.push_str("<p>");
                push_escaped(html, &collect_text(&tr));
                html.push_str("</p>\n");
            }
        }
        _ => {
            for c in n.children() {
                render_node(&c, resources, html, heading_level);
            }
        }
    }
}

fn render_inline(
    n: &roxmltree::Node,
    resources: &HashMap<String, ResourceEntry>,
    html: &mut String,
) {
    if n.is_text() {
        if let Some(t) = n.text() {
            push_escaped(html, t);
        }
        return;
    }
    if !n.is_element() {
        return;
    }
    match n.tag_name().name() {
        "emphasis" => {
            html.push_str("<em>");
            for c in n.children() {
                render_inline(&c, resources, html);
            }
            html.push_str("</em>");
        }
        "strong" => {
            html.push_str("<strong>");
            for c in n.children() {
                render_inline(&c, resources, html);
            }
            html.push_str("</strong>");
        }
        "strikethrough" => {
            html.push_str("<s>");
            for c in n.children() {
                render_inline(&c, resources, html);
            }
            html.push_str("</s>");
        }
        "sub" | "sup" | "code" => {
            let tag = n.tag_name().name();
            html.push_str(&format!("<{tag}>"));
            for c in n.children() {
                render_inline(&c, resources, html);
            }
            html.push_str(&format!("</{tag}>"));
        }
        "image" => render_image(n, resources, html),
        "a" => {
            // Links (usually footnotes) — keep the text, drop the target.
            for c in n.children() {
                render_inline(&c, resources, html);
            }
        }
        _ => {
            for c in n.children() {
                render_inline(&c, resources, html);
            }
        }
    }
}

fn render_image(
    n: &roxmltree::Node,
    resources: &HashMap<String, ResourceEntry>,
    html: &mut String,
) {
    // l:href="#id" (any namespace prefix)
    let href = n
        .attributes()
        .find(|a| a.name() == "href")
        .map(|a| a.value())
        .unwrap_or("");
    let id = href.trim_start_matches('#');
    if !id.is_empty() && resources.contains_key(id) {
        html.push_str(&format!(r#"<img data-argus-res="{id}" alt="">"#));
        html.push('\n');
    }
}

fn push_escaped(html: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            c => html.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const SAMPLE: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
 <description>
  <title-info>
   <author><first-name>Иван</first-name><last-name>Тестов</last-name></author>
   <book-title>Тестовая книга</book-title>
   <lang>ru</lang>
   <date>2019-01-01</date>
  </title-info>
  <publish-info><publisher>Тест Пресс</publisher></publish-info>
 </description>
 <body>
  <section>
   <title><p>Глава 1</p></title>
   <p>Первый <emphasis>абзац</emphasis>.</p>
   <image l:href="#pic1"/>
  </section>
  <section>
   <title><p>Глава 2</p></title>
   <p>Второй абзац.</p>
  </section>
 </body>
 <binary id="pic1" content-type="image/png">iVBORw0KGgo=</binary>
</FictionBook>"##;

    #[test]
    fn parse_fb2_sections_meta_images() {
        let dir = std::env::temp_dir().join(format!("argus_fb2_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("book.fb2");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(SAMPLE.as_bytes())
            .unwrap();

        let book = parse(&path).unwrap();
        assert_eq!(book.meta.title, "Тестовая книга");
        assert_eq!(book.meta.authors, vec!["Иван Тестов"]);
        assert_eq!(book.meta.year, Some(2019));
        assert_eq!(book.chapter_html.len(), 2);
        assert_eq!(book.toc.len(), 2);
        assert_eq!(book.toc[1].chapter, 2);
        assert!(book.chapter_html[0].contains("<em>абзац</em>"));
        assert!(book.chapter_html[0].contains(r#"data-argus-res="pic1""#));
        assert!(book.resources.contains_key("pic1"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
