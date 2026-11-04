use super::*;
use base64::Engine;
use std::io::Write;

fn zip(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in parts {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn rel(id: &str, kind: &str, target: &str) -> String {
    format!(
        r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
    )
}
fn rels(content: &str) -> String {
    format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{content}</Relationships>"#
    )
}
fn word_xml(content: &str) -> String {
    format!(
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{content}</w:body></w:document>"#
    )
}
fn docx() -> Vec<u8> {
    zip(&[
        ("[Content_Types].xml", "<Types/>"),
        (
            "word/document.xml",
            &word_xml(
                r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>背景与 task</w:t></w:r></w:p><w:p><w:r><w:t>正文🙂</w:t><w:tab/><w:t>after tab</w:t></w:r><w:del><w:r><w:delText>Deleted secret</w:delText></w:r></w:del><w:r><w:instrText>dangerous field instruction</w:instrText></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>名称</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>数值 42</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#,
            ),
        ),
        (
            "word/_rels/document.xml.rels",
            &rels(&rel("f", "footnotes", "footnotes.xml")),
        ),
        (
            "word/footnotes.xml",
            r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:footnote w:id="1"><w:p><w:r><w:t>脚注 evidence</w:t></w:r></w:p></w:footnote></w:footnotes>"#,
        ),
    ])
}
fn slide_xml(title: &str, body: &str) -> String {
    format!(
        r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:txBody><a:p><a:r><a:t>{body}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#
    )
}
fn pptx() -> Vec<u8> {
    zip(&[
        ("[Content_Types].xml", "<Types/>"),
        (
            "ppt/presentation.xml",
            r#"<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="b"/><p:sldId id="257" r:id="a"/></p:sldIdLst></p:presentation>"#,
        ),
        (
            "ppt/_rels/presentation.xml.rels",
            &rels(
                &(rel("a", "slide", "slides/slide1.xml") + &rel("b", "slide", "slides/slide9.xml")),
            ),
        ),
        (
            "ppt/slides/slide9.xml",
            &slide_xml("Actually first", "第一张"),
        ),
        (
            "ppt/slides/slide1.xml",
            &slide_xml("Actually second", "second slide evidence"),
        ),
        (
            "ppt/slides/_rels/slide9.xml.rels",
            &rels(&rel(
                "notes",
                "notesSlide",
                "../notesSlides/notesSlide7.xml",
            )),
        ),
        (
            "ppt/notesSlides/notesSlide7.xml",
            r#"<p:notes xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:sp><p:txBody><a:p><a:r><a:t>Speaker explanation for FIRST</a:t></a:r></a:p></p:txBody></p:sp></p:notes>"#,
        ),
    ])
}
fn xlsx() -> Vec<u8> {
    zip(&[("[Content_Types].xml","<Types/>"),("xl/workbook.xml",r#"<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><workbookPr date1904="1"/><sheets><sheet name="实验 Data" sheetId="1" r:id="s1"/><sheet name="Hidden" state="hidden" sheetId="2" r:id="s2"/></sheets></workbook>"#),
        ("xl/_rels/workbook.xml.rels",&rels(&(rel("s1","worksheet","worksheets/actual.xml")+&rel("s2","worksheet","worksheets/hidden.xml")+&rel("ss","sharedStrings","sharedStrings.xml")+&rel("st","styles","styles.xml")))),
        ("xl/sharedStrings.xml",r#"<sst><si><r><t>精确</t></r><r><t>标题</t></r><rPh><t>phonetic must be excluded</t></rPh></si></sst>"#),
        ("xl/styles.xml",r#"<styleSheet><numFmts><numFmt numFmtId="170" formatCode="yyyy-mm-dd"/></numFmts><cellXfs><xf numFmtId="0"/><xf numFmtId="170"/></cellXfs></styleSheet>"#),
        ("xl/worksheets/actual.xml",r#"<worksheet><sheetData><row r="2"><c r="B2" t="s"><v>0</v></c><c r="C2" t="inlineStr"><is><t>line 1&#10;line 2</t></is></c><c r="D2" t="b"><v>1</v></c></row><row r="3"><c r="B3"><v>42</v></c><c r="C3"><f>B3*2</f><v>84</v></c><c r="D3"><f>NOW()</f></c><c r="E3" s="1"><v>1</v></c></row></sheetData><mergeCells><mergeCell ref="B2:B3"/></mergeCells></worksheet>"#),
        ("xl/worksheets/hidden.xml","<worksheet><sheetData><row r=\"1\"><c r=\"A1\" t=\"inlineStr\"><is><t>Hidden evidence</t></is></c></row></sheetData></worksheet>")])
}
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("argus-office-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn read(&self, bytes: &[u8], format: Format, args: Value) -> Value {
        let path = self.0.join(format!("fixture.{}", format.extension()));
        std::fs::write(&path, bytes).unwrap();
        read(&path, format, &args).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn args() -> Value {
    json!({"attachment_id":"fixture","max_chars":30000})
}
fn text(value: &Value) -> &str {
    value["text"].as_str().unwrap()
}

#[test]
fn word_reads_paragraphs_tables_footnotes_and_ignores_deleted_text_and_field_code() {
    let f = Fixture::new();
    let bytes = docx();
    validate(&bytes, Format::Docx).unwrap();
    let result = f.read(&bytes, Format::Docx, args());
    assert!(
        text(&result).contains("背景与 task")
            && text(&result).contains("正文🙂")
            && text(&result).contains("数值 42")
    );
    assert!(
        !text(&result).contains("Deleted secret")
            && !text(&result).contains("dangerous field instruction")
    );
    assert_eq!(result["table_count"], 1);
    let foot = f.read(
        &bytes,
        Format::Docx,
        json!({"attachment_id":"fixture","section":"footnotes"}),
    );
    assert!(text(&foot).contains("脚注 evidence"));
    let overview = f.read(
        &bytes,
        Format::Docx,
        json!({"attachment_id":"fixture","view":"overview"}),
    );
    assert_eq!(overview["headings"][0]["text"], "背景与 task");
}
#[test]
fn powerpoint_follows_relationship_order_and_per_slide_notes() {
    let f = Fixture::new();
    let bytes = pptx();
    let first = f.read(
        &bytes,
        Format::Pptx,
        json!({"attachment_id":"fixture","slides":[1]}),
    );
    assert!(
        text(&first).contains("Actually first")
            && text(&first).contains("Speaker explanation for FIRST")
    );
    assert!(!text(&first).contains("Actually second"));
    let second = f.read(
        &bytes,
        Format::Pptx,
        json!({"attachment_id":"fixture","slides":[2]}),
    );
    assert!(
        text(&second).contains("Actually second") && !text(&second).contains("Speaker explanation")
    );
    let no_notes = f.read(
        &bytes,
        Format::Pptx,
        json!({"attachment_id":"fixture","slides":[1],"include_notes":false}),
    );
    assert!(!text(&no_notes).contains("Speaker explanation"));
}
#[test]
fn excel_preserves_addresses_unicode_formulas_missing_caches_formats_and_hidden_sheets() {
    let f = Fixture::new();
    let bytes = xlsx();
    let value = f.read(
        &bytes,
        Format::Xlsx,
        json!({"attachment_id":"fixture","range":"B2:E3","max_chars":30000}),
    );
    let cells = text(&value)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(cells[0]["cell"], "B2");
    assert_eq!(cells[0]["value"], "精确标题");
    assert_eq!(cells[3]["value"], "42");
    assert_eq!(cells[4]["formula"], "B3*2");
    assert_eq!(cells[4]["value"], "84");
    assert!(cells[5]["value"].is_null());
    assert_eq!(cells[6]["number_format"], "yyyy-mm-dd");
    assert_eq!(value["date_system"], "1904");
    let hidden = f.read(
        &bytes,
        Format::Xlsx,
        json!({"attachment_id":"fixture","sheet":"Hidden","range":"A1"}),
    );
    assert!(text(&hidden).contains("Hidden evidence"));
    let overview = f.read(
        &bytes,
        Format::Xlsx,
        json!({"attachment_id":"fixture","view":"overview"}),
    );
    assert_eq!(overview["sheets"][1]["state"], "hidden");
}
#[test]
fn csv_multiline_escaped_quotes_empty_cells_and_formula_text_are_literal() {
    let f = Fixture::new();
    let bytes = b"\xef\xbb\xbf\"name\",\"line 1\nline 2\",\"a\"\"b\"\r\n=SUM(A1:A2),,end\r\n";
    let value = f.read(bytes, Format::Csv, args());
    let content = text(&value);
    assert!(
        content.contains("line 1\\nline 2")
            && content.contains("a\\\"b")
            && content.contains("=SUM(A1:A2)")
    );
    assert!(!content.contains("\"formula\""));
    assert!(content.contains("\"cell\":\"B2\""));
}
#[test]
fn unicode_slices_reconstruct_office_text_and_excel_grid_paging_is_explicit() {
    let f = Fixture::new();
    let bytes = docx();
    let full = f.read(&bytes, Format::Docx, args());
    let mut joined = String::new();
    let mut offset = 0;
    loop {
        let part = f.read(
            &bytes,
            Format::Docx,
            json!({"attachment_id":"fixture","offset":offset,"max_chars":7}),
        );
        joined.push_str(text(&part));
        match part["next_offset"].as_u64() {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(joined, text(&full));
    let page = f.read(
        &xlsx(),
        Format::Xlsx,
        json!({"attachment_id":"fixture","range":"B2:C2"}),
    );
    assert_eq!(page["next_row"], 3);
    assert_eq!(page["next_column"], 4);
}
#[test]
fn office_archives_do_not_follow_external_relationships_or_expand_unbounded_xml() {
    assert!(resolve_target("word/document.xml", "../../secrets.xml").is_err());
    assert!(resolve_target("word/document.xml", "file:///secret").is_err());
    assert!(parse_xml("<!DOCTYPE x [<!ENTITY a 'expand'>]><x>&a;</x>").is_err());
    assert!(validate(&docx(), Format::Xlsx).is_err());
    let xml = rels(
        r#"<Relationship Id="secret" Type="anything" Target="https://example.test/private" TargetMode="External"/>"#,
    );
    let mut pkg = Package::new(zip(&[
        ("[Content_Types].xml", "<Types/>"),
        ("word/_rels/document.xml.rels", &xml),
    ]))
    .unwrap();
    assert!(pkg.relationships("word/document.xml").unwrap().is_empty());
}
#[test]
fn all_office_formats_remain_local_scoped_and_readable_after_checkpoint_reload() {
    let f = Fixture::new();
    for (format, bytes) in [
        (Format::Docx, docx()), (Format::Doc, legacy_doc()),
        (Format::Pptx, pptx()), (Format::Ppt, legacy_ppt()),
        (Format::Xlsx, xlsx()), (Format::Xls, legacy_xls()),
        (Format::Csv, b"name,value\nabc,42".to_vec()),
    ] {
        let mut history = vec![
            json!({"role":"user","content":[{"type":"local_document","id":"draft","filename":format!("fixture.{}",format.extension()),"format":format.extension(),"data_url":format!("data:application/octet-stream;base64,{}",base64::engine::general_purpose::STANDARD.encode(bytes))}]}),
        ];
        crate::chat_attachments::stage(f.0.to_str().unwrap(), &mut history).unwrap();
        let id = history[0]["content"][0]["id"].clone();
        let mut summary = json!({"role":"user","content":"Previous research context"});
        crate::chat_attachments::retain_in_summary(&history, &mut summary);
        let saved: Vec<Value> =
            serde_json::from_str(&serde_json::to_string(&vec![summary]).unwrap()).unwrap();
        let scope = crate::chat_attachments::Attachments::from_messages(&saved);
        assert!(scope
            .call_tool(
                f.0.to_str().unwrap(),
                format.read_tool(),
                &json!({"attachment_id":id}),
                false
            )
            .is_ok());
        assert!(crate::chat_attachments::Attachments::default()
            .call_tool(
                f.0.to_str().unwrap(),
                format.read_tool(),
                &json!({"attachment_id":id}),
                false
            )
            .is_err());
        crate::chat_attachments::for_request(&mut history);
        let outbound = serde_json::to_string(&history).unwrap();
        assert!(
            !outbound.contains("base64")
                && !outbound.contains("data_url")
                && !outbound.contains("local_document")
        );
        assert!(outbound.contains(format.read_tool()));
    }
}

fn ole(parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut file = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    for (name, bytes) in parts {
        file.create_stream(*name).unwrap().write_all(bytes).unwrap();
    }
    file.into_inner().into_inner()
}
fn put16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn legacy_doc() -> Vec<u8> {
    let ascii = b"Live body\r";
    let unicode = "中文表格\u{7}42\r\u{13}FIELD CODE\u{14}Saved result\u{15}\r";
    let foot = "Footnote evidence\r";
    let units = (unicode.to_string() + foot)
        .encode_utf16()
        .collect::<Vec<_>>();
    let mut data = vec![0u8; 1024];
    put16(&mut data, 0, 0xa5ec);
    put16(&mut data, 2, 0xc1);
    put16(&mut data, 10, 0x200);
    put16(&mut data, 32, 14);
    put16(&mut data, 62, 22);
    put32(
        &mut data,
        76,
        (ascii.len() + unicode.encode_utf16().count()) as u32,
    );
    put32(&mut data, 80, foot.encode_utf16().count() as u32);
    put16(&mut data, 152, 93);
    let ascii_at = data.len();
    data.extend_from_slice(ascii);
    let unicode_at = data.len();
    for unit in &units {
        data.extend_from_slice(&unit.to_le_bytes());
    }
    data.extend_from_slice(b"Stale deleted document bytes must never appear");
    let mut plc = vec![0u8; 28];
    put32(&mut plc, 4, ascii.len() as u32);
    put32(&mut plc, 8, (ascii.len() + units.len()) as u32);
    put32(&mut plc, 14, 0x40000000 | ((ascii_at * 2) as u32));
    put32(&mut plc, 22, unicode_at as u32);
    let mut clx = vec![2];
    clx.extend_from_slice(&(plc.len() as u32).to_le_bytes());
    clx.extend_from_slice(&plc);
    put32(&mut data, 418, 0);
    put32(&mut data, 422, clx.len() as u32);
    ole(&[("/WordDocument", data), ("/1Table", clx)])
}
#[test]
fn legacy_word_reads_unicode_compressed_pieces_fields_tables_and_only_current_text() {
    let f = Fixture::new();
    let bytes = legacy_doc();
    validate(&bytes, Format::Doc).unwrap();
    let result = f.read(&bytes, Format::Doc, args());
    let content = text(&result);
    assert!(
        content.contains("Live body")
            && content.contains("中文表格\t42")
            && content.contains("Saved result")
    );
    assert!(
        !content.contains("Stale deleted")
            && !content.contains("FIELD CODE")
            && !content.contains("Footnote evidence")
    );
    let foot = f.read(
        &bytes,
        Format::Doc,
        json!({"attachment_id":"fixture","section":"footnotes"}),
    );
    assert!(text(&foot).contains("Footnote evidence"));
}
fn rec(kind: u16, instance: u16, container: bool, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&((instance << 4) | if container { 15 } else { 0 }).to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}
fn ppt_text(value: &str) -> Vec<u8> {
    rec(
        4000,
        0,
        false,
        &value
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    )
}
fn persist(id: u32, slide_id: u32) -> Vec<u8> {
    let mut p = vec![0; 20];
    put32(&mut p, 0, id);
    put32(&mut p, 12, slide_id);
    rec(1011, 0, false, &p)
}
fn directory(entries: &[(u32, usize)]) -> Vec<u8> {
    let mut p = Vec::new();
    for (id, offset) in entries {
        p.extend_from_slice(&(id | (1 << 20)).to_le_bytes());
        p.extend_from_slice(&(*offset as u32).to_le_bytes());
    }
    rec(6002, 0, false, &p)
}
fn edit(previous: usize, dir: usize) -> Vec<u8> {
    let mut p = vec![0; 28];
    put32(&mut p, 8, previous as u32);
    put32(&mut p, 12, dir as u32);
    put32(&mut p, 16, 1);
    rec(4085, 0, false, &p)
}
fn append(data: &mut Vec<u8>, bytes: Vec<u8>) -> usize {
    let at = data.len();
    data.extend(bytes);
    at
}
fn legacy_ppt() -> Vec<u8> {
    let mut data = vec![0u8; 8];
    let old_a = append(&mut data, rec(1006, 0, true, &ppt_text("OLD stale shape")));
    let b = append(&mut data, rec(1006, 0, true, &ppt_text("B own text")));
    let mut note_atom = vec![0u8; 8];
    put32(&mut note_atom, 0, 256);
    let note = append(
        &mut data,
        rec(
            1008,
            0,
            true,
            &[
                rec(1009, 0, false, &note_atom),
                ppt_text("Notes only for A"),
            ]
            .concat(),
        ),
    );
    let old_list = rec(
        4080,
        0,
        true,
        &[
            persist(2, 256),
            ppt_text("OLD outline A"),
            persist(3, 257),
            ppt_text("B outline"),
        ]
        .concat(),
    );
    let notes_list = rec(4080, 2, true, &persist(4, 258));
    let old_doc = append(
        &mut data,
        rec(1000, 0, true, &[old_list, notes_list.clone()].concat()),
    );
    let old_dir = append(
        &mut data,
        directory(&[(1, old_doc), (2, old_a), (3, b), (4, note)]),
    );
    let old_edit = append(&mut data, edit(0, old_dir));
    let new_a = append(
        &mut data,
        rec(1006, 0, true, &ppt_text("LATEST shape A 中文")),
    );
    let new_list = rec(
        4080,
        0,
        true,
        &[
            persist(3, 257),
            ppt_text("B now first"),
            persist(2, 256),
            ppt_text("LATEST outline A"),
        ]
        .concat(),
    );
    let new_doc = append(
        &mut data,
        rec(1000, 0, true, &[new_list, notes_list].concat()),
    );
    let new_dir = append(&mut data, directory(&[(1, new_doc), (2, new_a)]));
    let new_edit = append(&mut data, edit(old_edit, new_dir));
    let mut current = vec![0; 20];
    put32(&mut current, 0, 20);
    put32(&mut current, 4, 0xe391c05f);
    put32(&mut current, 8, new_edit as u32);
    ole(&[
        ("/PowerPoint Document", data),
        ("/Current User", rec(4086, 0, false, &current)),
    ])
}
#[test]
fn legacy_powerpoint_resolves_latest_incremental_save_slide_order_and_notes_by_id() {
    let f = Fixture::new();
    let bytes = legacy_ppt();
    let first = f.read(
        &bytes,
        Format::Ppt,
        json!({"attachment_id":"fixture","slides":[1]}),
    );
    assert!(text(&first).contains("B now first") && !text(&first).contains("Notes only for A"));
    let second = f.read(
        &bytes,
        Format::Ppt,
        json!({"attachment_id":"fixture","slides":[2]}),
    );
    assert!(
        text(&second).contains("LATEST shape A 中文")
            && text(&second).contains("LATEST outline A")
            && text(&second).contains("Notes only for A")
    );
    assert!(!text(&second).contains("OLD"));
}
fn biff(kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&kind.to_le_bytes());
    v.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    v.extend_from_slice(payload);
    v
}
fn legacy_xls() -> Vec<u8> {
    let mut bof = vec![0; 16];
    put16(&mut bof, 0, 0x0600);
    put16(&mut bof, 2, 5);
    let mut global = biff(0x0809, &bof);
    global.extend(biff(0x0042, &1200u16.to_le_bytes()));
    let title = "旧版中文".encode_utf16().collect::<Vec<_>>();
    let mut sst = vec![0; 8];
    put32(&mut sst, 0, 1);
    put32(&mut sst, 4, 1);
    sst.extend_from_slice(&(title.len() as u16).to_le_bytes());
    sst.push(1);
    for unit in title {
        sst.extend_from_slice(&unit.to_le_bytes());
    }
    global.extend(biff(0x00fc, &sst));
    let mut bound = vec![0; 8];
    bound[6] = 4;
    bound.extend_from_slice(b"Data");
    let offset = global.len() + 4 + bound.len() + 4;
    put32(&mut bound, 0, offset as u32);
    global.extend(biff(0x0085, &bound));
    global.extend(biff(0x000a, &[]));
    put16(&mut bof, 2, 0x0010);
    global.extend(biff(0x0809, &bof));
    let mut dims = vec![0; 14];
    put32(&mut dims, 4, 2);
    put16(&mut dims, 10, 2);
    global.extend(biff(0x0200, &dims));
    let mut label = vec![0; 10];
    put32(&mut label, 6, 0);
    global.extend(biff(0x00fd, &label));
    let mut number = vec![0; 14];
    put16(&mut number, 0, 1);
    number[6..14].copy_from_slice(&42f64.to_le_bytes());
    global.extend(biff(0x0203, &number));
    let expression = [0x1e, 2, 0, 0x1e, 3, 0, 0x03];
    let mut formula = vec![0; 22];
    put16(&mut formula, 0, 1);
    put16(&mut formula, 2, 1);
    formula[6..14].copy_from_slice(&5f64.to_le_bytes());
    put16(&mut formula, 20, expression.len() as u16);
    formula.extend(expression);
    global.extend(biff(0x0006, &formula));
    global.extend(biff(0x000a, &[]));
    ole(&[("/Workbook", global)])
}
#[test]
fn legacy_excel_reads_real_biff_shared_strings_numbers_and_formulas() {
    let f = Fixture::new();
    let bytes = legacy_xls();
    let result = f.read(&bytes, Format::Xls, args());
    let content = text(&result);
    assert!(
        content.contains("旧版中文")
            && content.contains("42")
            && content.contains("2+3")
            && content.contains("\"formula\""),
        "{content}"
    );
}
#[test]
fn legacy_formats_refuse_wrong_containers_and_encryption_and_sparse_allocation_bombs() {
    assert!(validate(&docx(), Format::Doc).is_err());
    assert!(validate(&legacy_doc(), Format::Ppt).is_err());
    let encrypted = ole(&[("/EncryptedPackage", vec![0; 64])]);
    assert!(validate(&encrypted, Format::Doc)
        .unwrap_err()
        .contains("Password"));
    let mut dims = vec![0; 14];
    put32(&mut dims, 4, 65536);
    put16(&mut dims, 10, 256);
    let bomb = ole(&[("/Workbook", biff(0x0200, &dims))]);
    assert!(legacy::validate_xls_size(&bomb).is_err());
}

#[test]
fn csv_handles_utf16_gb18030_and_pages_without_coercing_zeros_or_numbers() {
    let f = Fixture::new();
    let source = "名称,编号,说明\r\n中文,0000123,\"comma, quote \"\" and newline\nsecond line\"\r\n";
    for bytes in [source.as_bytes().to_vec(), {
        let mut bytes = vec![0xff, 0xfe];
        for unit in source.encode_utf16() { bytes.extend(unit.to_le_bytes()); }
        bytes
    }, encoding_rs::GB18030.encode(source).0.into_owned()] {
        let full = f.read(&bytes, Format::Csv, args());
        assert!(text(&full).contains("中文") && text(&full).contains("0000123"));
        let mut joined = String::new(); let mut offset = 0;
        loop {
            let slice = f.read(&bytes, Format::Csv, json!({"attachment_id":"fixture","offset":offset,"max_chars":11}));
            joined.push_str(text(&slice));
            match slice["next_offset"].as_u64() { Some(next) => offset=next, None => break }
        }
        assert_eq!(joined, text(&full));
    }
}
