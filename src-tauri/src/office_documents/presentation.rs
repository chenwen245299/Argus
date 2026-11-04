use super::*;
use std::collections::{HashMap, HashSet};

pub(super) fn read(mut package: Package, args: &Value) -> Result<Value, String> {
    fn yes() -> bool {
        true
    }
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
        slides: Option<Vec<usize>>,
        #[serde(default = "yes")]
        include_notes: bool,
    }
    let a: Args = serde_json::from_value(args.clone())
        .map_err(|e| format!("Invalid PowerPoint reading arguments: {e}"))?;
    let page = PageArgs {
        _attachment_id: a.attachment_id,
        view: a.view,
        offset: a.offset,
        max_chars: a.max_chars,
    };
    page.validate()?;
    let main = package.main_part(Format::Pptx)?;
    let xml = package.xml(&main)?;
    let doc = parse_xml(&xml)?;
    let relationships: HashMap<_, _> = package
        .relationships(&main)?
        .into_iter()
        .filter(|r| r.kind.ends_with("/slide"))
        .map(|r| (r.id, r.target))
        .collect();
    let paths = doc
        .descendants()
        .find(|n| tag(*n, "sldIdLst"))
        .map(|list| {
            list.children()
                .filter(|n| tag(*n, "sldId"))
                .map(|node| {
                    let id = node
                        .attributes()
                        .find(|a| a.name() == "id" && a.namespace().is_some())
                        .map(|a| a.value())
                        .ok_or("Missing slide relationship id.")?;
                    relationships
                        .get(id)
                        .cloned()
                        .ok_or_else(|| "Slide relationship is missing or external.".to_string())
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let wanted = match a.slides {
        Some(slides) => {
            if slides.is_empty()
                || slides.len() > 20
                || slides.iter().any(|&n| n == 0 || n > paths.len())
            {
                return Err(format!(
                    "Choose 1–20 valid slide indices (this presentation has {} slides).",
                    paths.len()
                ));
            }
            let mut seen = HashSet::new();
            slides
                .into_iter()
                .filter(|n| seen.insert(*n))
                .collect::<Vec<_>>()
        }
        None => {
            if page.view == "overview" {
                (1..=paths.len()).skip(page.offset).take(100).collect()
            } else {
                (1..=paths.len()).collect()
            }
        }
    };
    let mut overview = Vec::new();
    let mut text = String::new();
    let mut image_count = 0;
    for number in wanted {
        let path = &paths[number - 1];
        let xml = package.xml(path)?;
        let slide = parse_xml(&xml)?;
        let hidden = slide
            .root_element()
            .attribute("show")
            .is_some_and(|v| v == "0" || v == "false");
        let title = slide
            .descendants()
            .find(|n| {
                tag(*n, "sp")
                    && n.descendants().any(|p| {
                        tag(p, "ph")
                            && p.attribute("type")
                                .is_some_and(|v| v == "title" || v == "ctrTitle")
                    })
            })
            .map(rich_text)
            .unwrap_or_default();
        overview.push(json!({"slide":number,"title":title,"hidden":hidden}));
        if page.view == "overview" {
            continue;
        }
        text.push_str(&format!(
            "\n[Slide {number}{}] {title}\n",
            if hidden { "; hidden" } else { "" }
        ));
        slide_text(slide.root_element(), &mut text, &mut image_count);
        let relationships = package.relationships(path)?;
        for rel in relationships {
            if a.include_notes && rel.kind.ends_with("/notesSlide") {
                let notes_xml = package.xml(&rel.target)?;
                let notes = parse_xml(&notes_xml)?;
                text.push_str("[Speaker notes]\n");
                for shape in notes.descendants().filter(|n| tag(*n, "sp")) {
                    if shape.descendants().any(|p| {
                        tag(p, "ph")
                            && p.attribute("type")
                                .is_some_and(|v| matches!(v, "sldNum" | "hdr" | "ftr" | "dt"))
                    }) {
                        continue;
                    }
                    slide_text(shape, &mut text, &mut image_count);
                }
            }
            if rel.kind.ends_with("/chart") {
                let chart_xml = package.xml(&rel.target)?;
                let chart = parse_xml(&chart_xml)?;
                text.push_str("[Chart cached data; not a visual inspection]\n");
                for series in chart.descendants().filter(|n| tag(*n, "ser")) {
                    for cache in series.children().filter(|n| {
                        matches!(
                            n.tag_name().name(),
                            "tx" | "cat" | "val" | "xVal" | "yVal" | "bubbleSize"
                        )
                    }) {
                        let values: Vec<_> = cache
                            .descendants()
                            .filter(|n| tag(*n, "v"))
                            .filter_map(|n| n.text())
                            .collect();
                        text.push_str(&format!("{}: {}\n", cache.tag_name().name(), json!(values)));
                    }
                }
            }
        }
    }
    let mut result = if page.view == "overview" {
        json!({"slides":overview,"offset":page.offset,"next_offset":if page.offset.saturating_add(100)<paths.len(){Some(page.offset.saturating_add(100))}else{None},"note":SOURCE_NOTE})
    } else {
        text_result(&text, &page)
    };
    result["format"] = json!("pptx");
    result["total_slides"] = json!(paths.len());
    result["unrendered_images"] = json!(image_count);
    Ok(result)
}
fn slide_text(node: Node<'_, '_>, text: &mut String, image_count: &mut usize) {
    for child in node.children().filter(|n| n.is_element()) {
        if tag(child, "p") {
            let mut line = String::new();
            for n in child.descendants().filter(|n| n.is_element()) {
                match n.tag_name().name() {
                    "t" => line.push_str(n.text().unwrap_or_default()),
                    "br" => line.push('\n'),
                    "tab" => line.push('\t'),
                    _ => {}
                }
            }
            text.push_str(&line);
            text.push('\n');
        } else if tag(child, "tbl") {
            text.push_str("[Table]\n");
            for (index, row) in child.children().filter(|n| tag(*n, "tr")).enumerate() {
                let cells: Vec<_> = row
                    .children()
                    .filter(|n| tag(*n, "tc"))
                    .map(|cell| {
                        let mut value = String::new();
                        slide_text(cell, &mut value, image_count);
                        json!(value.trim_end()).to_string()
                    })
                    .collect();
                text.push_str(&format!("Row {}\t{}\n", index + 1, cells.join("\t")));
            }
        } else if tag(child, "cNvPr") {
            if let Some(alt) = child
                .attribute("descr")
                .or_else(|| child.attribute("title"))
            {
                text.push_str(&format!("[Image/shape alt text: {alt}]\n"));
            }
        } else {
            if tag(child, "blip") {
                *image_count += 1;
            }
            slide_text(child, text, image_count);
        }
    }
}
