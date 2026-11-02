//! Freeze PDF pixels before note approval; only approved writes persist them.
use super::paper_images::{Crop, CropPreviews, SendImageArgs};
use base64::Engine;
use serde::{Deserialize, Serialize};

pub const INSTRUCTIONS: &str = " To embed PDF figures in the note, pass images:[{id,slug?,page,crop?,dpi?}] and place ![caption](argus-note-image://id) at each desired position in content. slug defaults to the note's paper; other library papers are supported. Use 1-based PDF page indices and top-left 0–1 crop fractions. First inspect the full page, then call view_paper_page with the EXACT crop and WAIT for its result in an earlier round; check the figure, axes, legend and caption before writing. Unpreviewed crops are rejected. Models without vision may embed full pages only. Up to 8 images; only approved writes save permanent, portable note assets. Do not use chat image references, base64 or absolute paths in note content. Read back get_note to verify the saved content.";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageArgs {
    id: String,
    slug: Option<String>,
    page: u32,
    crop: Option<Crop>,
    dpi: Option<u32>,
}

impl ImageArgs {
    fn source(&self, note_slug: &str) -> SendImageArgs {
        SendImageArgs {
            slug: self.slug.clone().unwrap_or_else(|| note_slug.into()),
            page: self.page,
            crop: self.crop,
            dpi: self.dpi,
            caption: None,
        }
    }
}

fn arguments(args: &serde_json::Value) -> Result<Vec<ImageArgs>, String> {
    let Some(images) = args.get("images") else {
        return Ok(vec![]);
    };
    let images: Vec<ImageArgs> =
        serde_json::from_value(images.clone()).map_err(|e| format!("Invalid note images: {e}"))?;
    if images.len() > 8 {
        return Err("A note write accepts at most 8 images.".into());
    }
    let mut ids = std::collections::HashSet::new();
    for image in &images {
        if image.id.is_empty()
            || image.id.len() > 64
            || !image
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || !ids.insert(&image.id)
        {
            return Err(
                "Each note image needs a unique id of 1–64 letters, digits, _ or -.".into(),
            );
        }
        image
            .source(args["slug"].as_str().unwrap_or(""))
            .validate()?;
    }
    Ok(images)
}

pub fn check_previews(
    previews: &CropPreviews,
    args: &serde_json::Value,
    vision: bool,
    round: usize,
) -> Result<(), String> {
    let slug = args["slug"].as_str().unwrap_or("");
    for image in arguments(args)? {
        let source = image.source(slug);
        previews.check_send(
            &serde_json::json!({"slug":source.slug,"page":source.page,
            "crop":source.crop,"dpi":source.dpi}),
            vision,
            round,
        )?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct PreparedImage {
    pub asset: crate::paper::NoteImageAsset,
    source_slug: String,
    paper_id: String,
    path: std::path::PathBuf,
    revision: crate::render::PdfRevision,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImagePreview {
    pub path: String,
    pub data_url: String,
}

pub fn previews(images: &[PreparedImage]) -> Vec<ImagePreview> {
    images
        .iter()
        .map(|image| ImagePreview {
            path: format!("assets/{}", image.asset.name),
            data_url: format!(
                "data:image/png;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(&image.asset.bytes)
            ),
        })
        .collect()
}

/// No file or directory is created here, including on refused/malformed writes.
pub fn prepare(
    root: &str,
    args: &serde_json::Value,
    content: &str,
) -> Result<(String, Vec<PreparedImage>), String> {
    let images = arguments(args)?;
    let marker =
        regex::Regex::new(r"!\[[^\]\r\n]*\]\(argus-note-image://([A-Za-z0-9_-]{1,64})\)").unwrap();
    let used: std::collections::HashSet<_> = marker
        .captures_iter(content)
        .map(|c| c[1].to_string())
        .collect();
    if used.len() != images.len() || images.iter().any(|i| !used.contains(&i.id)) {
        return Err("Every images entry must have a matching ![caption](argus-note-image://id) in content; all image ids must be declared.".into());
    }
    let mut content = content.to_string();
    let mut prepared = Vec::new();
    let mut total_bytes = 0;
    for image in images {
        let source = image.source(args["slug"].as_str().unwrap_or(""));
        let meta = crate::paper::read_meta(root, &source.slug)?;
        let document = super::tools::get_document_path(root, &source.slug)?;
        if document.file_type != "pdf" || !document.exists {
            return Err("Note images require an existing library PDF.".into());
        }
        let path = std::path::PathBuf::from(document.path);
        let revision = crate::render::pdf_revision(&path)?;
        let png = crate::render::render_pdf_page_png(
            &path,
            source.page,
            source.dpi.unwrap_or(crate::render::DEFAULT_DPI),
        )?;
        let rendered = super::paper_images::prepare_page_png(png, source.crop)?;
        if crate::render::pdf_revision(&path)? != revision {
            return Err("PDF changed while preparing note images. Inspect it again.".into());
        }
        total_bytes += rendered.png.len();
        if rendered.png.len() > 20 * 1024 * 1024 || total_bytes > 40 * 1024 * 1024 {
            return Err("Note images exceed 20 MB per image or 40 MB per write. Reduce dpi or split the note update.".into());
        }
        let name = format!("ai-{}.png", uuid::Uuid::new_v4());
        content = content.replace(
            &format!("](argus-note-image://{})", image.id),
            &format!("](assets/{name})"),
        );
        prepared.push(PreparedImage {
            asset: crate::paper::NoteImageAsset {
                name,
                bytes: rendered.png,
            },
            source_slug: source.slug,
            paper_id: meta.id,
            path,
            revision,
        });
    }
    if content.contains("argus-note-image:")
        || content.contains("argus-image:")
        || content.contains("argus-chat-image:")
    {
        return Err("Unresolved image reference in note content. Use images and ![caption](argus-note-image://id), not chat-only image references.".into());
    }
    Ok((content, prepared))
}

pub fn validate_sources(root: &str, images: &[PreparedImage]) -> Result<(), String> {
    for image in images {
        if crate::paper::read_meta(root, &image.source_slug)?.id != image.paper_id
            || std::path::PathBuf::from(
                super::tools::get_document_path(root, &image.source_slug)?.path,
            ) != image.path
            || crate::render::pdf_revision(&image.path)? != image.revision
        {
            return Err("Source paper or PDF changed since the preview. Nothing was saved; inspect it again.".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::app_tools::{PendingWrite, CREATE_NOTE_TOOL, UPDATE_NOTE_TOOL};
    use serde_json::json;

    struct Library(String);
    impl Drop for Library {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn library() -> Library {
        use lopdf::{dictionary, Document, Object, Stream};
        let root = std::env::temp_dir().join(format!("argus-note-images-{}", uuid::Uuid::new_v4()));
        let paper = root.join("papers/source");
        std::fs::create_dir_all(&paper).unwrap();
        std::fs::write(
            paper.join("meta.json"),
            r#"{"id":"source-id","title":"Source figure"}"#,
        )
        .unwrap();
        let target = root.join("papers/target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.join("meta.json"),
            r#"{"id":"target-id","title":"Target notes"}"#,
        )
        .unwrap();
        let mut pdf = Document::with_version("1.4");
        let pages = pdf.new_object_id();
        let contents = pdf.add_object(Stream::new(
            dictionary! {},
            b"1 0 0 rg 0 100 200 100 re f 0 0 1 rg 0 0 200 100 re f".to_vec(),
        ));
        let page = pdf.add_object(dictionary! { "Type" => "Page", "Parent" => pages,
        "MediaBox" => vec![0.into(),0.into(),200.into(),200.into()],
        "Resources" => dictionary! {}, "Contents" => contents });
        pdf.objects.insert(
            pages,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 },
            ),
        );
        let catalog = pdf.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages });
        pdf.trailer.set("Root", catalog);
        pdf.save(paper.join("paper.pdf")).unwrap();
        Library(root.to_string_lossy().into_owned())
    }
    fn args() -> serde_json::Value {
        json!({"slug":"target", "title":"图文笔记", "content":"# 方法\n\n![上半图](argus-note-image://top)\n\n解释与 $x^2$。\n\n![下半图](argus-note-image://bottom)",
            "images":[{"id":"top","slug":"source","page":1,"dpi":72,"crop":{"x":0.1,"y":0.1,"width":0.8,"height":0.3}},
                {"id":"bottom","slug":"source","page":1,"dpi":72,"crop":{"x":0.1,"y":0.6,"width":0.8,"height":0.3}}]})
    }
    fn assets_dir(root: &str) -> std::path::PathBuf {
        std::path::Path::new(root).join("papers/target/notes/assets")
    }
    fn assert_pixels(bytes: &[u8], color: [u8; 4]) {
        let image = image::load_from_memory(bytes).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (160, 60));
        assert!(image.pixels().all(|p| p.0 == color));
    }

    #[test]
    fn note_crops_require_earlier_exact_previews_and_vision() {
        let args = args();
        let mut seen = CropPreviews::default();
        assert!(check_previews(&seen, &args, true, 2).is_err());
        for image in args["images"].as_array().unwrap() {
            seen.remember(
                &json!({"slug":"source", "pages":[{"page":1,"crop":image["crop"]}]}),
                2,
            );
        }
        assert!(check_previews(&seen, &args, true, 2).is_err());
        assert!(check_previews(&seen, &args, false, 3).is_err());
        assert!(check_previews(&seen, &args, true, 3).is_ok());
        let mut changed = args.clone();
        changed["images"][0]["crop"]["y"] = json!(0.11);
        assert!(check_previews(&seen, &changed, true, 3).is_err());
        assert!(check_previews(
            &seen,
            &json!({"slug":"source","images":[{"id":"page","page":1}]}),
            false,
            1
        )
        .is_ok());
    }

    #[test]
    fn full_pages_default_to_the_notes_paper_and_title_only_edits_preserve_legacy_content() {
        let lib = library();
        let args = json!({"slug":"source", "title":"Full page", "content":"![整页](argus-note-image://page)",
            "images":[{"id":"page","page":1,"dpi":72}]});
        let pending = PendingWrite::parse(&lib.0, CREATE_NOTE_TOOL, &args).unwrap();
        let saved = pending.execute(&lib.0).unwrap();
        let content = crate::paper::get_note(&lib.0, "source", saved["note_id"].as_str().unwrap());
        let name = content
            .split("assets/")
            .nth(1)
            .unwrap()
            .trim_end_matches(')');
        let bytes = crate::paper::read_note_asset(&lib.0, "source", name).unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (200, 200));
        assert_eq!(image.get_pixel(100, 25).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(100, 175).0, [0, 0, 255, 255]);
        let legacy = crate::paper::create_note_with(
            &lib.0,
            "target",
            "old",
            "Legacy example: `argus-image://old.png`",
        )
        .unwrap();
        PendingWrite::parse(
            &lib.0,
            UPDATE_NOTE_TOOL,
            &json!({"slug":"target","note_id":legacy.id,"title":"renamed"}),
        )
        .unwrap()
        .execute(&lib.0)
        .unwrap();
        assert_eq!(
            crate::paper::get_note(&lib.0, "target", &legacy.id),
            "Legacy example: `argus-image://old.png`"
        );
        assert!(PendingWrite::parse(
            &lib.0,
            UPDATE_NOTE_TOOL,
            &json!({"slug":"target","note_id":legacy.id,"title":"again","images":[]})
        )
        .is_err());
    }

    #[test]
    fn grid_notes_commit_every_image_in_its_requested_table_position() {
        let lib = library();
        let mut args = args();
        args["content"] = json!("## 图解\n\n| ![上图](argus-note-image://top) | ![下图](argus-note-image://bottom) |\n| --- | --- |\n| ![下图对比](argus-note-image://bottom) | ![上图对比](argus-note-image://top) |\n\n完整解释。");
        let pending = PendingWrite::parse(&lib.0, CREATE_NOTE_TOOL, &args).unwrap();
        let preview = pending.preview(&lib.0);
        assert_eq!(preview.note_images.len(), 2);
        let top = &preview.note_images[0].path;
        let bottom = &preview.note_images[1].path;
        let expected = format!("## 图解\n\n| ![上图]({top}) | ![下图]({bottom}) |\n| --- | --- |\n| ![下图对比]({bottom}) | ![上图对比]({top}) |\n\n完整解释。");
        assert_eq!(preview.content, expected);
        let saved = pending.execute(&lib.0).unwrap();
        assert_eq!(
            crate::paper::get_note(&lib.0, "target", saved["note_id"].as_str().unwrap()),
            expected
        );
        assert_eq!(std::fs::read_dir(assets_dir(&lib.0)).unwrap().count(), 2);
    }

    #[test]
    fn approved_note_keeps_exact_preview_pixels_and_portable_links_after_move() {
        let mut lib = library();
        let pending = PendingWrite::parse(&lib.0, CREATE_NOTE_TOOL, &args()).unwrap();
        assert!(
            !assets_dir(&lib.0).exists(),
            "preparation must not save images"
        );
        let preview = pending.preview(&lib.0);
        assert_eq!(preview.note_images.len(), 2);
        assert_pixels(
            &base64::engine::general_purpose::STANDARD
                .decode(preview.note_images[0].data_url.split(',').nth(1).unwrap())
                .unwrap(),
            [255, 0, 0, 255],
        );
        assert!(!preview.content.contains("argus-note-image:"));
        assert!(!preview.content.contains("base64"));
        let saved = pending.execute(&lib.0).unwrap();
        let id = saved["note_id"].as_str().unwrap();
        assert_eq!(
            crate::paper::get_note(&lib.0, "target", id),
            preview.content
        );
        for (index, image) in preview.note_images.iter().enumerate() {
            let bytes = crate::paper::read_note_asset(
                &lib.0,
                "target",
                image.path.strip_prefix("assets/").unwrap(),
            )
            .unwrap();
            assert_pixels(
                &bytes,
                if index == 0 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                },
            );
        }
        let moved = format!("{}-moved", lib.0);
        std::fs::rename(&lib.0, &moved).unwrap();
        lib.0 = moved;
        assert_eq!(
            crate::paper::get_note(&lib.0, "target", id),
            preview.content
        );
        assert_pixels(
            &crate::paper::read_note_asset(
                &lib.0,
                "target",
                preview.note_images[0].path.strip_prefix("assets/").unwrap(),
            )
            .unwrap(),
            [255, 0, 0, 255],
        );
        assert!(!std::path::Path::new(&lib.0).join("chats").exists());
    }

    #[test]
    fn extending_existing_note_preserves_links_and_stale_edits_save_no_new_images() {
        let lib = library();
        let first = PendingWrite::parse(&lib.0, CREATE_NOTE_TOOL, &args())
            .unwrap()
            .execute(&lib.0)
            .unwrap();
        let id = first["note_id"].as_str().unwrap();
        let original = crate::paper::get_note(&lib.0, "target", id);
        let mut update = args();
        update["note_id"] = json!(id);
        update["content"] = json!(format!(
            "{original}\n\n{}",
            args()["content"].as_str().unwrap()
        ));
        let pending = PendingWrite::parse(&lib.0, UPDATE_NOTE_TOOL, &update).unwrap();
        let preview = pending.preview(&lib.0);
        assert_eq!(preview.note_before.unwrap().content, original);
        crate::paper::save_note(&lib.0, "target", id, "用户的新草稿").unwrap();
        assert!(pending.execute(&lib.0).unwrap_err().contains("changed"));
        assert_eq!(std::fs::read_dir(assets_dir(&lib.0)).unwrap().count(), 2);
        assert_eq!(crate::paper::get_note(&lib.0, "target", id), "用户的新草稿");
        let pending = PendingWrite::parse(&lib.0, UPDATE_NOTE_TOOL, &update).unwrap();
        pending.execute(&lib.0).unwrap();
        let saved = crate::paper::get_note(&lib.0, "target", id);
        assert!(saved.starts_with(&original));
        assert_eq!(std::fs::read_dir(assets_dir(&lib.0)).unwrap().count(), 4);
    }

    #[test]
    fn malformed_images_refusal_and_changed_sources_leave_no_assets() {
        let lib = library();
        for bad in [
            json!({"images":[{"id":"../escape","page":1}]}),
            json!({"images":[{"id":"same","page":1},{"id":"same","page":1}]}),
            json!({"images":[{"id":"x","page":0}]}),
            json!({"images":[{"id":"x","page":1,"dpi":900}]}),
            json!({"images":[{"id":"x","page":1,"path":"secret"}]}),
            json!({"images":vec![json!({"id":"x","page":1});9]}),
        ] {
            assert!(arguments(&bad).is_err());
        }
        let mut bad = args();
        bad["content"] = json!("![missing](argus-note-image://absent)");
        assert!(PendingWrite::parse(&lib.0, CREATE_NOTE_TOOL, &bad).is_err());
        bad = args();
        bad["images"][0]["slug"] = json!("../source");
        assert!(PendingWrite::parse(&lib.0, CREATE_NOTE_TOOL, &bad).is_err());
        bad = json!({"slug":"target","title":"bad","content":"![](argus-image://chat.png)"});
        assert!(PendingWrite::parse(&lib.0, CREATE_NOTE_TOOL, &bad).is_err());
        let pending = PendingWrite::parse(&lib.0, CREATE_NOTE_TOOL, &args()).unwrap();
        pending.preview(&lib.0);
        drop(pending); // refusal: execution is never called
        assert!(crate::paper::list_notes(&lib.0, "target").is_empty());
        assert!(!assets_dir(&lib.0).exists());
        let pending = PendingWrite::parse(&lib.0, CREATE_NOTE_TOOL, &args()).unwrap();
        std::fs::write(
            std::path::Path::new(&lib.0).join("papers/source/paper.pdf"),
            b"changed",
        )
        .unwrap();
        assert!(pending.execute(&lib.0).is_err());
        assert!(crate::paper::list_notes(&lib.0, "target").is_empty());
        assert!(!assets_dir(&lib.0).exists());
    }

    #[test]
    fn asset_failures_never_publish_broken_notes_or_replace_shared_pixels() {
        let lib = library();
        let note = crate::paper::create_note_with(&lib.0, "target", "original", "kept").unwrap();
        let before = crate::paper::note_snapshot(&lib.0, "target", &note.id).unwrap();
        let dir = assets_dir(&lib.0);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("shared.png"), b"shared pixels").unwrap();
        let assets = vec![
            crate::paper::NoteImageAsset {
                name: "new.png".into(),
                bytes: vec![1],
            },
            crate::paper::NoteImageAsset {
                name: "shared.png".into(),
                bytes: vec![2],
            },
        ];
        assert!(crate::paper::update_note_checked_with_assets(
            &lib.0,
            "target",
            Some("target-id"),
            &before,
            "new",
            "![](assets/new.png)",
            &assets
        )
        .is_err());
        assert_eq!(crate::paper::get_note(&lib.0, "target", &note.id), "kept");
        assert!(!dir.join("new.png").exists());
        assert_eq!(
            std::fs::read(dir.join("shared.png")).unwrap(),
            b"shared pixels"
        );
        assert!(crate::paper::create_note_with_assets(
            &lib.0,
            "target",
            Some("replaced-paper"),
            "new",
            "body",
            &assets
        )
        .is_err());
        assert_eq!(crate::paper::list_notes(&lib.0, "target").len(), 1);
        #[cfg(unix)]
        {
            std::fs::remove_file(dir.join("shared.png")).unwrap();
            std::fs::remove_dir(&dir).unwrap();
            let outside = std::path::Path::new(&lib.0).join("outside");
            std::fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, &dir).unwrap();
            assert!(crate::paper::create_note_with_assets(
                &lib.0,
                "target",
                Some("target-id"),
                "new",
                "body",
                &assets[..1]
            )
            .is_err());
            assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        }
    }
}
