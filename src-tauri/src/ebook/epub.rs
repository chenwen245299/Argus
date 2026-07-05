//! EPUB 2/3 parser: zip → container.xml → OPF (metadata/manifest/spine) →
//! nav.xhtml (EPUB3) or toc.ncx (EPUB2) → normalized [`ParsedEbook`].
//!
//! Chapter XHTML is deliberately *not* XML-parsed — real-world books are full
//! of non-well-formed markup. Bodies are extracted with regexes and the
//! frontend's DOMPurify pass is the actual sanitizer.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use super::{
    decode_entities, extract_body_html, first_heading_text, html_to_text, normalize_zip_path,
    resolve_href, rewrite_chapter_html, EbookLinkTarget, EbookMeta, EbookTocEntry, ParsedEbook,
    ResourceData, ResourceEntry,
};

type Archive = zip::ZipArchive<std::io::BufReader<std::fs::File>>;

pub fn parse(path: &Path) -> Result<ParsedEbook, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("Open EPUB: {e}"))?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("Not a valid EPUB (zip) file: {e}"))?;

    // 1. container.xml → OPF path
    let container = read_zip_string(&mut archive, "META-INF/container.xml")
        .map_err(|e| format!("EPUB missing container.xml: {e}"))?;
    let opf_path = parse_container(&container)
        .ok_or_else(|| "container.xml has no rootfile entry".to_string())?;
    let opf_path = normalize_zip_path(&opf_path);

    // 2. OPF → metadata + manifest + spine
    let opf_xml = read_zip_string(&mut archive, &opf_path)
        .map_err(|e| format!("EPUB missing OPF ({opf_path}): {e}"))?;
    let opf = parse_opf(&opf_xml, &opf_path)?;

    let mut meta = opf.meta;
    if meta.title.trim().is_empty() {
        meta.title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string();
    }

    // 3. Resources (images) from the manifest
    let mut resources: HashMap<String, ResourceEntry> = HashMap::new();
    for item in opf.manifest.values() {
        if item.media_type.starts_with("image/") {
            resources.insert(
                item.zip_path.clone(),
                ResourceEntry {
                    mime: if item.media_type == "image/jpg" {
                        "image/jpeg".to_string()
                    } else {
                        item.media_type.clone()
                    },
                    data: ResourceData::ZipEntry { inner_path: item.zip_path.clone() },
                },
            );
        }
    }

    // 4. Spine → ordered chapter documents
    if opf.spine.is_empty() {
        return Err("EPUB spine is empty".to_string());
    }
    let spine_index: HashMap<&str, u32> = opf
        .spine
        .iter()
        .enumerate()
        .map(|(i, p)| (p.as_str(), (i + 1) as u32))
        .collect();
    let mut chapter_html: Vec<String> = Vec::with_capacity(opf.spine.len());
    for (i, zip_path) in opf.spine.iter().enumerate() {
        let raw = read_zip_string(&mut archive, zip_path).unwrap_or_default();
        let body = extract_body_html(&raw);
        let chapter_dir_file = zip_path.clone();
        let current_chapter = (i + 1) as u32;
        let html = rewrite_chapter_html(
            &body,
            |href| {
                let h = href.trim();
                let lower = h.to_ascii_lowercase();
                if h.is_empty()
                    || lower.starts_with("data:")
                    || lower.starts_with("http:")
                    || lower.starts_with("https:")
                {
                    return None;
                }
                let resolved = resolve_href(&chapter_dir_file, h);
                resources.contains_key(&resolved).then_some(resolved)
            },
            |href| {
                let h = href.trim();
                if h.is_empty() {
                    return None;
                }
                let lower = h.to_ascii_lowercase();
                if lower.starts_with("http://") || lower.starts_with("https://") {
                    return Some(EbookLinkTarget::External(h.to_string()));
                }
                if lower.starts_with("data:")
                    || lower.starts_with("javascript:")
                    || lower.starts_with("mailto:")
                    || lower.starts_with("file:")
                {
                    return None;
                }
                if let Some(anchor) = h.strip_prefix('#') {
                    return Some(EbookLinkTarget::Chapter {
                        chapter: current_chapter,
                        anchor: (!anchor.is_empty()).then(|| anchor.to_string()),
                    });
                }
                href_to_chapter(h, &chapter_dir_file, &spine_index)
                    .map(|(chapter, anchor)| EbookLinkTarget::Chapter { chapter, anchor })
            },
        );
        chapter_html.push(html);
    }

    // 5. TOC: EPUB3 nav first, then NCX
    let mut toc: Vec<EbookTocEntry> = Vec::new();
    if let Some(nav_path) = &opf.nav_path {
        if let Ok(nav_xml) = read_zip_string(&mut archive, nav_path) {
            toc = parse_nav(&nav_xml, nav_path, &spine_index);
        }
    }
    if toc.is_empty() {
        if let Some(ncx_path) = &opf.ncx_path {
            if let Ok(ncx_xml) = read_zip_string(&mut archive, ncx_path) {
                toc = parse_ncx(&ncx_xml, ncx_path, &spine_index);
            }
        }
    }

    // 6. Per-chapter titles: TOC entry pointing at the chapter start, else
    //    the first heading in the markup.
    let mut chapter_titles: Vec<Option<String>> = vec![None; chapter_html.len()];
    for entry in &toc {
        let idx = (entry.chapter as usize).saturating_sub(1);
        if idx < chapter_titles.len() && chapter_titles[idx].is_none() && entry.anchor.is_none() {
            chapter_titles[idx] = Some(entry.title.clone());
        }
    }
    for (i, html) in chapter_html.iter().enumerate() {
        if chapter_titles[i].is_none() {
            chapter_titles[i] = first_heading_text(html);
        }
    }

    let chapter_text: Vec<String> = chapter_html.iter().map(|h| html_to_text(h)).collect();

    Ok(ParsedEbook {
        format: "epub".to_string(),
        book_path: path.to_path_buf(),
        meta,
        toc,
        chapter_html,
        chapter_titles,
        chapter_text,
        resources,
    })
}

// ── zip helpers ───────────────────────────────────────────────────────────────

fn read_zip_string(archive: &mut Archive, name: &str) -> Result<String, String> {
    let exact = if archive.index_for_name(name).is_some() {
        name.to_string()
    } else {
        // Some books disagree on case between OPF hrefs and actual entries.
        archive
            .file_names()
            .find(|n| n.eq_ignore_ascii_case(name))
            .map(|s| s.to_string())
            .ok_or_else(|| format!("{name} not in archive"))?
    };
    let mut zf = archive.by_name(&exact).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    zf.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let mut s = String::from_utf8_lossy(&buf).into_owned();
    if s.starts_with('\u{feff}') {
        s.remove(0);
    }
    Ok(s)
}

// ── container.xml ─────────────────────────────────────────────────────────────

fn parse_container(xml: &str) -> Option<String> {
    with_xml_doc(xml, |doc| {
        doc.descendants()
            .find(|n| n.has_tag_name_local("rootfile"))
            .and_then(|n| n.attribute("full-path"))
            .map(|s| s.to_string())
    })
    .flatten()
}

// ── OPF ───────────────────────────────────────────────────────────────────────

struct ManifestItem {
    zip_path: String,
    media_type: String,
}

struct Opf {
    meta: EbookMeta,
    /// manifest id → item
    manifest: HashMap<String, ManifestItem>,
    /// spine as normalized zip paths of (x)html documents, reading order
    spine: Vec<String>,
    nav_path: Option<String>,
    ncx_path: Option<String>,
}

fn parse_opf(xml: &str, opf_path: &str) -> Result<Opf, String> {
    with_xml_doc(xml, |doc| parse_opf_doc(doc, opf_path))
        .ok_or_else(|| "OPF is not parseable XML".to_string())
}

fn parse_opf_doc(doc: &roxmltree::Document, opf_path: &str) -> Opf {
    let root = doc.root_element();

    // metadata (Dublin Core local names)
    let mut meta = EbookMeta::default();
    if let Some(md) = root.children().find(|n| n.has_tag_name_local("metadata")) {
        for n in md.children().filter(|n| n.is_element()) {
            let text = || n.text().map(|t| decode_entities(t.trim())).filter(|t| !t.is_empty());
            match n.tag_name().name() {
                "title" if meta.title.is_empty() => {
                    if let Some(t) = text() {
                        meta.title = t;
                    }
                }
                "creator" => {
                    if let Some(t) = text() {
                        meta.authors.push(t);
                    }
                }
                "date" if meta.year.is_none() => {
                    meta.year = n.text().and_then(extract_year);
                }
                "language" if meta.language.is_none() => meta.language = text(),
                "publisher" if meta.publisher.is_none() => meta.publisher = text(),
                "identifier" if meta.identifier.is_none() => meta.identifier = text(),
                "description" if meta.description.is_none() => meta.description = text(),
                _ => {}
            }
        }
    }

    // manifest
    let mut manifest: HashMap<String, ManifestItem> = HashMap::new();
    let mut nav_path: Option<String> = None;
    let mut ncx_id_fallback: Option<String> = None;
    if let Some(mf) = root.children().find(|n| n.has_tag_name_local("manifest")) {
        for item in mf.children().filter(|n| n.has_tag_name_local("item")) {
            let (Some(id), Some(href)) = (item.attribute("id"), item.attribute("href")) else {
                continue;
            };
            let media_type = item.attribute("media-type").unwrap_or("").to_string();
            let zip_path = resolve_href(opf_path, href);
            if item
                .attribute("properties")
                .is_some_and(|p| p.split_whitespace().any(|t| t == "nav"))
            {
                nav_path = Some(zip_path.clone());
            }
            if media_type == "application/x-dtbncx+xml" {
                ncx_id_fallback = Some(id.to_string());
            }
            manifest.insert(id.to_string(), ManifestItem { zip_path, media_type });
        }
    }

    // spine
    let mut spine: Vec<String> = Vec::new();
    let ncx_path: Option<String>;
    if let Some(sp) = root.children().find(|n| n.has_tag_name_local("spine")) {
        let ncx_id = sp.attribute("toc").map(|s| s.to_string()).or(ncx_id_fallback);
        ncx_path = ncx_id.and_then(|id| manifest.get(&id).map(|i| i.zip_path.clone()));
        for itemref in sp.children().filter(|n| n.has_tag_name_local("itemref")) {
            if itemref.attribute("linear").is_some_and(|l| l.eq_ignore_ascii_case("no")) {
                continue;
            }
            let Some(idref) = itemref.attribute("idref") else { continue };
            let Some(item) = manifest.get(idref) else { continue };
            if item.media_type.contains("xhtml") || item.media_type.contains("html") {
                spine.push(item.zip_path.clone());
            }
        }
    } else {
        ncx_path = ncx_id_fallback.and_then(|id| manifest.get(&id).map(|i| i.zip_path.clone()));
    }

    Opf { meta, manifest, spine, nav_path, ncx_path }
}

// ── EPUB3 nav.xhtml ───────────────────────────────────────────────────────────

const OPS_NS: &str = "http://www.idpf.org/2007/ops";

fn parse_nav(xml: &str, nav_path: &str, spine_index: &HashMap<&str, u32>) -> Vec<EbookTocEntry> {
    with_xml_doc(xml, |doc| {
        // Prefer <nav epub:type="toc">; fall back to the first <nav> with an <ol>.
        let nav = doc
            .descendants()
            .find(|n| {
                n.has_tag_name_local("nav")
                    && n.attribute((OPS_NS, "type"))
                        .is_some_and(|t| t.split_whitespace().any(|v| v == "toc"))
            })
            .or_else(|| doc.descendants().find(|n| n.has_tag_name_local("nav")));
        let Some(nav) = nav else { return vec![] };
        let Some(ol) = nav.children().find(|n| n.has_tag_name_local("ol")) else {
            return vec![];
        };
        let mut out = Vec::new();
        walk_nav_ol(ol, 1, nav_path, spine_index, &mut out);
        out
    })
    .unwrap_or_default()
}

fn walk_nav_ol(
    ol: roxmltree::Node,
    level: u8,
    nav_path: &str,
    spine_index: &HashMap<&str, u32>,
    out: &mut Vec<EbookTocEntry>,
) {
    for li in ol.children().filter(|n| n.has_tag_name_local("li")) {
        if let Some(a) = li.descendants().find(|n| n.has_tag_name_local("a")) {
            let title = collect_text(a);
            if let (Some(href), false) = (a.attribute("href"), title.is_empty()) {
                if let Some((chapter, anchor)) = href_to_chapter(href, nav_path, spine_index) {
                    out.push(EbookTocEntry { title, level, chapter, anchor });
                }
            }
        }
        if let Some(sub) = li.children().find(|n| n.has_tag_name_local("ol")) {
            walk_nav_ol(sub, level.saturating_add(1).min(6), nav_path, spine_index, out);
        }
    }
}

// ── EPUB2 toc.ncx ─────────────────────────────────────────────────────────────

fn parse_ncx(xml: &str, ncx_path: &str, spine_index: &HashMap<&str, u32>) -> Vec<EbookTocEntry> {
    with_xml_doc(xml, |doc| {
        let Some(nav_map) = doc.descendants().find(|n| n.has_tag_name_local("navMap")) else {
            return vec![];
        };
        let mut out = Vec::new();
        for np in nav_map.children().filter(|n| n.has_tag_name_local("navPoint")) {
            walk_nav_point(np, 1, ncx_path, spine_index, &mut out);
        }
        out
    })
    .unwrap_or_default()
}

fn walk_nav_point(
    np: roxmltree::Node,
    level: u8,
    ncx_path: &str,
    spine_index: &HashMap<&str, u32>,
    out: &mut Vec<EbookTocEntry>,
) {
    let title = np
        .children()
        .find(|n| n.has_tag_name_local("navLabel"))
        .and_then(|nl| nl.children().find(|n| n.has_tag_name_local("text")))
        .map(collect_text)
        .unwrap_or_default();
    let src = np
        .children()
        .find(|n| n.has_tag_name_local("content"))
        .and_then(|c| c.attribute("src"));
    if let (Some(src), false) = (src, title.is_empty()) {
        if let Some((chapter, anchor)) = href_to_chapter(src, ncx_path, spine_index) {
            out.push(EbookTocEntry { title, level, chapter, anchor });
        }
    }
    for child in np.children().filter(|n| n.has_tag_name_local("navPoint")) {
        walk_nav_point(child, level.saturating_add(1).min(6), ncx_path, spine_index, out);
    }
}

// ── shared XML helpers ────────────────────────────────────────────────────────

/// Parse XML and hand the document to `f`, retrying once with HTML named
/// entities numericized — nav documents in the wild use `&nbsp;` etc., which
/// strict XML rejects. Closure-based because roxmltree borrows the input.
fn with_xml_doc<T>(xml: &str, f: impl FnOnce(&roxmltree::Document) -> T) -> Option<T> {
    match roxmltree::Document::parse(xml) {
        Ok(d) => Some(f(&d)),
        Err(_) => {
            let fixed = numericize_entities(xml);
            roxmltree::Document::parse(&fixed).ok().map(|d| f(&d))
        }
    }
}

fn numericize_entities(xml: &str) -> String {
    use regex::Regex;
    static ENT_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let ent_re = ENT_RE.get_or_init(|| Regex::new(r"&([a-zA-Z][a-zA-Z0-9]{1,10});").unwrap());
    ent_re
        .replace_all(xml, |caps: &regex::Captures| {
            let name = &caps[1];
            match name {
                "amp" | "lt" | "gt" | "quot" | "apos" => format!("&{name};"),
                _ => {
                    let decoded = decode_entities(&format!("&{name};"));
                    match decoded.chars().next() {
                        Some(c) if decoded.chars().count() == 1 && c != '&' => {
                            format!("&#{};", c as u32)
                        }
                        _ => " ".to_string(),
                    }
                }
            }
        })
        .into_owned()
}

fn collect_text(node: roxmltree::Node) -> String {
    let mut s = String::new();
    for d in node.descendants().filter(|d| d.is_text()) {
        if let Some(t) = d.text() {
            s.push_str(t);
            s.push(' ');
        }
    }
    decode_entities(s.split_whitespace().collect::<Vec<_>>().join(" ").trim())
}

fn href_to_chapter(
    href: &str,
    base_file: &str,
    spine_index: &HashMap<&str, u32>,
) -> Option<(u32, Option<String>)> {
    let anchor = href.split('#').nth(1).map(|s| s.to_string()).filter(|s| !s.is_empty());
    let path = resolve_href(base_file, href);
    spine_index.get(path.as_str()).map(|idx| (*idx, anchor))
}

fn extract_year(s: &str) -> Option<u32> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        if bytes[i..i + 4].iter().all(|b| b.is_ascii_digit()) {
            let y: u32 = s[i..i + 4].parse().ok()?;
            if (1400..=2200).contains(&y) {
                return Some(y);
            }
        }
    }
    None
}

// roxmltree has no local-name matcher on Node by default; tiny extension trait.
trait LocalName {
    fn has_tag_name_local(&self, name: &str) -> bool;
}

impl LocalName for roxmltree::Node<'_, '_> {
    fn has_tag_name_local(&self, name: &str) -> bool {
        self.is_element() && self.tag_name().name() == name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn build_test_epub(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("test.epub");
        let file = std::fs::File::create(&path).unwrap();
        let mut z = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();

        z.start_file("mimetype", opts).unwrap();
        z.write_all(b"application/epub+zip").unwrap();

        z.start_file("META-INF/container.xml", opts).unwrap();
        z.write_all(r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#.as_bytes()).unwrap();

        z.start_file("OEBPS/content.opf", opts).unwrap();
        z.write_all(r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="uid">
  <metadata>
    <dc:title>测试书</dc:title>
    <dc:creator>作者甲</dc:creator>
    <dc:creator>Author B</dc:creator>
    <dc:date>2021-05-01</dc:date>
    <dc:language>zh</dc:language>
    <dc:publisher>Test Press</dc:publisher>
    <dc:identifier id="uid">uuid:abc</dc:identifier>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="img1" href="images/pic.png" media-type="image/png"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>"#.as_bytes()).unwrap();

        z.start_file("OEBPS/nav.xhtml", opts).unwrap();
        z.write_all(r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body><nav epub:type="toc"><ol>
  <li><a href="text/ch1.xhtml">第一章</a>
    <ol><li><a href="text/ch1.xhtml#s2">第一节</a></li></ol>
  </li>
  <li><a href="text/ch2.xhtml">第二章</a></li>
</ol></nav></body></html>"#.as_bytes()).unwrap();

        z.start_file("OEBPS/text/ch1.xhtml", opts).unwrap();
        z.write_all(r#"<html><head><style>p{color:red}</style></head>
<body><h1>第一章</h1><p>正文第一段。</p><img src="../images/pic.png" alt="fig"/>
<p id="s2">第二段落。</p></body></html>"#.as_bytes()).unwrap();

        z.start_file("OEBPS/text/ch2.xhtml", opts).unwrap();
        z.write_all(r#"<html><body><h1>第二章</h1><p>Chapter two body.</p>
<a href="ch1.xhtml">back</a></body></html>"#.as_bytes()).unwrap();

        z.start_file("OEBPS/images/pic.png", opts).unwrap();
        z.write_all(&[0x89, b'P', b'N', b'G']).unwrap();

        z.finish().unwrap();
        path
    }

    #[test]
    fn parse_minimal_epub3() {
        let dir = std::env::temp_dir().join(format!("argus_epub_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = build_test_epub(&dir);

        let book = parse(&path).unwrap();
        assert_eq!(book.meta.title, "测试书");
        assert_eq!(book.meta.authors, vec!["作者甲", "Author B"]);
        assert_eq!(book.meta.year, Some(2021));
        assert_eq!(book.meta.publisher.as_deref(), Some("Test Press"));
        assert_eq!(book.chapter_html.len(), 2);

        // TOC: 3 entries, nested level, anchors resolved
        assert_eq!(book.toc.len(), 3);
        assert_eq!(book.toc[0],_entry("第一章", 1, 1, None));
        assert_eq!(book.toc[1], _entry("第一节", 2, 1, Some("s2")));
        assert_eq!(book.toc[2], _entry("第二章", 1, 2, None));

        // Chapter HTML: style stripped, img rewritten, links made app-handled
        assert!(!book.chapter_html[0].contains("<style"));
        assert!(book.chapter_html[0].contains(r#"data-argus-res="OEBPS/images/pic.png""#));
        assert!(!book.chapter_html[1].contains(r#"href="ch1.xhtml""#));
        assert!(book.chapter_html[1].contains(r#"data-argus-link-chapter="1""#));

        // Titles + text
        assert_eq!(book.chapter_titles[0].as_deref(), Some("第一章"));
        assert!(book.chapter_text[0].contains("正文第一段。"));

        // Resource readable
        let res = super::super::read_resource(&book, "OEBPS/images/pic.png").unwrap();
        assert_eq!(res.mime, "image/png");

        std::fs::remove_dir_all(&dir).ok();
    }

    fn _entry(title: &str, level: u8, chapter: u32, anchor: Option<&str>) -> (String, u8, u32, Option<String>) {
        (title.to_string(), level, chapter, anchor.map(|s| s.to_string()))
    }

    impl PartialEq<(String, u8, u32, Option<String>)> for EbookTocEntry {
        fn eq(&self, other: &(String, u8, u32, Option<String>)) -> bool {
            self.title == other.0
                && self.level == other.1
                && self.chapter == other.2
                && self.anchor == other.3
        }
    }
}
