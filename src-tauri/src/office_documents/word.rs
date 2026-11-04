use super::*;
use std::collections::HashMap;

pub(super) fn read(mut package: Package, args: &Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Args {
        attachment_id: String,
        #[serde(default = "default_view")]
        view: String,
        #[serde(default)]
        offset: usize,
        #[serde(default = "default_chars")]
        max_chars: usize,
        #[serde(default = "body")]
        section: String,
    }
    fn body() -> String {
        "body".into()
    }
    let a: Args = serde_json::from_value(args.clone())
        .map_err(|e| format!("Invalid Word reading arguments: {e}"))?;
    let page = PageArgs {
        _attachment_id: a.attachment_id,
        view: a.view,
        offset: a.offset,
        max_chars: a.max_chars,
    };
    page.validate()?;
    if !["body", "headers", "footnotes", "endnotes", "comments"].contains(&a.section.as_str()) {
        return Err("Unknown Word document section.".into());
    }
    let main = package.main_part(Format::Docx)?;
    let relationships = package.relationships(&main)?;
    let styles = if let Some(rel) = relationships.iter().find(|r| r.kind.ends_with("/styles")) {
        let xml = package.xml(&rel.target)?;
        let doc = parse_xml(&xml)?;
        doc.descendants()
            .filter(|n| tag(*n, "style"))
            .filter_map(|node| {
                let id = attribute(node, "styleId")?;
                let name = node
                    .children()
                    .find(|n| tag(*n, "name"))
                    .and_then(|n| attribute(n, "val"))
                    .unwrap_or(id);
                let level = node
                    .descendants()
                    .find(|n| tag(*n, "outlineLvl"))
                    .and_then(|n| attribute(n, "val"))
                    .and_then(|v| v.parse::<u8>().ok());
                Some((id.to_string(), (name.to_string(), level)))
            })
            .collect::<HashMap<_, _>>()
    } else {
        HashMap::new()
    };
    let parts: Vec<_> = if a.section == "body" {
        vec![main]
    } else {
        relationships
            .iter()
            .filter(|rel| {
                if a.section == "headers" {
                    rel.kind.ends_with("/header") || rel.kind.ends_with("/footer")
                } else {
                    rel.kind.ends_with(&format!("/{}", a.section))
                }
            })
            .map(|r| r.target.clone())
            .collect()
    };
    let mut text = String::new();
    let mut paragraphs = 0;
    let mut tables = 0;
    let mut images = 0;
    let mut headings = Vec::new();
    for (part_index, part) in parts.iter().enumerate() {
        let xml = package.xml(part)?;
        let doc = parse_xml(&xml)?;
        if doc.root_element().tag_name().name() != "document" && a.section == "body" {
            return Err("Invalid Word main document.".into());
        }
        let mut indices = HashMap::new();
        for node in doc.descendants().filter(|n| tag(*n, "p") && !hidden(*n)) {
            paragraphs += 1;
            indices.insert(node.range().start, paragraphs);
            if let Some(style) = node
                .children()
                .find(|n| tag(*n, "pPr"))
                .and_then(|n| n.children().find(|n| tag(*n, "pStyle")))
                .and_then(|n| attribute(n, "val"))
            {
                let (name, level) = styles.get(style).cloned().unwrap_or((style.into(), None));
                if level.is_some_and(|v| v < 9)
                    || name.to_ascii_lowercase().starts_with("heading")
                    || name.starts_with("标题")
                {
                    headings.push(json!({"paragraph":paragraphs,"style":name,"level":level.map(|v|v+1),"text":paragraph_text(node)}));
                }
            }
        }
        images += doc
            .descendants()
            .filter(|n| tag(*n, "blip") && !hidden(*n))
            .count();
        if a.section != "body" {
            text.push_str(&format!("\n[{} part {}]\n", a.section, part_index + 1));
        }
        let container = doc
            .descendants()
            .find(|n| tag(*n, "body"))
            .unwrap_or(doc.root_element());
        render_blocks(container, &indices, &mut tables, &mut text);
    }
    let mut result = if page.view == "overview" {
        json!({"headings":headings.iter().skip(page.offset).take(100).collect::<Vec<_>>(),"heading_count":headings.len(),"offset":page.offset,"next_offset":if page.offset.saturating_add(100)<headings.len(){Some(page.offset.saturating_add(100))}else{None},"note":SOURCE_NOTE})
    } else {
        text_result(&text, &page)
    };
    result["format"] = json!("docx");
    result["section"] = json!(a.section);
    result["paragraph_count"] = json!(paragraphs);
    result["table_count"] = json!(tables);
    result["unrendered_images"] = json!(images);
    result["available_sections"] = json!(["body", "headers", "footnotes", "endnotes", "comments"]
        .into_iter()
        .filter(|section| *section == "body"
            || relationships.iter().any(|r| if *section == "headers" {
                r.kind.ends_with("/header") || r.kind.ends_with("/footer")
            } else {
                r.kind.ends_with(&format!("/{section}"))
            }))
        .collect::<Vec<_>>());
    Ok(result)
}
fn hidden(node: Node<'_, '_>) -> bool {
    node.ancestors().any(|n| {
        tag(n, "del")
            || tag(n, "moveFrom")
            || attribute(n, "type")
                .is_some_and(|v| v == "separator" || v == "continuationSeparator")
    })
}
fn paragraph_text(node: Node<'_, '_>) -> String {
    let mut text = String::new();
    for n in node.descendants().filter(|n| n.is_element() && !hidden(*n)) {
        match n.tag_name().name() {
            "t" => text.push_str(n.text().unwrap_or_default()),
            "tab" => text.push('\t'),
            "br" | "cr" => text.push('\n'),
            "footnoteReference" | "endnoteReference" => text.push_str(&format!(
                " [{}:{}]",
                n.tag_name().name(),
                attribute(n, "id").unwrap_or("?")
            )),
            "docPr" => {
                if let Some(alt) = n.attribute("descr").or_else(|| n.attribute("title")) {
                    text.push_str(&format!(" [Image alt text: {alt}]"));
                }
            }
            _ => {}
        }
    }
    text
}
fn render_blocks(
    node: Node<'_, '_>,
    indices: &HashMap<usize, usize>,
    tables: &mut usize,
    text: &mut String,
) {
    for child in node.children().filter(|n| n.is_element() && !hidden(*n)) {
        if tag(child, "p") {
            text.push_str(&format!(
                "[Paragraph {}] {}\n",
                indices.get(&child.range().start).copied().unwrap_or(0),
                paragraph_text(child)
            ));
        } else if tag(child, "tbl") {
            *tables += 1;
            text.push_str(&format!("\n[Table {}]\n", tables));
            for (row_index, row) in child.children().filter(|n| tag(*n, "tr")).enumerate() {
                let cells: Vec<_> = row
                    .children()
                    .filter(|n| tag(*n, "tc"))
                    .map(|cell| {
                        let mut content = String::new();
                        render_blocks(cell, indices, tables, &mut content);
                        json!(content.trim_end()).to_string()
                    })
                    .collect();
                text.push_str(&format!("Row {}\t{}\n", row_index + 1, cells.join("\t")));
            }
            text.push('\n');
        } else {
            render_blocks(child, indices, tables, text);
        }
    }
}
