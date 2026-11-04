//! Word 97–2003 piece-table text and live PowerPoint 97–2003 persist objects.
//! Follow the current save's references; never scan stale/deleted text records.
//! Specs: https://learn.microsoft.com/openspecs/office_file_formats/ms-doc/
//!        https://learn.microsoft.com/openspecs/office_file_formats/ms-ppt/
use super::*;
use std::collections::{HashMap, HashSet};
type Compound = cfb::CompoundFile<Cursor<Vec<u8>>>;
const OLE: &[u8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1";
fn compound(bytes: &[u8]) -> Result<Compound, String> {
    if bytes.len() > MAX_FILE || !bytes.starts_with(OLE) {
        return Err("Expected an unencrypted Office 97–2003 binary file (.doc/.ppt/.xls).".into());
    }
    let file = cfb::CompoundFile::open(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("Read legacy Office container: {e}"))?;
    if file.is_stream("/EncryptedPackage") || file.is_stream("/EncryptionInfo") {
        return Err("Password-protected Office files must be decrypted before attaching.".into());
    }
    Ok(file)
}
fn stream(file: &mut Compound, name: &str) -> Result<Vec<u8>, String> {
    let source = file
        .open_stream(name)
        .map_err(|e| format!("Read legacy Office stream {name}: {e}"))?;
    let mut bytes = Vec::new();
    source
        .take(MAX_FILE as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE {
        return Err("Legacy Office stream exceeds the reading limit.".into());
    }
    Ok(bytes)
}
fn u16_at(data: &[u8], at: usize) -> Result<u16, String> {
    let bytes = data
        .get(at..at.checked_add(2).ok_or("Invalid binary offset.")?)
        .ok_or("Truncated legacy Office record.")?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}
fn u32_at(data: &[u8], at: usize) -> Result<u32, String> {
    let bytes = data
        .get(at..at.checked_add(4).ok_or("Invalid binary offset.")?)
        .ok_or("Truncated legacy Office record.")?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
fn bytes_at(data: &[u8], start: usize, len: usize) -> Result<&[u8], String> {
    data.get(
        start
            ..start
                .checked_add(len)
                .ok_or("Invalid binary record length.")?,
    )
    .ok_or_else(|| "Truncated legacy Office data.".into())
}
fn utf16(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() % 2 != 0 {
        return Err("Invalid UTF-16 legacy Office text.".into());
    }
    String::from_utf16(
        &bytes
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect::<Vec<_>>(),
    )
    .map_err(|_| "Invalid UTF-16 legacy Office text.".into())
}

pub(super) fn validate(bytes: &[u8], format: Format) -> Result<(), String> {
    let file = compound(bytes)?;
    let valid = match format {
        Format::Doc => file.is_stream("/WordDocument"),
        Format::Ppt => file.is_stream("/PowerPoint Document") && file.is_stream("/Current User"),
        Format::Xls => file.is_stream("/Workbook") || file.is_stream("/Book"),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err("Legacy document contents do not match its file extension.".into())
    }
}
pub(super) fn read(bytes: &[u8], format: Format, args: &Value) -> Result<Value, String> {
    match format {
        Format::Doc => word(bytes, args),
        Format::Ppt => powerpoint(bytes, args),
        _ => Err("Unsupported legacy Office format.".into()),
    }
}

fn word(bytes: &[u8], args: &Value) -> Result<Value, String> {
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
    let a: Args =
        serde_json::from_value(args.clone()).map_err(|e| format!("Invalid Word arguments: {e}"))?;
    let page = PageArgs {
        _attachment_id: a.attachment_id,
        view: a.view,
        offset: a.offset,
        max_chars: a.max_chars,
    };
    page.validate()?;
    let mut file = compound(bytes)?;
    let data = stream(&mut file, "/WordDocument")?;
    if u16_at(&data, 0)? != 0xa5ec || u16_at(&data, 2)? < 0x00c1 {
        return Err(
            "This Word version predates Word 97. Save it as .docx before attaching.".into(),
        );
    }
    let flags = u16_at(&data, 10)?;
    if flags & 0x8100 != 0 {
        return Err("Password-protected/obfuscated Word files are not readable. Decrypt the file before attaching.".into());
    }
    let table = stream(
        &mut file,
        if flags & 0x0200 != 0 {
            "/1Table"
        } else {
            "/0Table"
        },
    )?;
    let sw = u16_at(&data, 32)? as usize;
    let lw_count_at = 34 + sw * 2;
    let lw = u16_at(&data, lw_count_at)? as usize;
    if lw < 11 {
        return Err("Missing Word character counts.".into());
    }
    let lw_start = lw_count_at + 2;
    let lengths = (3..11)
        .map(|i| u32_at(&data, lw_start + i * 4).map(|v| v as usize))
        .collect::<Result<Vec<_>, _>>()?;
    let fc_count_at = lw_start + lw * 4;
    if u16_at(&data, fc_count_at)? <= 33 {
        return Err("Missing Word piece-table pointer.".into());
    }
    let fc_start = fc_count_at + 2;
    let clx_start = u32_at(&data, fc_start + 33 * 8)? as usize;
    let clx_len = u32_at(&data, fc_start + 33 * 8 + 4)? as usize;
    let clx = bytes_at(&table, clx_start, clx_len)?;
    let mut pos = 0;
    while clx.get(pos) == Some(&1) {
        let len = u16_at(clx, pos + 1)? as usize;
        pos = pos
            .checked_add(3 + len)
            .ok_or("Invalid Word property run.")?;
    }
    if clx.get(pos) != Some(&2) {
        return Err("Missing Word piece table.".into());
    }
    let length = u32_at(clx, pos + 1)? as usize;
    let plc = bytes_at(clx, pos + 5, length)?;
    if length < 4 || (length - 4) % 12 != 0 {
        return Err("Invalid Word piece table.".into());
    }
    let count = (length - 4) / 12;
    if count > 1_000_000 {
        return Err("Word document contains too many text pieces.".into());
    }
    let total = lengths
        .iter()
        .try_fold(0usize, |a, n| a.checked_add(*n))
        .ok_or("Word text exceeds the reading limit.")?;
    if total > 32 * 1024 * 1024 {
        return Err("Word text exceeds the reading limit.".into());
    }
    let section_index = match a.section.as_str() {
        "body" => 0,
        "footnotes" => 1,
        "headers" => 2,
        "comments" => 4,
        "endnotes" => 5,
        _ => return Err("Unknown Word section.".into()),
    };
    let start = lengths[..section_index].iter().sum::<usize>();
    let end = start + lengths[section_index];
    let mut spans = vec![(start, end)];
    if a.section == "body" && lengths[6] > 0 {
        let s = lengths[..6].iter().sum();
        spans.push((s, s + lengths[6]));
    }
    let mut raw = String::new();
    for (span_index, (start, end)) in spans.into_iter().enumerate() {
        if span_index > 0 {
            raw.push_str("\r[Text boxes]\r");
        }
        let mut covered = 0usize;
        for i in 0..count {
            let cp = u32_at(plc, i * 4)? as usize;
            let next = u32_at(plc, (i + 1) * 4)? as usize;
            if next < cp {
                return Err("Word piece-table positions are out of order.".into());
            }
            let lo = start.max(cp);
            let hi = end.min(next);
            if hi <= lo {
                continue;
            }
            covered = covered
                .checked_add(hi - lo)
                .ok_or("Word text is too large.")?;
            let fc = u32_at(plc, (count + 1) * 4 + i * 8 + 2)?;
            let compressed = fc & 0x4000_0000 != 0;
            let fc = (fc & 0x3fff_ffff) as usize;
            let text = if compressed {
                let bytes = bytes_at(&data, fc / 2 + lo - cp, hi - lo)?;
                encoding_rs::WINDOWS_1252
                    .decode_without_bom_handling(bytes)
                    .0
                    .into_owned()
            } else {
                utf16(bytes_at(&data, fc + (lo - cp) * 2, (hi - lo) * 2)?)?
            };
            raw.push_str(&text);
        }
        if covered != end - start {
            return Err("Word piece table does not cover the requested text section.".into());
        }
    }
    let clean = word_text(&raw);
    let paragraphs = clean.split('\n').collect::<Vec<_>>();
    let text = paragraphs
        .iter()
        .enumerate()
        .map(|(i, text)| format!("[Paragraph {}] {text}\n", i + 1))
        .collect::<String>();
    let mut value = if page.view == "overview" {
        json!({"format":"doc","paragraph_count":paragraphs.len(),"available_sections":["body","footnotes","headers","comments","endnotes"],"note":SOURCE_NOTE})
    } else {
        text_result(&text, &page)
    };
    value["format"] = json!("doc");
    value["section"] = json!(a.section);
    value["paragraph_count"] = json!(paragraphs.len());
    value["legacy_note"]=json!("Text follows the current Word piece table. Tabs preserve table cell boundaries; full formatting, revision visibility and exact table geometry are not reconstructed. Field instructions are excluded; field results are read as saved.");
    Ok(value)
}
fn word_text(text: &str) -> String {
    let mut result = String::new();
    let mut fields = Vec::<bool>::new();
    for ch in text.chars() {
        match ch {
            '\u{13}' => fields.push(false),
            '\u{14}' => {
                if let Some(field) = fields.last_mut() {
                    *field = true;
                }
            }
            '\u{15}' => {
                fields.pop();
            }
            _ => {
                if fields.iter().any(|shown| !*shown) {
                    continue;
                }
                match ch {
                    '\r' | '\u{b}' => result.push('\n'),
                    '\u{7}' => result.push('\t'),
                    '\u{c}' => result.push_str("\n[Page break]\n"),
                    c if c.is_control() && c != '\t' && c != '\n' => {}
                    _ => result.push(ch),
                }
            }
        }
    }
    result
}

#[derive(Clone, Copy)]
struct Record<'a> {
    kind: u16,
    instance: u16,
    container: bool,
    payload: &'a [u8],
    next: usize,
}
fn record(data: &[u8], at: usize) -> Result<Record<'_>, String> {
    let flags = u16_at(data, at)?;
    let kind = u16_at(data, at + 2)?;
    let len = u32_at(data, at + 4)? as usize;
    let payload = bytes_at(data, at + 8, len)?;
    Ok(Record {
        kind,
        instance: flags >> 4,
        container: flags & 15 == 15,
        payload,
        next: at + 8 + len,
    })
}
fn children(data: &[u8]) -> Result<Vec<Record<'_>>, String> {
    let mut records = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let rec = record(data, at)?;
        at = rec.next;
        records.push(rec);
        if records.len() > 100_000 {
            return Err("Too many PowerPoint records.".into());
        }
    }
    Ok(records)
}
fn collect_text(
    rec: Record<'_>,
    depth: usize,
    count: &mut usize,
    text: &mut String,
) -> Result<(), String> {
    *count += 1;
    if *count > 100_000 || depth > 64 {
        return Err("PowerPoint record nesting exceeds the reading limit.".into());
    }
    if rec.kind == 4000 {
        text.push_str(&utf16(rec.payload)?.replace(['\r', '\u{b}'], "\n"));
        text.push('\n');
    } else if rec.kind == 4008 {
        for &b in rec.payload {
            text.push(if b == 13 || b == 11 {
                '\n'
            } else {
                char::from(b)
            });
        }
        text.push('\n');
    } else if rec.container || rec.kind == 0xf00d {
        for child in children(rec.payload)? {
            collect_text(child, depth + 1, count, text)?;
        }
    }
    if text.len() > MAX_FILE {
        return Err("PowerPoint text exceeds the reading limit.".into());
    }
    Ok(())
}
struct Slide {
    persist: u32,
    id: u32,
    text: String,
}
fn list_slides(list: Record<'_>) -> Result<Vec<Slide>, String> {
    let mut slides = Vec::<Slide>::new();
    for rec in children(list.payload)? {
        if rec.kind == 1011 {
            slides.push(Slide {
                persist: u32_at(rec.payload, 0)?,
                id: u32_at(rec.payload, 12)?,
                text: String::new(),
            });
        } else if let Some(slide) = slides.last_mut() {
            let mut count = 0;
            collect_text(rec, 0, &mut count, &mut slide.text)?;
        }
    }
    Ok(slides)
}
fn powerpoint(bytes: &[u8], args: &Value) -> Result<Value, String> {
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
        .map_err(|e| format!("Invalid PowerPoint arguments: {e}"))?;
    let page = PageArgs {
        _attachment_id: a.attachment_id,
        view: a.view,
        offset: a.offset,
        max_chars: a.max_chars,
    };
    page.validate()?;
    let mut file = compound(bytes)?;
    let current = stream(&mut file, "/Current User")?;
    let data = stream(&mut file, "/PowerPoint Document")?;
    let current = record(&current, 0)?;
    if current.kind != 4086 || u32_at(current.payload, 4)? != 0xe391_c05f {
        return Err("Encrypted or invalid legacy PowerPoint file.".into());
    }
    let mut edit = u32_at(current.payload, 8)? as usize;
    let mut directory = HashMap::new();
    let mut visited = HashSet::new();
    let mut document_id = None;
    loop {
        if !visited.insert(edit) || visited.len() > 4096 {
            return Err("Invalid PowerPoint save-history chain.".into());
        }
        let atom = record(&data, edit)?;
        if atom.kind != 4085 {
            return Err("Missing PowerPoint current save record.".into());
        }
        if atom.payload.len() >= 32 && u32_at(atom.payload, 28)? != 0 {
            return Err("Password-protected PowerPoint file.".into());
        }
        document_id.get_or_insert(u32_at(atom.payload, 16)?);
        let block = record(&data, u32_at(atom.payload, 12)? as usize)?;
        if block.kind != 6002 {
            return Err("Invalid PowerPoint persist directory.".into());
        }
        let mut at = 0;
        while at < block.payload.len() {
            let packed = u32_at(block.payload, at)?;
            at += 4;
            let start = packed & 0x000f_ffff;
            let count = packed >> 20;
            if count == 0 {
                return Err("Empty PowerPoint persist run.".into());
            }
            for index in 0..count {
                let offset = u32_at(block.payload, at)? as usize;
                at += 4;
                if offset >= data.len() {
                    return Err("PowerPoint object offset leaves its stream.".into());
                }
                directory.entry(start + index).or_insert(offset);
            }
            if directory.len() > 100_000 {
                return Err("PowerPoint has too many objects.".into());
            }
        }
        let previous = u32_at(atom.payload, 8)? as usize;
        if previous == 0 {
            break;
        }
        if previous >= edit {
            return Err("Invalid PowerPoint save-history ordering.".into());
        }
        edit = previous;
    }
    let object = |id: u32| -> Result<Record<'_>, String> {
        record(
            &data,
            *directory
                .get(&id)
                .ok_or("Missing live PowerPoint object.")?,
        )
    };
    let document = object(document_id.ok_or("Missing PowerPoint document reference.")?)?;
    if document.kind != 1000 {
        return Err("Invalid PowerPoint document container.".into());
    }
    let lists = children(document.payload)?;
    let slides = lists
        .iter()
        .find(|r| r.kind == 4080 && r.instance == 0)
        .map(|r| list_slides(*r))
        .transpose()?
        .unwrap_or_default();
    let mut notes = HashMap::new();
    if a.include_notes {
        if let Some(list) = lists.iter().find(|r| r.kind == 4080 && r.instance == 2) {
            for note in list_slides(*list)? {
                let container = object(note.persist)?;
                if container.kind != 1008 {
                    return Err("Invalid PowerPoint notes container.".into());
                }
                let note_atom = children(container.payload)?
                    .into_iter()
                    .find(|r| r.kind == 1009)
                    .ok_or("Missing PowerPoint notes association.")?;
                let slide_id = u32_at(note_atom.payload, 0)?;
                let mut text = String::new();
                let mut count = 0;
                collect_text(container, 0, &mut count, &mut text)?;
                notes.insert(slide_id, text);
            }
        }
    }
    let selected = match a.slides {
        Some(slides_requested) => {
            if slides_requested.is_empty()
                || slides_requested.len() > 20
                || slides_requested
                    .iter()
                    .any(|n| *n == 0 || *n > slides.len())
            {
                return Err(format!(
                    "Choose 1–20 slide indices, from 1 to {}.",
                    slides.len()
                ));
            }
            slides_requested
        }
        None => {
            if page.view == "overview" {
                (1..=slides.len()).skip(page.offset).take(100).collect()
            } else {
                (1..=slides.len()).collect()
            }
        }
    };
    let mut overview = Vec::new();
    let mut text = String::new();
    let mut count = 0;
    for index in selected {
        let slide = &slides[index - 1];
        let container = object(slide.persist)?;
        if container.kind != 1006 {
            return Err("Invalid PowerPoint slide container.".into());
        }
        let mut content = slide.text.clone();
        collect_text(container, 0, &mut count, &mut content)?;
        let title = content
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default();
        overview.push(json!({"slide":index,"title":title,"slide_id":slide.id}));
        text.push_str(&format!("\n[Slide {index}]\n{content}"));
        if let Some(note) = notes.get(&slide.id) {
            text.push_str("[Speaker notes]\n");
            text.push_str(note);
        }
    }
    let mut value = if page.view == "overview" {
        json!({"slides":overview,"offset":page.offset,"next_offset":if page.offset.saturating_add(100)<slides.len(){Some(page.offset.saturating_add(100))}else{None},"note":SOURCE_NOTE})
    } else {
        text_result(&text, &page)
    };
    value["format"] = json!("ppt");
    value["total_slides"] = json!(slides.len());
    value["legacy_note"]=json!("Text follows live persist objects in presentation order, with notes associated by slide id. Full layout, images, legacy embedded chart objects and animations are not rendered.");
    Ok(value)
}

/// Calamine's XLS reader builds dense ranges. Check both DIMENSIONS and actual
/// BIFF cell records before letting a hostile sparse grid trigger allocation.
pub(super) fn validate_xls_size(bytes: &[u8]) -> Result<(), String> {
    let mut file = compound(bytes)?;
    let name = if file.is_stream("/Workbook") {
        "/Workbook"
    } else {
        "/Book"
    };
    let data = stream(&mut file, name)?;
    let mut at = 0;
    let (mut max_row, mut max_col) = (0u64, 0u64);
    let mut total = 0u64;
    let mut record_count = 0;
    while at < data.len() {
        record_count += 1;
        if record_count > 1_000_000 {
            return Err("Legacy Excel has too many BIFF records.".into());
        }
        let kind = u16_at(&data, at)?;
        let len = u16_at(&data, at + 2)? as usize;
        let payload = bytes_at(&data, at + 4, len)?;
        at += 4 + len;
        if kind == 0x00fc && (len < 8 || u32_at(payload, 4)? > 1_000_000) {
            return Err("Legacy Excel shared-string count exceeds the reading limit.".into());
        }
        if kind == 0x002f {
            return Err("Password-protected Excel workbook. Decrypt it before attaching.".into());
        }
        if matches!(kind, 0x0809 | 0x0409 | 0x0209 | 0x0009) {
            total = total
                .checked_add(max_row.saturating_mul(max_col))
                .ok_or("Excel grid is too large.")?;
            max_row = 0;
            max_col = 0;
        }
        if kind == 0x0200 && len >= 14 {
            max_row = max_row.max(u32_at(payload, 4)? as u64);
            max_col = max_col.max(u16_at(payload, 10)? as u64);
        }
        if matches!(
            kind,
            0x00fd | 0x027e | 0x00bd | 0x00be | 0x0203 | 0x0006 | 0x0204 | 0x0205 | 0x0201
        ) && len >= 4
        {
            max_row = max_row.max(u16_at(payload, 0)? as u64 + 1);
            max_col = max_col.max(u16_at(payload, 2)? as u64 + 1);
            if matches!(kind, 0x00bd | 0x00be) {
                max_col = max_col.max(u16_at(payload, len - 2)? as u64 + 1);
            }
        }
        if max_row > 65536
            || max_col > 256
            || total.saturating_add(max_row.saturating_mul(max_col)) > 2_000_000
        {
            return Err("Legacy Excel workbook exceeds the local 2,000,000-cell allocation limit. Split the workbook or save as .xlsx.".into());
        }
    }
    Ok(())
}
