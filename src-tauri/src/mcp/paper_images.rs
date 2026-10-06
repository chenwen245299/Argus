//! Render a paper page (or a crop of it) for delivery in an in-app answer.
//! No arbitrary paths, URLs or model-supplied image bytes are accepted. This
//! layer only reads the paper; copilot saves the result with the conversation.

use base64::Engine;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ImageEncoder};
use serde::{Deserialize, Serialize};

pub const SEND_PAPER_IMAGE_TOOL: &str = "send_paper_image";

/// Fractions of the displayed, rotated page, measured from its top-left corner.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Crop {
    fn validate(self) -> Result<(), String> {
        if ![self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            || self.x < 0.0
            || self.y < 0.0
            || self.width <= 0.0
            || self.height <= 0.0
            || self.x + self.width > 1.0 + 1e-9
            || self.y + self.height > 1.0 + 1e-9
        {
            return Err(
                "Invalid crop: use page fractions (0–1), positive width/height, \
                        and keep the rectangle inside the page. The origin is top-left."
                    .into(),
            );
        }
        Ok(())
    }

    fn pixels(self, width: u32, height: u32) -> Result<(u32, u32, u32, u32), String> {
        self.validate()?;
        if width == 0 || height == 0 {
            return Err("Cannot crop an empty image.".into());
        }
        let x = (self.x * width as f64).floor() as u32;
        let y = (self.y * height as f64).floor() as u32;
        let right = ((self.x + self.width) * width as f64)
            .ceil()
            .min(width as f64) as u32;
        let bottom = ((self.y + self.height) * height as f64)
            .ceil()
            .min(height as f64) as u32;
        if right <= x || bottom <= y {
            return Err("The crop is too small to contain a pixel.".into());
        }
        Ok((x, y, right - x, bottom - y))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendImageArgs {
    pub slug: String,
    pub page: u32,
    pub crop: Option<Crop>,
    pub caption: Option<String>,
    pub dpi: Option<u32>,
}

impl SendImageArgs {
    fn validate(&self) -> Result<(), String> {
        crate::path_guard::validate_segment("paper slug", &self.slug)?;
        if self.page == 0 {
            return Err("Page numbers are 1-based.".into());
        }
        if !(72..=300).contains(&self.dpi.unwrap_or(crate::render::DEFAULT_DPI)) {
            return Err("dpi must be between 72 and 300 (default 150).".into());
        }
        if self
            .caption
            .as_ref()
            .is_some_and(|s| s.chars().count() > 1000)
        {
            return Err("caption must be at most 1000 characters.".into());
        }
        if let Some(crop) = self.crop {
            crop.validate()?;
        }
        Ok(())
    }
}

fn crop_image(image: DynamicImage, crop: Option<Crop>) -> Result<DynamicImage, String> {
    match crop {
        Some(crop) => {
            let (x, y, w, h) = crop.pixels(image.width(), image.height())?;
            Ok(image.crop_imm(x, y, w, h))
        }
        None => Ok(image),
    }
}

pub fn render(root: &str, args: SendImageArgs) -> Result<serde_json::Value, String> {
    args.validate()?;
    let document = super::tools::get_document_path(root, &args.slug)?;
    if document.file_type != "pdf" {
        return Err("send_paper_image currently supports PDF papers only.".into());
    }
    if !document.exists {
        return Err(format!("No PDF file found for '{}'.", args.slug));
    }
    let dpi = args.dpi.unwrap_or(crate::render::DEFAULT_DPI);
    let png =
        crate::render::render_pdf_page_png(std::path::Path::new(&document.path), args.page, dpi)?;
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|e| format!("Decode rendered page: {e}"))?;
    let image = crop_image(image, args.crop)?;
    let (width, height) = (image.width(), image.height());
    // Preserve all pixels; cropping never resizes or resamples the figure.
    let png = if args.crop.is_some() {
        let mut bytes = Vec::new();
        PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::Up)
            .write_image(image.as_bytes(), width, height, image.color().into())
            .map_err(|e| format!("Encode cropped image: {e}"))?;
        bytes
    } else {
        png
    };
    Ok(serde_json::json!({
        "slug": args.slug,
        "pages": [{
            "page": args.page,
            "caption": args.caption.unwrap_or_default().trim(),
            "crop": args.crop,
            "dpi": dpi,
            "width": width,
            "height": height,
            "png_base64": base64::engine::general_purpose::STANDARD.encode(png),
        }],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_preserves_the_selected_pixels_and_uses_top_left_coordinates() {
        let original =
            image::RgbaImage::from_fn(100, 80, |x, y| image::Rgba([x as u8, y as u8, 123, 255]));
        let cropped = crop_image(
            DynamicImage::ImageRgba8(original.clone()),
            Some(Crop {
                x: 0.25,
                y: 0.5,
                width: 0.5,
                height: 0.25,
            }),
        )
        .unwrap()
        .to_rgba8();
        assert_eq!(cropped.dimensions(), (50, 20));
        for (x, y, pixel) in cropped.enumerate_pixels() {
            assert_eq!(pixel, original.get_pixel(x + 25, y + 40));
        }
    }

    #[test]
    fn crop_rounds_outwards_and_accepts_the_page_edge() {
        let crop = Crop {
            x: 0.1,
            y: 0.1,
            width: 0.9,
            height: 0.9,
        };
        assert_eq!(crop.pixels(11, 13).unwrap(), (1, 1, 10, 12));
        let full = Crop {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        };
        assert_eq!(full.pixels(100, 80).unwrap(), (0, 0, 100, 80));
    }

    /// Exercise the actual PDF renderer and tool dispatch, with asymmetric
    /// color regions so upside-down coordinates cannot accidentally pass.
    #[test]
    fn a_pdf_figure_is_rendered_and_cropped_through_the_agent_tool() {
        use lopdf::{dictionary, Document, Object, Stream};
        let root = std::env::temp_dir().join(format!("argus-send-image-{}", uuid::Uuid::new_v4()));
        let paper = root.join("papers/figure-paper");
        std::fs::create_dir_all(&paper).unwrap();
        std::fs::write(
            paper.join("meta.json"),
            r#"{"id":"figure-paper","title":"Test figure"}"#,
        )
        .unwrap();
        let mut pdf = Document::with_version("1.4");
        let pages_id = pdf.new_object_id();
        let content = pdf.add_object(Stream::new(
            dictionary! {},
            b"1 0 0 rg 0 100 200 100 re f 0 0 1 rg 0 0 200 100 re f".to_vec(),
        ));
        let page_id = pdf.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()],
            "Resources" => dictionary! {}, "Contents" => content,
        });
        pdf.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
            }),
        );
        let catalog = pdf.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        pdf.trailer.set("Root", catalog);
        pdf.save(paper.join("paper.pdf")).unwrap();

        let result = super::super::agent::call(
            root.to_str().unwrap(),
            SEND_PAPER_IMAGE_TOOL,
            &serde_json::json!({
                "slug": "figure-paper", "page": 1, "dpi": 72,
                "caption": "Upper red figure",
                "crop": {"x": 0.1, "y": 0.1, "width": 0.8, "height": 0.3},
            }),
        )
        .unwrap();
        let page = &result["pages"][0];
        assert_eq!(page["width"], 160);
        assert_eq!(page["height"], 60);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(page["png_base64"].as_str().unwrap())
            .unwrap();
        let cropped = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert!(
            cropped.pixels().all(|p| p.0 == [255, 0, 0, 255]),
            "crop took the wrong part of the PDF"
        );
        // An invalid page or an ebook is reported rather than fabricating an image.
        assert!(super::super::agent::call(
            root.to_str().unwrap(),
            SEND_PAPER_IMAGE_TOOL,
            &serde_json::json!({"slug":"figure-paper", "page":2})
        )
        .is_err());
        std::fs::write(
            paper.join("meta.json"),
            r#"{"id":"figure-paper","title":"Test ebook","file_type":"epub"}"#,
        )
        .unwrap();
        assert!(super::super::agent::call(
            root.to_str().unwrap(),
            SEND_PAPER_IMAGE_TOOL,
            &serde_json::json!({"slug":"figure-paper", "page":1})
        )
        .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_rectangles_and_render_requests_are_rejected_before_io() {
        for crop in [
            Crop {
                x: -0.1,
                y: 0.0,
                width: 0.5,
                height: 0.5,
            },
            Crop {
                x: 0.9,
                y: 0.0,
                width: 0.2,
                height: 0.5,
            },
            Crop {
                x: 0.0,
                y: 1.0,
                width: 0.5,
                height: 0.1,
            },
            Crop {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.5,
            },
            Crop {
                x: f64::NAN,
                y: 0.0,
                width: 0.5,
                height: 0.5,
            },
        ] {
            assert!(crop.pixels(100, 100).is_err());
        }
        for args in [
            serde_json::json!({"slug": "../escape", "page": 1}),
            serde_json::json!({"slug": "paper", "page": 0}),
            serde_json::json!({"slug": "paper", "page": 1, "dpi": 301}),
            serde_json::json!({"slug": "paper", "page": 1, "caption": "x".repeat(1001)}),
        ] {
            let args = serde_json::from_value(args).unwrap();
            assert!(render("/nonexistent-library", args).is_err());
        }
    }
}
