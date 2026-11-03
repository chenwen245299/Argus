//! Bounded front matter for metadata, independent of full-book extraction/OCR.
use base64::Engine;
use crate::models::{ChatContentPart, ImageUrlData};

pub(crate) struct Evidence {
    pub text: String,
    pub images: Vec<ChatContentPart>,
    pub inspected: usize,
    pub total: usize,
    pub ebook: bool,
}

const MAX_TEXT_PER_PAGE: usize = 1800;
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

pub(crate) fn collect(root: &str, slug: &str, limit: usize, vision: bool) -> Result<Evidence, String> {
    let meta = crate::paper::read_meta(root, slug)?;
    if crate::ebook::is_ebook_file_type(meta.file_type.as_deref()) {
        let book = crate::ebook::get_parsed(root, slug)?;
        let mut text = format!("Embedded ebook metadata (a hint; verify against the book's own front matter):\n{}\n",
            serde_json::to_string(&book.meta).map_err(|e| e.to_string())?);
        let inspected = limit.min(book.chapter_text.len());
        for (index, chapter) in book.chapter_text.iter().take(inspected).enumerate() {
            text.push_str(&format!("\n[Opening ebook section {}]\n{}\n", index + 1,
                chapter.chars().take(MAX_TEXT_PER_PAGE).collect::<String>()));
        }
        let mut images = Vec::new();
        if vision {
            // EPUBs reflow and have no fixed PDF pages. Show their actual cover
            // resources, alongside the first five/ten spine sections as text.
            let mut covers: Vec<_> = book.resources.keys().filter(|key| {
                let key = key.to_lowercase();
                key.contains("cover") || key.contains("封面")
            }).collect();
            covers.sort();
            let mut bytes = 0;
            for key in covers.into_iter().take(2) {
                if let Ok(image) = crate::ebook::read_resource(&book, key) {
                    if !matches!(image.mime.as_str(), "image/png" | "image/jpeg" | "image/webp" | "image/gif") { continue; }
                    bytes += image.base64.len();
                    if bytes > MAX_IMAGE_BYTES { break; }
                    images.push(ChatContentPart::Text { text: format!("[Ebook cover resource: {key}]") });
                    images.push(ChatContentPart::ImageUrl { image_url: ImageUrlData {
                        url: format!("data:{};base64,{}", image.mime, image.base64), detail: None,
                    } });
                }
            }
        }
        return Ok(Evidence { text, images, inspected, total: book.chapter_text.len(), ebook: true });
    }

    let path = crate::metadata::find_pdf_in_dir(root, slug);
    let (total, texts) = crate::render::metadata_page_texts(&path, limit).or_else(|_| {
        let doc = lopdf::Document::load(&path).map_err(|e| format!("Cannot open PDF for metadata recognition: {e}"))?;
        let total = doc.get_pages().len();
        let texts = (1..=limit.min(total)).map(|page| doc.extract_text(&[page as u32]).unwrap_or_default()).collect();
        Ok::<_, String>((total, texts))
    })?;
    let inspected = limit.min(total);
    let mut text = String::new();
    let mut images = Vec::new();
    let mut image_bytes = 0;
    for page in 1..=inspected {
        let page_text = &texts[page - 1];
        text.push_str(&format!("\n[PDF page {page}]\n{}\n",
            page_text.chars().take(MAX_TEXT_PER_PAGE).collect::<String>()));
        if vision {
            match crate::render::render_metadata_page_png(&path, page as u32) {
                Ok(png) if image_bytes + png.len() <= MAX_IMAGE_BYTES => {
                    image_bytes += png.len();
                    images.push(ChatContentPart::Text { text: format!("[PDF page {page} image]") });
                    images.push(ChatContentPart::ImageUrl { image_url: ImageUrlData {
                        url: format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)),
                        detail: None,
                    } });
                }
                Ok(_) => text.push_str(&format!("[Page {page} image omitted: preview size limit]\n")),
                Err(e) => text.push_str(&format!("[Page {page} image unavailable: {e}]\n")),
            }
        }
    }
    Ok(Evidence { text, images, inspected, total, ebook: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Object, Stream, content::{Content, Operation}};
    struct Fixture { root: String }
    impl Fixture {
        fn pdf(pages: usize) -> Self {
            let root = std::env::temp_dir().join(format!("argus-metadata-evidence-{}", uuid::Uuid::new_v4()));
            let dir = root.join("papers").join("book");
            std::fs::create_dir_all(&dir).unwrap();
            let mut doc = lopdf::Document::with_version("1.5");
            let pages_id = doc.new_object_id();
            let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
            let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
            let mut kids = Vec::new();
            for index in 1..=pages {
                let text = if index == 6 { "TRUE BOOK TITLE - Actual Author - Copyright 2026".to_string() }
                    else { format!("Front matter page {index}") };
                let content = Content { operations: vec![
                    Operation::new("BT", vec![]), Operation::new("Tf", vec!["F1".into(), 18.into()]),
                    Operation::new("Td", vec![40.into(), 700.into()]),
                    Operation::new("Tj", vec![Object::string_literal(text)]), Operation::new("ET", vec![]),
                ] }.encode().unwrap();
                let content = doc.add_object(Stream::new(dictionary! {}, content));
                let page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id,
                    "Contents" => content, "Resources" => resources, "MediaBox" => vec![0.into(),0.into(),612.into(),792.into()] });
                kids.push(Object::Reference(page));
            }
            doc.objects.insert(pages_id, dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => pages as i64 }.into());
            let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
            doc.trailer.set("Root", catalog);
            doc.save(dir.join("book.pdf")).unwrap();
            std::fs::write(dir.join("meta.json"), serde_json::json!({
                "id":"unique-book", "title":"Misleading filename", "authors":[], "added_at":"2026-11-03", "reading_status":"unread"
            }).to_string()).unwrap();
            Self { root: root.to_string_lossy().into() }
        }
    }
    impl Drop for Fixture { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); } }

    #[test]
    fn first_five_and_ten_pages_have_labeled_text_without_reading_later_pages() {
        let f = Fixture::pdf(12);
        let first = collect(&f.root, "book", 5, false).unwrap();
        assert_eq!((first.inspected, first.total), (5,12));
        assert!(first.images.is_empty());
        assert!(!first.text.contains("TRUE BOOK TITLE"));
        let expanded = collect(&f.root, "book", 10, false).unwrap();
        assert_eq!(expanded.inspected, 10);
        assert!(expanded.text.contains("[PDF page 6]"));
        assert!(expanded.text.contains("TRUE BOOK TITLE"));
        assert!(!expanded.text.contains("[PDF page 11]"));
    }

    #[test]
    fn vision_receives_five_real_bounded_page_images_and_text_only_models_receive_none() {
        let f = Fixture::pdf(7);
        let evidence = collect(&f.root, "book", 5, true).unwrap();
        let images: Vec<_> = evidence.images.iter().filter_map(|p| match p {
            ChatContentPart::ImageUrl { image_url } => Some(image_url), _ => None,
        }).collect();
        assert_eq!(images.len(), 5);
        for image in images {
            let png = base64::engine::general_purpose::STANDARD.decode(image.url.split(',').nth(1).unwrap()).unwrap();
            let bitmap = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
            assert_eq!(bitmap.width().max(bitmap.height()), 1600);
            assert!(bitmap.height() > bitmap.width());
        }
        let short = Fixture::pdf(2);
        let evidence = collect(&short.root, "book", 5, true).unwrap();
        assert_eq!((evidence.inspected,evidence.total), (2,2));
        assert_eq!(evidence.images.iter().filter(|p| matches!(p, ChatContentPart::ImageUrl { .. })).count(), 2);
    }
}
