//! Exact, page-bound PDF text selectors shared by page inspection and marking.
//! A model selects returned IDs rather than retyping lossy mathematical text.
use crate::models::Rect;
use serde::Serialize;

pub(super) type Glyph = (char, Option<Rect>);

#[derive(Debug, Clone, Serialize)]
pub(super) struct TextTarget {
    pub id: String,
    pub text: String,
    #[serde(skip)]
    start: usize,
    #[serde(skip)]
    end: usize,
}

pub(super) fn targets(page: u32, glyphs: &[Glyph]) -> Vec<TextTarget> {
    // Stable across requests and app versions; geometry binds IDs to this PDF
    // layout too. This is an opaque lookup key, never a caller-supplied offset.
    let mut hash = 0xcbf29ce484222325u64;
    for (ch, rect) in glyphs {
        for byte in (*ch as u32).to_le_bytes().into_iter().chain(
            rect.iter()
                .flat_map(|r| [r.x, r.y, r.width, r.height])
                .flat_map(f32::to_le_bytes),
        ) {
            hash = (hash ^ byte as u64).wrapping_mul(0x100000001b3);
        }
    }
    let mut out = Vec::new();
    let mut start = 0;
    for end in 0..=glyphs.len() {
        if end < glyphs.len() && !matches!(glyphs[end].0, '\r' | '\n') {
            continue;
        }
        let mut first = start;
        let mut last = end;
        while first < last && glyphs[first].0.is_whitespace() {
            first += 1;
        }
        while last > first && glyphs[last - 1].0.is_whitespace() {
            last -= 1;
        }
        if let Ok((text, _)) = locate_range(glyphs, first, last) {
            out.push(TextTarget {
                id: format!("p{page}-{hash:016x}-l{}", out.len() + 1),
                text,
                start: first,
                end: last,
            });
        }
        start = end + 1;
    }
    out
}

pub(super) fn locate_anchor(
    page: u32,
    glyphs: &[Glyph],
    anchor: &str,
    end_anchor: Option<&str>,
    quote: &str,
) -> Result<(String, Vec<Rect>), String> {
    let targets = targets(page, glyphs);
    let lookup = |id: &str| {
        targets.iter().find(|t| t.id == id).ok_or_else(||
        "Unknown or stale anchor on this PDF page. Call view_paper_page and copy an annotation_targets ID exactly.".to_string())
    };
    let start = lookup(anchor)?;
    let end = lookup(end_anchor.unwrap_or(anchor))?;
    if end.start < start.start {
        return Err("end_anchor must follow anchor on the same page.".into());
    }
    if end.end - start.start > 10_000 {
        return Err("An anchor range must select at most 10,000 characters.".into());
    }
    let selected = &glyphs[start.start..end.end];
    if !quote.trim().is_empty() {
        // A quote can narrow a line/range, but must still be exact and unique
        // inside it. Never ignore a conflicting quote just because IDs match.
        locate_passage(selected, quote)
    } else {
        locate_range(glyphs, start.start, end.end)
    }
}

/// Ignore PDF line wrapping, discretionary hyphens and common font ligatures,
/// while retaining a map back to the actual glyphs. Never fuzzy-match a quote.
fn normalized_chars(chars: &[char]) -> Vec<(char, usize)> {
    let mut out = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_whitespace() || c == '\u{ad}' {
            continue;
        }
        if c == '-'
            && chars.get(i + 1).is_some_and(|c| matches!(c, '\r' | '\n'))
            && i > 0
            && chars[i - 1].is_alphabetic()
        {
            continue;
        }
        let expanded = match c {
            'ﬀ' => "ff",
            'ﬁ' => "fi",
            'ﬂ' => "fl",
            'ﬃ' => "ffi",
            'ﬄ' => "ffl",
            _ => "",
        };
        if expanded.is_empty() {
            out.push((c, i));
        } else {
            out.extend(expanded.chars().map(|c| (c, i)));
        }
    }
    out
}

pub(super) fn locate_passage(
    glyphs: &[(char, Option<crate::models::Rect>)],
    quote: &str,
) -> Result<(String, Vec<crate::models::Rect>), String> {
    let chars: Vec<char> = glyphs.iter().map(|(c, _)| *c).collect();
    let source = normalized_chars(&chars);
    let target = normalized_chars(&quote.chars().collect::<Vec<_>>());
    if target.is_empty() || target.len() > source.len() {
        return Err(
            "Original text was not found. Read this PDF page and quote its text exactly.".into(),
        );
    }
    let matches: Vec<usize> = source
        .windows(target.len())
        .enumerate()
        .filter_map(|(i, w)| w.iter().zip(&target).all(|(a, b)| a.0 == b.0).then_some(i))
        .collect();
    if matches.len() != 1 {
        return Err(if matches.is_empty() {
            "Original text was not found. Quote the page text exactly."
        } else {
            "This text occurs more than once on the page. Include a longer, unique excerpt."
        }
        .into());
    }
    let start = source[matches[0]].1;
    let end = source[matches[0] + target.len() - 1].1;
    locate_range(glyphs, start, end + 1)
}

fn locate_range(glyphs: &[Glyph], start: usize, end: usize) -> Result<(String, Vec<Rect>), String> {
    if start >= end || end > glyphs.len() || end - start > 10_000 {
        return Err("An anchor range must select 1–10,000 characters in page order.".into());
    }
    let mut rects: Vec<Rect> = Vec::new();
    for (ch, rect) in &glyphs[start..end] {
        if ch.is_whitespace() || *ch == '\u{ad}' {
            continue;
        }
        let r = rect
            .as_ref()
            .filter(|r| {
                [r.x, r.y, r.width, r.height].iter().all(|v| v.is_finite())
                    && r.width > 0.0
                    && r.height > 0.0
            })
            .ok_or("The selected text has no reliable glyph geometry; nothing was marked.")?;
        if let Some(last) = rects.last_mut() {
            if (last.y - r.y).abs() < r.height * 0.25
                && (last.height - r.height).abs() < r.height * 0.35
                && r.x >= last.x
                && r.x <= last.x + last.width + r.height * 0.8
            {
                last.width = (r.x + r.width).max(last.x + last.width) - last.x;
                last.height = (r.y + r.height).max(last.y + last.height) - last.y;
                continue;
            }
        }
        rects.push(r.clone());
    }
    if rects.is_empty() {
        return Err("No selectable PDF text in this passage.".into());
    }
    Ok((glyphs[start..end].iter().map(|(c, _)| c).collect(), rects))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glyphs(text: &str) -> Vec<Glyph> {
        let (mut x, mut y) = (0.0, 10.0);
        text.chars()
            .map(|c| {
                if c == '\n' {
                    x = 0.0;
                    y += 20.0;
                }
                let rect = (!c.is_whitespace()).then_some(Rect {
                    x,
                    y,
                    width: 7.0,
                    height: 12.0,
                });
                x += 8.0;
                (c, rect)
            })
            .collect()
    }

    #[test]
    fn page_targets_select_real_math_glyphs_and_disambiguate_repeated_text() {
        let glyphs = glyphs("Repeat\r\nmˆ = ∑ x² + \u{10}a\u{11}\r\nRepeat");
        let targets = targets(16, &glyphs);
        assert_eq!(targets.len(), 3);
        assert!(locate_passage(&glyphs, "Repeat")
            .unwrap_err()
            .contains("more than once"));
        let (_, first) = locate_anchor(16, &glyphs, &targets[0].id, None, "Repeat").unwrap();
        let (_, last) = locate_anchor(16, &glyphs, &targets[2].id, None, "Repeat").unwrap();
        assert!(first[0].y < last[0].y);
        assert_eq!(
            locate_anchor(16, &glyphs, &targets[1].id, None, "")
                .unwrap()
                .0,
            "mˆ = ∑ x² + \u{10}a\u{11}"
        );
        assert_eq!(
            locate_anchor(16, &glyphs, &targets[0].id, Some(&targets[2].id), "")
                .unwrap()
                .0,
            "Repeat\r\nmˆ = ∑ x² + \u{10}a\u{11}\r\nRepeat"
        );
        let serialized = serde_json::to_value(&targets).unwrap();
        assert!(
            serialized[0].get("start").is_none(),
            "offsets are not model-selected geometry"
        );
    }

    #[test]
    fn forged_stale_wrong_page_reversed_and_conflicting_selectors_are_refused() {
        let mut glyphs = glyphs("Alpha\nBeta");
        let targets = targets(1, &glyphs);
        assert!(locate_anchor(1, &glyphs, "made-up", None, "").is_err());
        assert!(locate_anchor(2, &glyphs, &targets[0].id, None, "").is_err());
        assert!(locate_anchor(1, &glyphs, &targets[1].id, Some(&targets[0].id), "").is_err());
        assert!(locate_anchor(1, &glyphs, &targets[0].id, None, "Beta").is_err());
        glyphs[0].1.as_mut().unwrap().x += 1.0;
        assert!(
            locate_anchor(1, &glyphs, &targets[0].id, None, "").is_err(),
            "changed layout invalidates IDs"
        );
        glyphs[0].0 = 'Z';
        assert!(locate_anchor(1, &glyphs, &targets[0].id, None, "").is_err());
    }

    #[test]
    fn targets_never_offer_unreliable_geometry_and_ranges_cannot_cross_it() {
        let mut source = glyphs("Alpha\nMissing\nBeta");
        source[6].1 = None;
        let targets = targets(1, &source);
        assert_eq!(
            targets.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["Alpha", "Beta"]
        );
        assert!(locate_anchor(1, &source, &targets[0].id, Some(&targets[1].id), "").is_err());
        assert!(locate_passage(&source, "Missing").is_err());
        assert!(super::targets(1, &[('a', None)]).is_empty());
        assert!(super::targets(1, &glyphs(" ")).is_empty());
    }
}
