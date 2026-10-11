//! Render a paper page (or a crop of it) for delivery in an in-app answer.
//! No arbitrary paths, URLs or model-supplied image bytes are accepted. This
//! layer only reads the paper; copilot saves the result with the conversation.

use base64::Engine;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ImageEncoder};
use serde::{Deserialize, Serialize};

pub const SEND_PAPER_IMAGE_TOOL: &str = "send_paper_image";

/// Fractions of the displayed, rotated page, measured from its top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Crop {
    pub fn validate(self) -> Result<(), String> {
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

/// Successful crop previews from this answer only. A preview produced in the
/// current tool batch has not reached the model yet, so it cannot authorize a
/// send in that same batch. DPI may change; normalized page geometry may not.
#[derive(Default)]
pub struct CropPreviews(Vec<(String, u32, Crop, usize)>);

impl CropPreviews {
    pub fn remember(&mut self, value: &serde_json::Value, round: usize) {
        let Some(slug) = value["slug"].as_str() else {
            return;
        };
        for page in value["pages"].as_array().into_iter().flatten() {
            if let (Some(n), Ok(crop)) = (
                page["page"].as_u64(),
                serde_json::from_value::<Crop>(page["crop"].clone()),
            ) {
                if crop.validate().is_ok() {
                    self.0.push((slug.into(), n as u32, crop, round));
                }
            }
        }
    }

    pub fn check_send(
        &self,
        args: &serde_json::Value,
        vision: bool,
        round: usize,
    ) -> Result<(), String> {
        let args: SendImageArgs =
            serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
        args.validate()?;
        let Some(crop) = args.crop else {
            return Ok(());
        };
        if !vision {
            return Err("This model cannot inspect crops. Omit crop to send the full page, or use a vision-capable model.".into());
        }
        let matches = |a: Crop, b: Crop| {
            [a.x - b.x, a.y - b.y, a.width - b.width, a.height - b.height]
                .iter()
                .all(|delta| delta.abs() < 1e-9)
        };
        if self.0.iter().any(|(slug, page, preview, seen)| {
            slug == &args.slug && *page == args.page && *seen < round && matches(*preview, crop)
        }) {
            Ok(())
        } else {
            Err("Crop not previewed. Call view_paper_page with this slug, pages:[page] and the EXACT same crop, then WAIT for the image. Verify the intended figure number/content and that axes, legends and caption are complete; adjust and preview again if needed. Only send after inspecting that result.".into())
        }
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

pub struct PageImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub page_width: u32,
    pub page_height: u32,
}

fn encode_image(image: &DynamicImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(&mut bytes, CompressionType::Fast, FilterType::Up)
        .write_image(
            image.as_bytes(),
            image.width(),
            image.height(),
            image.color().into(),
        )
        .map_err(|e| format!("Encode page image: {e}"))?;
    Ok(bytes)
}

/// Preview and delivery use the very same crop conversion, even at different DPI.
pub fn prepare_page_png(png: Vec<u8>, crop: Option<Crop>) -> Result<PageImage, String> {
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .map_err(|e| format!("Decode rendered page: {e}"))?;
    let (page_width, page_height) = (image.width(), image.height());
    let image = crop_image(image, crop)?;
    Ok(PageImage {
        png: if crop.is_some() {
            encode_image(&image)?
        } else {
            png
        },
        width: image.width(),
        height: image.height(),
        page_width,
        page_height,
    })
}

/// A faint, labelled 0–1 grid for locating figures on a full-page inspection.
/// The image keeps the page's exact dimensions and origin. Delivered images
/// and crop previews always use the clean pixels instead.
pub fn coordinate_grid(png: &[u8]) -> Result<Vec<u8>, String> {
    let mut image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| format!("Decode inspection image: {e}"))?
        .to_rgba8();
    let (w, h) = image.dimensions();
    let scale = (w / 600).clamp(1, 4);
    for tick in 0..=10 {
        let x = ((w - 1) as f64 * tick as f64 / 10.0).round() as u32;
        let y = ((h - 1) as f64 * tick as f64 / 10.0).round() as u32;
        for yy in 0..h {
            if yy % 8 < 3 {
                tint_grid_pixel(&mut image, x, yy);
            }
        }
        for xx in 0..w {
            if xx % 8 < 3 {
                tint_grid_pixel(&mut image, xx, y);
            }
        }
        let label = format!("{:.1}", tick as f64 / 10.0);
        let label_w = 18 * scale;
        let label_h = 9 * scale;
        draw_grid_label(
            &mut image,
            x.min(w.saturating_sub(label_w)),
            2 * scale,
            &label,
            scale,
        );
        draw_grid_label(
            &mut image,
            2 * scale,
            y.min(h.saturating_sub(label_h)),
            &label,
            scale,
        );
    }
    encode_image(&DynamicImage::ImageRgba8(image))
}

fn tint_grid_pixel(image: &mut image::RgbaImage, x: u32, y: u32) {
    let pixel = image.get_pixel_mut(x, y);
    for (channel, grid) in pixel.0[..3].iter_mut().zip([30u16, 100, 190]) {
        *channel = ((*channel as u16 * 7 + grid) / 8) as u8;
    }
}

fn draw_grid_label(image: &mut image::RgbaImage, x: u32, y: u32, label: &str, scale: u32) {
    // A tiny bitmap font avoids adding a font library or platform font dependency.
    const ZERO: [u8; 7] = [14, 17, 19, 21, 25, 17, 14];
    const ONE: [u8; 7] = [4, 12, 4, 4, 4, 4, 14];
    let (w, h) = image.dimensions();
    for yy in y..(y + 9 * scale).min(h) {
        for xx in x..(x + label.len() as u32 * 6 * scale).min(w) {
            image.put_pixel(xx, yy, image::Rgba([235, 245, 255, 255]));
        }
    }
    for (i, ch) in label.bytes().enumerate() {
        let rows = match ch {
            b'0' => ZERO,
            b'1' => ONE,
            b'2' => [14, 17, 1, 2, 4, 8, 31],
            b'3' => [30, 1, 1, 14, 1, 1, 30],
            b'4' => [2, 6, 10, 18, 31, 2, 2],
            b'5' => [31, 16, 16, 30, 1, 1, 30],
            b'6' => [14, 16, 16, 30, 17, 17, 14],
            b'7' => [31, 1, 2, 4, 8, 8, 8],
            b'8' => [14, 17, 17, 14, 17, 17, 14],
            b'9' => [14, 17, 17, 15, 1, 1, 14],
            _ => [0, 0, 0, 0, 0, 6, 6],
        };
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5u32 {
                if bits & (1 << (4 - col)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let xx = x + (i as u32 * 6 + col) * scale + dx;
                        let yy = y + (row as u32 + 1) * scale + dy;
                        if xx < w && yy < h {
                            image.put_pixel(xx, yy, image::Rgba([20, 80, 150, 255]));
                        }
                    }
                }
            }
        }
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
    let image = prepare_page_png(png, args.crop)?;
    Ok(serde_json::json!({
        "slug": args.slug,
        "pages": [{
            "page": args.page,
            "caption": args.caption.unwrap_or_default().trim(),
            "crop": args.crop,
            "dpi": dpi,
            "width": image.width,
            "height": image.height,
            "page_width": image.page_width,
            "page_height": image.page_height,
            "png_base64": base64::engine::general_purpose::STANDARD.encode(image.png),
        }],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cropped_sends_require_an_identical_preview_from_an_earlier_round() {
        let mut previews = CropPreviews::default();
        let args = serde_json::json!({"slug":"paper", "page":5,
            "crop":{"x":0.1,"y":0.2,"width":0.8,"height":0.3}});
        assert!(previews.check_send(&args, true, 1).is_err());
        previews.remember(
            &serde_json::json!({"slug":"paper", "pages":[{
                "page":5, "crop":args["crop"]
            }]}),
            1,
        );
        assert!(
            previews.check_send(&args, true, 1).is_err(),
            "same-batch preview was not seen yet"
        );
        assert!(previews.check_send(&args, true, 2).is_ok());
        assert!(
            previews.check_send(&args, false, 2).is_err(),
            "a text-only model cannot inspect the crop"
        );
        for (key, changed) in [
            ("slug", serde_json::json!("other")),
            ("page", serde_json::json!(6)),
            (
                "crop",
                serde_json::json!({"x":0.1,"y":0.3,"width":0.8,"height":0.3}),
            ),
        ] {
            let mut other = args.clone();
            other[key] = changed;
            assert!(previews.check_send(&other, true, 2).is_err());
        }
        let mut clearer = args.clone();
        clearer["dpi"] = serde_json::json!(300);
        assert!(previews.check_send(&clearer, true, 2).is_ok());
        assert!(CropPreviews::default()
            .check_send(&serde_json::json!({"slug":"paper","page":5}), false, 1)
            .is_ok());
    }

    #[test]
    fn inspection_grid_preserves_dimensions_and_clean_crop_pixels() {
        let image = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            600,
            800,
            image::Rgba([255, 255, 255, 255]),
        ));
        let clean = encode_image(&image).unwrap();
        let gridded = image::load_from_memory(&coordinate_grid(&clean).unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(gridded.dimensions(), (600, 800));
        assert_ne!(
            gridded.get_pixel(300, 402).0,
            [255, 255, 255, 255],
            "grid x=0.5 is misplaced"
        );
        let cropped = prepare_page_png(
            clean,
            Some(Crop {
                x: 0.25,
                y: 0.25,
                width: 0.5,
                height: 0.5,
            }),
        )
        .unwrap();
        assert_eq!((cropped.width, cropped.height), (300, 400));
        assert_eq!((cropped.page_width, cropped.page_height), (600, 800));
        assert!(image::load_from_memory(&cropped.png)
            .unwrap()
            .to_rgba8()
            .pixels()
            .all(|p| p.0 == [255, 255, 255, 255]));
    }

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
        // Preview and delivery must select exactly the same clean pixels.
        // Repeat on a rotated PDF so page orientation cannot drift between them.
        for rotation in [0, 90, 180, 270] {
            pdf.get_object_mut(page_id)
                .unwrap()
                .as_dict_mut()
                .unwrap()
                .set("Rotate", rotation);
            pdf.save(paper.join("paper.pdf")).unwrap();
            let crop = serde_json::json!({"x":0.1,"y":0.1,"width":0.8,"height":0.3});
            let preview = super::super::agent::call(
                root.to_str().unwrap(),
                "view_paper_page",
                &serde_json::json!({"slug":"figure-paper", "pages":[1], "crop":crop}),
            )
            .unwrap();
            let sent = super::super::agent::call(
                root.to_str().unwrap(),
                SEND_PAPER_IMAGE_TOOL,
                &serde_json::json!({"slug":"figure-paper", "page":1, "crop":crop}),
            )
            .unwrap();
            let pixels = |value: &serde_json::Value| {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(value["pages"][0]["png_base64"].as_str().unwrap())
                    .unwrap();
                image::load_from_memory(&bytes).unwrap().to_rgba8()
            };
            assert_eq!(pixels(&preview), pixels(&sent), "rotation {rotation}");
            assert_eq!(preview["pages"][0]["coordinate_grid"], false);
        }
        assert!(super::super::agent::call(root.to_str().unwrap(), "view_paper_page",
            &serde_json::json!({"slug":"figure-paper", "pages":[1,2], "crop":{"x":0.0,"y":0.0,"width":1.0,"height":1.0}})).is_err());
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
