use super::*;
use calamine::{Data, Reader};
use std::collections::HashMap;

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
    sheet: Option<String>,
    range: Option<String>,
    #[serde(default = "one")]
    start_row: u32,
    #[serde(default = "one")]
    start_column: u32,
    #[serde(default = "rows")]
    max_rows: u32,
    #[serde(default = "columns")]
    max_columns: u32,
}
fn one() -> u32 {
    1
}
fn rows() -> u32 {
    50
}
fn columns() -> u32 {
    20
}
impl Args {
    fn parse(args: &Value) -> Result<Self, String> {
        let a: Self = serde_json::from_value(args.clone())
            .map_err(|e| format!("Invalid Excel reading arguments: {e}"))?;
        a.page().validate()?;
        if a.start_row == 0
            || a.start_row > 1_048_576
            || a.start_column == 0
            || a.start_column > 16_384
            || a.max_rows == 0
            || a.max_rows > 200
            || a.max_columns == 0
            || a.max_columns > 50
        {
            return Err("Use 1-based rows/columns, up to 200 rows and 50 columns per grid.".into());
        }
        Ok(a)
    }
    fn page(&self) -> PageArgs {
        PageArgs {
            _attachment_id: self.attachment_id.clone(),
            view: self.view.clone(),
            offset: self.offset,
            max_chars: self.max_chars,
        }
    }
    fn bounds(&self) -> Result<(u32, u32, u32, u32), String> {
        let bounds = if let Some(range) = &self.range {
            let (start, end) = range.split_once(':').unwrap_or((range, range));
            let (r, c) = address(start)?;
            let (er, ec) = address(end)?;
            (r, c, er, ec)
        } else {
            (
                self.start_row,
                self.start_column,
                self.start_row
                    .saturating_add(self.max_rows - 1)
                    .min(1_048_576),
                self.start_column
                    .saturating_add(self.max_columns - 1)
                    .min(16_384),
            )
        };
        let (r, c, er, ec) = bounds;
        if er < r || ec < c || er - r >= 200 || ec - c >= 50 {
            return Err(
                "Select an A1 range of at most 200 rows × 50 columns, in increasing order.".into(),
            );
        }
        Ok(bounds)
    }
}
fn address(text: &str) -> Result<(u32, u32), String> {
    let text = text.replace('$', "").to_ascii_uppercase();
    let split = text
        .find(|c: char| c.is_ascii_digit())
        .ok_or("Invalid A1 cell address.")?;
    let mut col = 0u32;
    for b in text[..split].bytes() {
        if !b.is_ascii_uppercase() {
            return Err("Invalid A1 column.".into());
        }
        col = col
            .checked_mul(26)
            .and_then(|v| v.checked_add((b - b'A' + 1) as u32))
            .ok_or("Invalid A1 column.")?;
    }
    let row = text[split..]
        .parse::<u32>()
        .map_err(|_| "Invalid A1 row.")?;
    if !(1..=1_048_576).contains(&row) || !(1..=16_384).contains(&col) {
        return Err("A1 address exceeds Excel's grid.".into());
    }
    Ok((row, col))
}
fn cell_name(row: u32, mut col: u32) -> String {
    let mut letters = Vec::new();
    while col > 0 {
        col -= 1;
        letters.push((b'A' + (col % 26) as u8) as char);
        col /= 26;
    }
    format!("{}{row}", letters.into_iter().rev().collect::<String>())
}
fn grid_result(
    a: &Args,
    format: Format,
    sheet: &str,
    dimensions: (u32, u32),
    text: &str,
) -> Result<Value, String> {
    let (r, c, er, ec) = a.bounds()?;
    let mut value = text_result(text, &a.page());
    value["format"] = json!(format.extension());
    value["sheet"] = json!(sheet);
    value["range"] = json!(format!("{}:{}", cell_name(r, c), cell_name(er, ec)));
    value["dimensions"] = json!({"rows":dimensions.0,"columns":dimensions.1});
    value["next_row"] = json!(if er < dimensions.0 {
        Some(er + 1)
    } else {
        None
    });
    value["next_column"] = json!(if ec < dimensions.1 {
        Some(ec + 1)
    } else {
        None
    });
    value["formula_note"]=json!("Formulas have NOT been evaluated. Values on formula cells are the saved cache, which may be missing or stale. External data and macros are never run.");
    Ok(value)
}
struct Sheet {
    name: String,
    path: String,
    state: String,
}
pub(super) fn read_xlsx(mut package: Package, args: &Value) -> Result<Value, String> {
    let a = Args::parse(args)?;
    let main = package.main_part(Format::Xlsx)?;
    let xml = package.xml(&main)?;
    let workbook = parse_xml(&xml)?;
    let date1904 = workbook
        .descendants()
        .find(|n| tag(*n, "workbookPr"))
        .and_then(|n| n.attribute("date1904"))
        .is_some_and(|v| v == "1" || v == "true");
    let relationships = package.relationships(&main)?;
    let mut sheets = Vec::new();
    for node in workbook.descendants().filter(|n| tag(*n, "sheet")) {
        let id = node
            .attributes()
            .find(|a| a.name() == "id" && a.namespace().is_some())
            .map(|a| a.value())
            .ok_or("Missing sheet relationship.")?;
        let Some(rel) = relationships.iter().find(|r| r.id == id) else {
            return Err("Missing or external sheet relationship.".into());
        };
        // Chartsheets are listed explicitly, not misread as worksheet data.
        sheets.push(Sheet {
            name: node.attribute("name").unwrap_or_default().into(),
            path: rel.target.clone(),
            state: node.attribute("state").unwrap_or("visible").into(),
        });
    }
    if a.view == "overview" {
        let mut overview = Vec::new();
        for sheet in sheets.iter().skip(a.offset).take(100) {
            let xml = package.xml(&sheet.path)?;
            let doc = parse_xml(&xml)?;
            let dims = dimensions(&doc)?;
            overview.push(json!({"name":sheet.name,"state":sheet.state,"rows":dims.0,"columns":dims.1,"kind":doc.root_element().tag_name().name()}));
        }
        return Ok(
            json!({"format":"xlsx","sheets":overview,"total_sheets":sheets.len(),"offset":a.offset,"next_offset":if a.offset.saturating_add(100)<sheets.len(){Some(a.offset.saturating_add(100))}else{None},"date_system":if date1904{"1904"}else{"1900"},"note":SOURCE_NOTE}),
        );
    }
    let sheet = match &a.sheet {
        Some(name) => sheets.iter().find(|s| s.name == *name),
        None => sheets.first(),
    }
    .ok_or("Worksheet not found. Use view=overview for exact names.")?;
    let strings = if let Some(rel) = relationships
        .iter()
        .find(|r| r.kind.ends_with("/sharedStrings"))
    {
        let xml = package.xml(&rel.target)?;
        let doc = parse_xml(&xml)?;
        doc.root_element()
            .children()
            .filter(|n| tag(*n, "si"))
            .map(rich_text)
            .collect::<Vec<_>>()
    } else {
        vec![]
    };
    let mut formats = HashMap::new();
    let mut xfs = Vec::new();
    if let Some(rel) = relationships.iter().find(|r| r.kind.ends_with("/styles")) {
        let xml = package.xml(&rel.target)?;
        let doc = parse_xml(&xml)?;
        for node in doc.descendants().filter(|n| tag(*n, "numFmt")) {
            if let (Some(id), Some(code)) =
                (node.attribute("numFmtId"), node.attribute("formatCode"))
            {
                formats.insert(id.to_string(), code.to_string());
            }
        }
        if let Some(parent) = doc.descendants().find(|n| tag(*n, "cellXfs")) {
            for xf in parent.children().filter(|n| tag(*n, "xf")) {
                xfs.push(xf.attribute("numFmtId").unwrap_or("0").to_string());
            }
        }
    }
    let xml = package.xml(&sheet.path)?;
    let doc = parse_xml(&xml)?;
    if !tag(doc.root_element(), "worksheet") {
        return Err("This sheet has no cell grid (for example, a chart sheet).".into());
    }
    let dims = dimensions(&doc)?;
    let (r, c, er, ec) = a.bounds()?;
    let mut text = String::new();
    let mut row_cursor = 0;
    for row in doc.descendants().filter(|n| tag(*n, "row")) {
        row_cursor = row
            .attribute("r")
            .map(str::parse)
            .transpose()
            .map_err(|_| "Invalid worksheet row.")?
            .unwrap_or(row_cursor + 1);
        let mut column = 0;
        for cell in row.children().filter(|n| tag(*n, "c")) {
            let (cr, cc) = if let Some(name) = cell.attribute("r") {
                address(name)?
            } else {
                (row_cursor, column + 1)
            };
            column = cc;
            if cr < r || cr > er || cc < c || cc > ec {
                continue;
            }
            let mut value =
                json!({"cell":cell_name(cr,cc),"type":cell.attribute("t").unwrap_or("n")});
            let raw = cell
                .children()
                .find(|n| tag(*n, "v"))
                .and_then(|n| n.text());
            value["value"] = match cell.attribute("t") {
                Some("s") => {
                    let index = raw
                        .ok_or("Missing shared string index.")?
                        .parse::<usize>()
                        .map_err(|_| "Invalid shared string index.")?;
                    json!(strings
                        .get(index)
                        .ok_or("Shared string index is out of bounds.")?)
                }
                Some("inlineStr") => json!(cell
                    .children()
                    .find(|n| tag(*n, "is"))
                    .map(rich_text)
                    .unwrap_or_default()),
                Some("b") => match raw {
                    Some("1") => json!(true),
                    Some("0") => json!(false),
                    None => Value::Null,
                    _ => return Err("Invalid boolean cell.".into()),
                },
                _ => raw.map_or(Value::Null, |v| json!(v)),
            };
            if let Some(formula) = cell.children().find(|n| tag(*n, "f")) {
                value["formula"] = json!(formula.text());
                value["cached_result"] = json!(true);
                for key in ["t", "si", "ref"] {
                    if let Some(v) = formula.attribute(key) {
                        value[format!("formula_{key}")] = json!(v);
                    }
                }
            }
            if let Some(style) = cell.attribute("s") {
                let index = style.parse::<usize>().map_err(|_| "Invalid cell style.")?;
                let id = xfs.get(index).ok_or("Missing cell number format.")?;
                value["number_format_id"] = json!(id);
                value["number_format"] = json!(formats
                    .get(id)
                    .map(String::as_str)
                    .or_else(|| builtin_format(id)));
            }
            text.push_str(&value.to_string());
            text.push('\n');
        }
    }
    let mut result = grid_result(&a, Format::Xlsx, &sheet.name, dims, &text)?;
    result["date_system"] = json!(if date1904 { "1904" } else { "1900" });
    result["merged_ranges"] = json!(doc
        .descendants()
        .filter(|n| tag(*n, "mergeCell"))
        .filter_map(|n| n.attribute("ref"))
        .take(500)
        .collect::<Vec<_>>());
    Ok(result)
}
fn dimensions(doc: &Document<'_>) -> Result<(u32, u32), String> {
    let (mut rows, mut cols) = (0, 0);
    let mut row_cursor = 0u32;
    for row in doc.descendants().filter(|n| tag(*n, "row")) {
        row_cursor = row
            .attribute("r")
            .map(str::parse)
            .transpose()
            .map_err(|_| "Invalid worksheet row.")?
            .unwrap_or(row_cursor + 1);
        let mut column = 0u32;
        for cell in row.children().filter(|n| tag(*n, "c")) {
            let (r, c) = if let Some(name) = cell.attribute("r") {
                address(name)?
            } else {
                (row_cursor, column + 1)
            };
            if r == 0 || r > 1_048_576 || c == 0 || c > 16_384 {
                return Err("Cell exceeds Excel's grid.".into());
            }
            column = c;
            rows = rows.max(r);
            cols = cols.max(c);
        }
    }
    Ok((rows, cols))
}
fn builtin_format(id: &str) -> Option<&'static str> {
    match id {
        "0" => Some("General"),
        "1" => Some("0"),
        "2" => Some("0.00"),
        "9" => Some("0%"),
        "10" => Some("0.00%"),
        "14" => Some("mm-dd-yy"),
        "15" => Some("d-mmm-yy"),
        "16" => Some("d-mmm"),
        "17" => Some("mmm-yy"),
        "18" => Some("h:mm AM/PM"),
        "19" => Some("h:mm:ss AM/PM"),
        "20" => Some("h:mm"),
        "21" => Some("h:mm:ss"),
        "22" => Some("m/d/yy h:mm"),
        "49" => Some("@"),
        _ => None,
    }
}

pub(super) fn read_delimited(bytes: &[u8], format: Format, args: &Value) -> Result<Value, String> {
    let a = Args::parse(args)?;
    if a.sheet.as_deref().is_some_and(|s| s != "Sheet1") {
        return Err("CSV/TSV has one sheet named Sheet1.".into());
    }
    let (text, encoding) = decode_text(bytes, true)?;
    let rows = parse_delimited(&text, if format == Format::Tsv { '\t' } else { ',' })?;
    let dims = (
        rows.len() as u32,
        rows.iter().map(Vec::len).max().unwrap_or(0) as u32,
    );
    if a.view == "overview" {
        return Ok(
            json!({"format":format.extension(),"sheets":[{"name":"Sheet1","rows":dims.0,"columns":dims.1,"state":"visible"}],"total_sheets":1,"encoding":encoding,"note":SOURCE_NOTE}),
        );
    }
    let (r, c, er, ec) = a.bounds()?;
    let mut output = String::new();
    for (row_index, row) in rows
        .iter()
        .enumerate()
        .skip((r - 1) as usize)
        .take((er - r + 1) as usize)
    {
        for (col_index, value) in row
            .iter()
            .enumerate()
            .skip((c - 1) as usize)
            .take((ec - c + 1) as usize)
        {
            output.push_str(&json!({"cell":cell_name(row_index as u32+1,col_index as u32+1),"type":"string","value":value}).to_string());
            output.push('\n');
        }
    }
    let mut result = grid_result(&a, format, "Sheet1", dims, &output)?;
    result["encoding"] = json!(encoding);
    Ok(result)
}
fn parse_delimited(text: &str, delimiter: char) -> Result<Vec<Vec<String>>, String> {
    let mut result = Vec::new();
    let mut row = Vec::new();
    let mut value = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut closed = false;
    let mut count = 0;
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    value.push('"');
                } else {
                    quoted = false;
                    closed = true;
                }
            } else {
                value.push(ch);
            }
            continue;
        }
        if ch == delimiter {
            row.push(std::mem::take(&mut value));
            count += 1;
            closed = false;
        } else if ch == '\r' || ch == '\n' {
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut value));
            count += 1;
            result.push(std::mem::take(&mut row));
            closed = false;
        } else if ch == '"' && value.is_empty() && !closed {
            quoted = true;
        } else {
            if closed {
                return Err("Unexpected text after a CSV quoted field.".into());
            }
            value.push(ch);
        }
        if count > 1_000_000 || row.len() > 16384 || result.len() > 1_048_576 {
            return Err("CSV/TSV exceeds the local reading grid limit.".into());
        }
    }
    if quoted {
        return Err("Unterminated CSV quoted field.".into());
    }
    if !value.is_empty() || !row.is_empty() || closed {
        row.push(value);
        result.push(row);
    }
    Ok(result)
}

pub(super) fn read_xls(bytes: &[u8], args: &Value) -> Result<Value, String> {
    let a = Args::parse(args)?;
    super::legacy::validate_xls_size(bytes)?;
    let mut book: calamine::Xls<_> = calamine::open_workbook_from_rs(Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("Read legacy Excel workbook: {e}"))?;
    let metadata = book.sheets_metadata().to_vec();
    if a.view == "overview" {
        let mut sheets = Vec::new();
        for sheet in metadata.iter().skip(a.offset).take(100) {
            let range = book
                .worksheet_range(&sheet.name)
                .map_err(|e| format!("Read Excel sheet: {e}"))?;
            let dims = range.end().map_or((0, 0), |(r, c)| (r + 1, c + 1));
            sheets.push(json!({"name":sheet.name,"state":format!("{:?}",sheet.visible),"rows":dims.0,"columns":dims.1}));
        }
        return Ok(
            json!({"format":"xls","sheets":sheets,"total_sheets":metadata.len(),"offset":a.offset,"next_offset":if a.offset.saturating_add(100)<metadata.len(){Some(a.offset.saturating_add(100))}else{None},"note":SOURCE_NOTE}),
        );
    }
    let sheet = match &a.sheet {
        Some(name) => metadata.iter().find(|s| s.name == *name),
        None => metadata.first(),
    }
    .ok_or("Worksheet not found. Use view=overview for exact names.")?;
    let range = book
        .worksheet_range(&sheet.name)
        .map_err(|e| format!("Read Excel cells: {e}"))?;
    let formula = book
        .worksheet_formula(&sheet.name)
        .map_err(|e| format!("Read Excel formulas: {e}"))?;
    let dims = range.end().map_or((0, 0), |(r, c)| (r + 1, c + 1));
    let (r, c, er, ec) = a.bounds()?;
    let mut text = String::new();
    for row in r..=er {
        for col in c..=ec {
            let position = (row - 1, col - 1);
            let value = range.get_value(position).unwrap_or(&Data::Empty);
            let formula = formula.get_value(position).filter(|s| !s.is_empty());
            if *value == Data::Empty && formula.is_none() {
                continue;
            }
            let (mut kind, mut raw) = ("empty", Value::Null);
            let mut extra = None;
            match value {
                Data::Int(v) => {
                    kind = "number";
                    raw = json!(v)
                }
                Data::Float(v) => {
                    kind = "number";
                    raw = json!(v)
                }
                Data::String(v) => {
                    kind = "string";
                    raw = json!(v)
                }
                Data::Bool(v) => {
                    kind = "boolean";
                    raw = json!(v)
                }
                Data::Error(v) => {
                    kind = "error";
                    raw = json!(v.to_string())
                }
                Data::DateTime(v) => {
                    kind = "datetime";
                    raw = json!(v.as_f64());
                    extra = v.as_datetime().map(|dt| dt.to_string())
                }
                Data::DateTimeIso(v) | Data::DurationIso(v) => {
                    kind = "datetime_iso";
                    raw = json!(v)
                }
                Data::Empty => {}
            }
            let mut record = json!({"cell":cell_name(row,col),"type":kind,"value":raw});
            if let Some(formula) = formula {
                record["formula"] = json!(formula);
                record["cached_result"] = json!(true);
            }
            if let Some(iso) = extra {
                record["datetime"] = json!(iso);
            }
            text.push_str(&record.to_string());
            text.push('\n');
        }
    }
    let mut result = grid_result(&a, Format::Xls, &sheet.name, dims, &text)?;
    result["formatting_note"]=json!("Legacy XLS values, dates and formulas are read; full number-format/layout metadata is unavailable.");
    Ok(result)
}
