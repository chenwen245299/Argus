//! Submission destinations are not publication venues. Keep explicit status in
//! the existing venue field so old libraries/exports need no schema migration.

use regex::Regex;
use std::sync::OnceLock;

use crate::models::PaperMeta;

/// Keep in sync with `isUnpublishedVenue` in src/utils/rankBadges.ts.
pub fn is_unpublished_venue(venue: &str) -> bool {
    static STATUS: OnceLock<Regex> = OnceLock::new();
    STATUS.get_or_init(|| Regex::new(
        r"(?i)(?-u:\b)(?:under[\s-]+review|in[\s-]+review|awaiting[\s-]+decision|pending[\s-]+review|submission|submitted|withdrawn|rejected|desk[\s-]+reject(?:ed)?|not[\s-]+accepted|acceptance[\s-]+unconfirmed|pre[\s-]?print|arxiv|biorxiv|medrxiv)(?-u:\b)|在审|审稿中|待审|投稿中|已投稿|未录用|拒稿|撤稿|预印本"
    ).unwrap()).is_match(&venue.replace('_', " "))
}

pub const METADATA_PUBLICATION_RULES: &str =
    "- A submission destination is NOT a publication venue. Preserve explicit status: \
for example, 'Under review as a conference paper at ICLR 2027' means \
\"venue\": \"ICLR 2027 (under review)\", never just \"ICLR 2027\". \
Keep submitted, withdrawn and rejected qualifiers too. Anonymous authors and \
conference template names are not evidence of acceptance. If only preprint or \
under-review status is stated, use that status without inventing a venue. \
Use a bare journal/conference name only with evidence of acceptance/publication. \
A target conference's year alone is not the manuscript's publication year; \
leave year null unless a date for this document is explicitly stated.";

struct ReviewNotice {
    venue: String,
    target_year: Option<u32>,
}

/// Recognize the document's own notice, never a mention in the abstract, body
/// or reference list. PDF extractors may wrap the notice across lines.
fn review_notice(text: &str) -> Option<ReviewNotice> {
    let mut prelude = String::new();
    for line in text.lines().take(60) {
        let heading = line
            .trim()
            .trim_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ':' || c.is_whitespace())
            .to_lowercase();
        if matches!(
            heading.as_str(),
            "abstract"
                | "introduction"
                | "references"
                | "bibliography"
                | "publications"
                | "selected publications"
                | "摘要"
                | "参考文献"
        ) || [
            "abstract:",
            "abstract—",
            "abstract–",
            "abstract.",
            "abstract ",
            "摘要：",
        ]
        .iter()
        .any(|prefix| heading.starts_with(prefix))
        {
            break;
        }
        if prelude.len() + line.len() > 6000 {
            break;
        }
        prelude.push_str(line);
        prelude.push('\n');
    }
    static TARGETED: OnceLock<Regex> = OnceLock::new();
    let targeted = TARGETED.get_or_init(|| Regex::new(
        r"(?im)^[\t ]*(under[\s-]+review(?:\s+as\s+(?:(?:a|an)\s+)?(?:conference\s+paper|submission|manuscript))?\s+(?:at|to|for)|(?:manuscript\s+)?submitted\s+(?:to|for))\s+([^\r\n]{1,180})"
    ).unwrap());
    if let Some(caps) = targeted.captures(&prelude) {
        let target = caps[2].trim().trim_end_matches('.').trim();
        if !target.is_empty() {
            let status = if caps[1].to_ascii_lowercase().starts_with("under") {
                "under review"
            } else {
                "submitted"
            };
            let target_year = target.split_whitespace().find_map(|s| {
                let s = s.trim_matches(|c: char| !c.is_ascii_digit());
                (s.len() == 4)
                    .then(|| s.parse::<u32>().ok())
                    .flatten()
                    .filter(|y| (1900..=2099).contains(y))
            });
            return Some(ReviewNotice {
                venue: format!("{target} ({status})"),
                target_year,
            });
        }
    }
    static GENERIC: OnceLock<Regex> = OnceLock::new();
    let generic = GENERIC.get_or_init(|| Regex::new(
        r"(?im)^[\t ]*(?:preprint[.\t ]+)?under[\s-]+review[.\t ]*$|^[\t ]*(?:在审|审稿中|投稿中)[。\t ]*$"
    ).unwrap());
    if generic.is_match(&prelude) {
        return Some(ReviewNotice {
            venue: "Under review".into(),
            target_year: None,
        });
    }
    None
}

/// Also repairs a previously stripped status when metadata is fetched again.
/// This is based on an explicit first-page notice, not on anonymity or a year.
pub fn apply_review_notice(meta: &mut PaperMeta, text: &str) -> bool {
    let Some(notice) = review_notice(text) else {
        return false;
    };
    // The PDF may be the old submission version even after a decision. Never
    // replace an explicit final status with its older "under review" banner.
    if meta.venue.as_deref().is_some_and(has_final_decision) {
        if meta.venue.as_deref().is_some_and(is_unpublished_venue) {
            return meta.journal_rank.take().is_some();
        }
        return false;
    }
    let mut changed =
        meta.venue.as_deref() != Some(notice.venue.as_str()) || meta.journal_rank.is_some();
    meta.venue = Some(notice.venue);
    meta.journal_rank = None;
    // An upcoming conference year cannot be this document's publication year.
    // Leave actual current/past manuscript dates alone.
    use chrono::Datelike;
    if notice
        .target_year
        .is_some_and(|y| y > chrono::Utc::now().year() as u32 && meta.year == Some(y))
    {
        meta.year = None;
        changed = true;
    }
    changed
}

fn has_final_decision(venue: &str) -> bool {
    static FINAL_STATUS: OnceLock<Regex> = OnceLock::new();
    FINAL_STATUS.get_or_init(|| Regex::new(
        r"(?i)\b(?:withdrawn|rejected|desk[\s-]+reject(?:ed)?|not[\s-]+accepted|accepted|published|poster|oral|spotlight)\b|未录用|拒稿|撤稿|已录用|已发表"
    ).unwrap()).is_match(&venue.replace('_', " "))
}

/// An extractor may drop a final qualifier while reading an old submission PDF.
/// Preserve the known decision before applying the source's older review notice.
pub fn apply_metadata_venue(meta: &mut PaperMeta, incoming: Option<String>, text: &str) {
    let keep_decision =
        meta.venue.as_deref().is_some_and(has_final_decision) && review_notice(text).is_some();
    if !keep_decision {
        if let Some(venue) = incoming {
            meta.venue = Some(venue);
        }
    }
    apply_review_notice(meta, text);
}

/// Read only the cached front matter; never extract a PDF or make a request as
/// a side effect of citation enrichment.
pub fn apply_cached_review_notice(root: &str, slug: &str, meta: &mut PaperMeta) -> bool {
    use std::io::Read;
    if crate::path_guard::validate_segment("paper slug", slug).is_err()
        || meta.file_type.as_deref().is_some_and(|t| t != "pdf")
    {
        return false;
    }
    let path = crate::paper::paper_dir(root, slug).join("fulltext.txt");
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut bytes = Vec::new();
    if file.take(16_384).read_to_end(&mut bytes).is_err() {
        return false;
    }
    apply_review_notice(meta, &String::from_utf8_lossy(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> PaperMeta {
        serde_json::from_value(serde_json::json!({"id":"test", "title":"VeriSearch", "venue":"ICLR 2027", "year":2027})).unwrap()
    }

    #[test]
    fn submission_statuses_have_no_publication_rank() {
        for venue in [
            "ICLR 2027 (under review)",
            "ICLR 2027 Conference Submission",
            "Submitted to NeurIPS 2026",
            "ICLR.cc/2027/Conference/Submission",
            "ICLR.cc/2025/Conference/Withdrawn_Submission",
            "ICLR 2027 Under_Review",
            "ICLR 2027（under review中）",
            "TMLR under-review",
            "ICLR 2025 Withdrawn Submission",
            "ICLR 2025 (rejected)",
            "ICLR 2027 (在审)",
            "ICML 2026 投稿中",
            "arXiv",
            "bioRxiv preprint",
            "ICLR 2027 (acceptance unconfirmed)",
        ] {
            assert!(is_unpublished_venue(venue), "{venue}");
        }
        for venue in [
            "ICLR 2025",
            "ICLR 2025 Poster",
            "NeurIPS 2024",
            "International Conference on Learning Representations",
            "Physical Review Letters",
            "IEEE Transactions on Pattern Analysis and Machine Intelligence",
        ] {
            assert!(!is_unpublished_venue(venue), "{venue}");
        }
    }

    #[test]
    fn repairs_stripped_status_and_stale_rank_including_wrapped_banners() {
        for banner in [
            "Under review as a conference paper at ICLR 2027",
            "Under review as a conference\npaper at\nICLR 2027",
        ] {
            let mut m = meta();
            m.journal_rank = Some(serde_json::from_value(serde_json::json!({"official":{"ccf":"A"},"custom":[],"venue":"ICLR 2027","fetched_at":""})).unwrap());
            assert!(apply_review_notice(
                &mut m,
                &format!("{banner}\nVeriSearch\nAnonymous authors\nAbstract\nBody")
            ));
            assert_eq!(m.venue.as_deref(), Some("ICLR 2027 (under review)"));
            assert!(m.journal_rank.is_none());
            assert!(!apply_review_notice(&mut m, banner));
        }
    }

    #[test]
    fn submitted_and_generic_notices_do_not_invent_publication() {
        let mut m = meta();
        apply_review_notice(&mut m, "Submitted to TMLR\nA Paper\nAbstract\nText");
        assert_eq!(m.venue.as_deref(), Some("TMLR (submitted)"));
        apply_review_notice(&mut m, "A Paper\nPreprint. Under review.\nAbstract\nText");
        assert_eq!(m.venue.as_deref(), Some("Under review"));
    }

    #[test]
    fn body_references_anonymity_and_published_notices_do_not_downgrade_papers() {
        for text in ["Published as a conference paper at ICLR 2025\nA Paper\nAbstract\nBody", "A Paper\nAnonymous authors\nICLR 2027", "A Paper\nAbstract\nUnder review as a conference paper at ICLR 2027", "A Paper\nAbstract: We analyze review notices.\nUnder review as a conference paper at ICLR 2027", "A Paper\nReferences\nUnder review as a conference paper at ICLR 2027", "A Paper\nWe study papers under review at ICLR 2027."] {
            let mut m = meta();
            assert!(!apply_review_notice(&mut m, text), "{text}");
            assert_eq!(m.venue.as_deref(), Some("ICLR 2027"));
        }
    }

    #[test]
    fn an_old_pdf_does_not_replace_an_explicit_final_decision() {
        for venue in [
            "ICLR 2025 (accepted)",
            "ICLR 2025 Poster",
            "ICLR 2025 Withdrawn Submission",
            "ICLR 2025 (rejected)",
        ] {
            let mut m = meta();
            m.venue = Some(venue.into());
            apply_review_notice(&mut m, "Under review as a conference paper at ICLR 2025");
            assert_eq!(m.venue.as_deref(), Some(venue));
            apply_metadata_venue(
                &mut m,
                Some("ICLR 2025".into()),
                "Under review as a conference paper at ICLR 2025",
            );
            assert_eq!(m.venue.as_deref(), Some(venue));
        }
    }

    #[test]
    fn citation_enrichment_repairs_cached_front_matter_without_a_network_result() {
        let root = std::env::temp_dir().join(format!("argus-publication-{}", uuid::Uuid::new_v4()));
        let dir = root.join("papers").join("paper");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("fulltext.txt"), "Under review as a conference paper at ICLR 2027\nVeriSearch\nAnonymous authors\nAbstract\nBody").unwrap();
        let mut m = meta();
        let update = crate::metadata::MetaUpdate {
            venue: Some("ICLR 2027".into()),
            cite_count: Some(1),
            ..Default::default()
        };
        crate::metadata::backfill_meta(&mut m, &update);
        assert!(apply_cached_review_notice(
            root.to_str().unwrap(),
            "paper",
            &mut m
        ));
        assert_eq!(m.venue.as_deref(), Some("ICLR 2027 (under review)"));
        crate::paper::write_meta(root.to_str().unwrap(), "paper", &m).unwrap();
        let mut loaded = crate::paper::read_meta(root.to_str().unwrap(), "paper").unwrap();
        assert_eq!(loaded.venue, m.venue);
        assert!(!crate::metadata::backfill_meta(&mut loaded, &update));
        assert!(!apply_cached_review_notice(
            root.to_str().unwrap(),
            "paper",
            &mut loaded
        ));
        assert!(!apply_cached_review_notice(
            root.to_str().unwrap(),
            "../paper",
            &mut loaded
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn future_target_year_is_not_a_publication_date() {
        use chrono::Datelike;
        let future = chrono::Utc::now().year() as u32 + 1;
        let mut m = meta();
        m.year = Some(future);
        apply_review_notice(
            &mut m,
            &format!("Under review as a conference paper at ICLR {future}"),
        );
        assert_eq!(m.year, None);
        m.year = Some(future - 1);
        apply_review_notice(
            &mut m,
            &format!("Under review as a conference paper at ICLR {future}"),
        );
        assert_eq!(m.year, Some(future - 1));
    }
}
