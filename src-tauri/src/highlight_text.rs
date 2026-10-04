//! The text of a highlight as it should READ — one paragraph, not one fragment
//! per printed line.
//!
//! A PDF has no paragraphs, only positioned lines, so a selection comes back with
//! a line break at every wrap. Left as is, an exported Markdown quote becomes
//! `> line` × N, and the agent tools and the embedding chunks carry the wrap
//! points, which cuts one sentence into shards.
//!
//! [`Highlight::text`] therefore stays EXACTLY as captured — it is the source of
//! truth, and what `keep_line_breaks` shows — and every read-only consumer goes
//! through [`display_text`]. Nothing is rewritten on disk, so a library full of
//! old highlights is fixed the moment this ships, and an older build (which drops
//! the `keep_line_breaks` flag it does not know) merely merges again.
//!
//! The TypeScript twin is `src/utils/highlightText.ts`. The two must behave
//! identically, character for character — keep every rule below in step with it:
//!
//! * Lines split on `\r\n` or any of `\n`, `\r`, U+2028, U+2029. (`\r\n` is
//!   treated as two separators here: the empty segment between them is blank and
//!   dropped, exactly as the twin drops the empty line a literal `\r\n` never
//!   produces, so the outcome is the same.)
//! * Each line is trimmed with an EXPLICIT whitespace set ([`is_space`]) — not
//!   `str::trim`, which differs from JavaScript's `trim()` at U+0085 (Rust strips
//!   it, JS does not) and U+FEFF (JS strips it, Rust does not).
//! * Blank lines are dropped.
//! * Joining the accumulated text `acc` with the next line: a soft hyphen at the
//!   end of `acc` is dropped and the lines join directly; else if the last
//!   character of `acc` OR the first of the next line is CJK ([`is_cjk`]; Hangul
//!   is not) they join with no space; else if `acc` ends in a dash ([`is_dash`])
//!   that has a non-whitespace character before it they join with no space and
//!   the hyphen is KEPT; else they join with one ASCII space.
//!
//! The twin works in UTF-16 and takes the last/first CODE POINT; a Rust `char` is
//! already one, so astral characters (U+20000 is CJK extension B, U+1F600 is not)
//! agree by construction.
//!
//! Pure: nothing here reads or writes the library.

use crate::models::Highlight;

/// Characters that count as whitespace here. Spelled out rather than
/// `char::is_whitespace` / `str::trim` so it is the very set the TypeScript twin
/// uses (JavaScript's `\s`/`trim()`: their built-ins differ at U+0085 and U+FEFF).
fn is_space(c: char) -> bool {
    matches!(
        c,
        '\u{09}'..='\u{0d}'
            | '\u{20}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// Han, kana, bopomofo, CJK punctuation and full-width forms: scripts written
/// with no spaces between words, so a wrap there must be rejoined without one.
/// Hangul is left out on purpose — Korean separates words with spaces, and the
/// wrap replaced one.
fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{2e80}'..='\u{2fdf}'
            | '\u{3000}'..='\u{303f}'
            | '\u{3040}'..='\u{30ff}'
            | '\u{3100}'..='\u{312f}'
            | '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{ff00}'..='\u{ffef}'
            | '\u{20000}'..='\u{2fa1f}'
    )
}

/// Hyphen and dashes a line can end on: `-` U+2010 U+2011 U+2013 U+2014.
fn is_dash(c: char) -> bool {
    matches!(c, '\u{2d}' | '\u{2010}' | '\u{2011}' | '\u{2013}' | '\u{2014}')
}

const SOFT_HYPHEN: char = '\u{ad}';

/// The lines of `text`, split on `\r\n`, `\n`, `\r`, U+2028 and U+2029 (see the
/// module docs for why splitting `\r\n` as two separators is equivalent).
fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
}

fn trim_space(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// Whether `text` has a line break worth a toggle: more than one non-blank line.
///
/// The twin of `hasLineBreaks`. Nothing on the Rust side needs it yet — the
/// export, the agent tools and the embedding chunks only read the merged text —
/// but it is part of the rule the two share, and the tests pin it.
#[allow(dead_code)]
pub fn has_line_breaks(text: &str) -> bool {
    split_lines(text).filter(|line| !trim_space(line).is_empty()).nth(1).is_some()
}

/// Rejoin the lines of a wrapped selection into one paragraph.
///
/// * Lines are trimmed and blank ones dropped: a PDF selection cannot tell a wrap
///   from a paragraph break, and this is the "one highlight, one paragraph" view.
///   (A list, code or an equation is what "keep line breaks" is for.)
/// * Between two lines: a single space — except with no space at all when either
///   side of the break is CJK, or when the line ends in a soft hyphen (dropped),
///   or in a hyphen / dash that hugs the word before it (`long-` + `term` ->
///   `long-term`, `COVID-` + `19` -> `COVID-19`).
/// * The hyphen is KEPT: telling a wrapped word (`sen-`/`tence`) from a compound
///   (`long-`/`term`) needs a dictionary, and deleting a real hyphen is worse than
///   leaving a visible one. `keep_line_breaks` still shows the original.
pub fn merge_wrapped_lines(text: &str) -> String {
    let mut acc = String::with_capacity(text.len());
    for raw in split_lines(text) {
        let line = trim_space(raw);
        let Some(first) = line.chars().next() else { continue };
        let Some(last) = acc.chars().next_back() else {
            acc.push_str(line);
            continue;
        };

        if last == SOFT_HYPHEN {
            acc.pop();
            acc.push_str(line);
        } else if is_cjk(last) || is_cjk(first) {
            acc.push_str(line);
        } else if is_dash(last) && dash_hugs_word(&acc) {
            acc.push_str(line);
        } else {
            acc.push(' ');
            acc.push_str(line);
        }
    }
    acc
}

/// `acc` ends in a dash that has a character before it, and that character is
/// not whitespace (so a spaced dash like `this -` stays a separate token).
fn dash_hugs_word(acc: &str) -> bool {
    let mut back = acc.chars().rev();
    back.next(); // the dash itself
    back.next().is_some_and(|before| !is_space(before))
}

/// What a highlight shows and exports: the merged paragraph, unless the user
/// chose to keep the original line breaks. Ebook records (they carry offsets) are
/// returned untouched — there a newline is a real paragraph boundary, not a wrap.
pub fn display_text(h: &Highlight) -> String {
    if h.start_offset.is_some() || h.end_offset.is_some() {
        return h.text.clone();
    }
    if h.keep_line_breaks == Some(true) {
        return h.text.clone();
    }
    merge_wrapped_lines(&h.text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Rect;

    /// Every case of the TypeScript test list, with the same expected output.
    #[test]
    fn merge_matches_the_typescript_cases() {
        let cases: &[(&str, &str)] = &[
            // Plain wrapping.
            ("the quick brown\nfox jumps over\nthe lazy dog", "the quick brown fox jumps over the lazy dog"),
            ("a\r\nb\rc\u{2028}d\u{2029}e", "a b c d e"),
            ("  a  \n\n   \n  b\t\n", "a b"),
            ("one  two", "one  two"),
            ("", ""),
            (" \n \n", ""),
            // CJK on either side of the break: no space.
            ("这是一个很长的\n句子被换行了", "这是一个很长的句子被换行了"),
            ("结果如下：\n第一项，\n第二项", "结果如下：第一项，第二项"),
            ("使用 BERT\n模型", "使用 BERT模型"),
            ("模型\nBERT 结果", "模型BERT 结果"),
            ("これは\nテストです", "これはテストです"),
            // Hangul is not CJK here: Korean separates words with spaces.
            ("안녕하세요\n세계", "안녕하세요 세계"),
            // A hyphen that hugs the word before it: joined, hyphen kept.
            ("a long-\nterm plan", "a long-term plan"),
            ("COVID-\n19", "COVID-19"),
            ("sen-\ntence", "sen-tence"),
            // A spaced or leading dash is a token of its own.
            ("this -\nthat", "this - that"),
            ("-\nx", "- x"),
            ("10–\n20", "10–20"),
            ("well—\nknown", "well—known"),
            ("a\u{2010}\nb", "a\u{2010}b"),
            ("a\u{2011}\nb", "a\u{2011}b"),
            // A soft hyphen is dropped.
            ("sen\u{ad}\ntence", "sentence"),
            // The explicit whitespace set.
            ("\u{a0}a\u{3000}\n\u{feff}b", "a b"),
            // Astral code points: U+20000 is CJK extension B, U+1F600 is not.
            ("a\u{20000}\nb", "a\u{20000}b"),
            ("a\u{1F600}\nb", "a\u{1F600} b"),
        ];
        for (input, expected) in cases {
            assert_eq!(merge_wrapped_lines(input), *expected, "input {input:?}");
        }
    }

    #[test]
    fn has_line_breaks_needs_two_non_blank_lines() {
        assert!(!has_line_breaks("a"));
        assert!(!has_line_breaks("a\n"));
        assert!(!has_line_breaks("a\n\n"));
        assert!(has_line_breaks("a\nb"));
        assert!(has_line_breaks("\n a \n\n b"));
        // Nothing at all, or only blanks, is not a break either.
        assert!(!has_line_breaks(""));
        assert!(!has_line_breaks(" \n \n"));
    }

    /// JavaScript's `trim()` keeps U+0085 and strips U+FEFF; Rust's `str::trim`
    /// does the opposite. The explicit set must follow JavaScript.
    #[test]
    fn u0085_is_not_whitespace_and_not_a_line_break() {
        assert_eq!(merge_wrapped_lines("a\u{85}\nb"), "a\u{85} b");
        assert!(!is_space('\u{85}'));
        assert!(is_space('\u{feff}'));
        // Not trimmed at the edges of a line...
        assert_eq!(merge_wrapped_lines("\u{85}a\u{85}"), "\u{85}a\u{85}");
        // ...and alone it is one (non-blank) line, not a break.
        assert!(!has_line_breaks("\u{85}"));
        assert!(!has_line_breaks("a\u{85}b"));
        assert_eq!(merge_wrapped_lines("a\u{85}b"), "a\u{85}b");
        // A line of only U+0085 is a real line, so it is kept and joined.
        assert_eq!(merge_wrapped_lines("a\n\u{85}\nb"), "a \u{85} b");
        // The BOM is trimmed, as JavaScript does.
        assert_eq!(merge_wrapped_lines("\u{feff}a\u{feff}"), "a");
    }

    #[test]
    fn the_whitespace_set_is_exactly_the_spelled_out_one() {
        let spaces = [
            '\u{09}', '\u{0a}', '\u{0b}', '\u{0c}', '\u{0d}', '\u{20}', '\u{a0}', '\u{1680}',
            '\u{2000}', '\u{2005}', '\u{200a}', '\u{2028}', '\u{2029}', '\u{202f}', '\u{205f}',
            '\u{3000}', '\u{feff}',
        ];
        for c in spaces {
            assert!(is_space(c), "{:04x} should be whitespace", c as u32);
        }
        // Neighbours of the ranges, and the zero-width characters JS keeps.
        for c in ['\u{08}', '\u{0e}', '\u{1f}', '\u{21}', '\u{85}', '\u{9f}', '\u{a1}', '\u{180e}',
                  '\u{1fff}', '\u{200b}', '\u{200c}', '\u{200d}', '\u{200e}', '\u{202e}',
                  '\u{2060}', '\u{2fff}', '\u{3001}', '\u{fefe}', '\u{ff}'] {
            assert!(!is_space(c), "{:04x} should not be whitespace", c as u32);
        }
    }

    #[test]
    fn cjk_ranges_include_their_ends_and_leave_hangul_out() {
        for c in ['\u{2e80}', '\u{2fdf}', '\u{3000}', '\u{303f}', '\u{3040}', '\u{30ff}',
                  '\u{3100}', '\u{312f}', '\u{3400}', '\u{4dbf}', '\u{4e00}', '\u{9fff}',
                  '\u{f900}', '\u{faff}', '\u{ff00}', '\u{ffef}', '\u{20000}', '\u{2fa1f}'] {
            assert!(is_cjk(c), "{:04x} should be CJK", c as u32);
        }
        for c in ['\u{2e7f}', '\u{2fe0}', '\u{3130}', '\u{33ff}', '\u{4dc0}', '\u{a000}',
                  '\u{f8ff}', '\u{fb00}', '\u{feff}', '\u{fff0}', '\u{1ffff}', '\u{2fa20}',
                  '\u{ac00}', '\u{1100}', '\u{d7a3}', 'a', '1', '.'] {
            assert!(!is_cjk(c), "{:04x} should not be CJK", c as u32);
        }
    }

    #[test]
    fn dash_rules() {
        // Every dash the rule names joins without a space...
        for d in ['-', '\u{2010}', '\u{2011}', '\u{2013}', '\u{2014}'] {
            assert_eq!(merge_wrapped_lines(&format!("ab{d}\ncd")), format!("ab{d}cd"), "{d:?}");
        }
        // ...any other does not (U+2012 figure dash, U+2015 horizontal bar, minus sign).
        for d in ['\u{2012}', '\u{2015}', '\u{2212}'] {
            assert_eq!(merge_wrapped_lines(&format!("ab{d}\ncd")), format!("ab{d} cd"), "{d:?}");
        }
        // A dash preceded by a non-breaking space or ideographic space is spaced.
        assert_eq!(merge_wrapped_lines("a\u{a0}-\nb"), "a\u{a0}- b");
        // A dash behind an astral character hugs it (the character before is not space).
        assert_eq!(merge_wrapped_lines("a\u{1F600}-\nb"), "a\u{1F600}-b");
        // Only the LAST character decides: a dash mid-line is irrelevant.
        assert_eq!(merge_wrapped_lines("a-b\nc"), "a-b c");
        // The dash test looks at the accumulated text, so it carries across lines.
        assert_eq!(merge_wrapped_lines("one\nlong-\nterm"), "one long-term");
    }

    #[test]
    fn soft_hyphen_rules() {
        // Dropped even when it is the only character so far.
        assert_eq!(merge_wrapped_lines("\u{ad}\nb"), "b");
        // Takes priority over a CJK neighbour (and is dropped).
        assert_eq!(merge_wrapped_lines("模\u{ad}\n型"), "模型");
        // A soft hyphen mid-line is left alone.
        assert_eq!(merge_wrapped_lines("a\u{ad}b\nc"), "a\u{ad}b c");
        // A trailing soft hyphen on the last line is left alone (nothing to join).
        assert_eq!(merge_wrapped_lines("a\nb\u{ad}"), "a b\u{ad}");
        // Consecutive soft-hyphenated lines.
        assert_eq!(merge_wrapped_lines("in\u{ad}\nter\u{ad}\nna\u{ad}\ntional"), "international");
    }

    #[test]
    fn the_cjk_check_comes_before_the_dash_check() {
        assert_eq!(merge_wrapped_lines("模型-\nBERT"), "模型-BERT");
        assert_eq!(merge_wrapped_lines("BERT-\n模型"), "BERT-模型");
        // A spaced dash alone keeps the space (see `dash_rules`); with a CJK
        // neighbour the CJK rule wins first and the space goes.
        assert_eq!(merge_wrapped_lines("a -\nb"), "a - b");
        assert_eq!(merge_wrapped_lines("a -\n模型"), "a -模型");
    }

    #[test]
    fn line_terminators_are_all_recognised() {
        assert_eq!(merge_wrapped_lines("a\r\n\r\nb"), "a b");
        assert_eq!(merge_wrapped_lines("a\n\rb"), "a b");
        assert_eq!(merge_wrapped_lines("a\u{2028}\u{2029}b"), "a b");
        assert_eq!(merge_wrapped_lines("a\u{b}b"), "a\u{b}b");
        // U+000B / U+000C are whitespace, not line breaks: they trim at a line's
        // edges but do not split a line.
        assert_eq!(merge_wrapped_lines("\u{b}a\u{c}\nb"), "a b");
        assert!(!has_line_breaks("a\u{b}b"));
        assert!(has_line_breaks("a\r\nb"));
        assert!(has_line_breaks("a\u{2028}b"));
    }

    #[test]
    fn a_single_line_is_returned_trimmed_and_otherwise_untouched() {
        assert_eq!(merge_wrapped_lines("  plain text, no wrap. "), "plain text, no wrap.");
        assert_eq!(merge_wrapped_lines("keeps  inner   spacing"), "keeps  inner   spacing");
        assert_eq!(merge_wrapped_lines("已经是一行。"), "已经是一行。");
        assert_eq!(merge_wrapped_lines("a-"), "a-");
    }

    fn record(text: &str) -> Highlight {
        Highlight {
            id: "h".into(),
            page: 1,
            rects: vec![Rect { x: 0.1, y: 0.5, width: 0.3, height: 0.02 }],
            text: text.into(),
            color: "#ffd400".into(),
            note: None,
            created_at: "2026-03-01T10:00:00.000Z".into(),
            updated_at: None,
            style: "highlight".into(),
            start_offset: None,
            end_offset: None,
            anchor_prefix: None,
            anchor_suffix: None,
            keep_line_breaks: None,
        }
    }

    #[test]
    fn display_text_merges_unless_the_user_keeps_the_breaks() {
        let raw = "the quick brown\nfox jumps over\nthe lazy dog";
        let mut h = record(raw);
        // Absent flag = merged: what every legacy highlight gets.
        assert_eq!(display_text(&h), "the quick brown fox jumps over the lazy dog");
        h.keep_line_breaks = Some(false);
        assert_eq!(display_text(&h), "the quick brown fox jumps over the lazy dog");
        // Kept: the text as captured, byte for byte (untrimmed, blank lines and all).
        h.keep_line_breaks = Some(true);
        assert_eq!(display_text(&h), raw);
        h.text = "  a \n\n b ".into();
        assert_eq!(display_text(&h), "  a \n\n b ");
        // The captured text is never altered.
        let h = record(raw);
        let _ = display_text(&h);
        assert_eq!(h.text, raw);
    }

    #[test]
    fn display_text_leaves_ebook_records_alone() {
        // A newline in an ebook highlight is a real paragraph boundary.
        let raw = "first paragraph\n\nsecond paragraph";
        for (start, end) in [(Some(3), Some(40)), (Some(3), None), (None, Some(40))] {
            let mut h = record(raw);
            h.start_offset = start;
            h.end_offset = end;
            assert_eq!(display_text(&h), raw);
            h.keep_line_breaks = Some(false);
            assert_eq!(display_text(&h), raw);
        }
    }

    #[test]
    fn keep_line_breaks_is_omitted_from_json_unless_set() {
        let h = record("a\nb");
        let json = serde_json::to_value(&h).unwrap();
        assert!(json.get("keep_line_breaks").is_none(), "{json}");

        let mut kept = record("a\nb");
        kept.keep_line_breaks = Some(true);
        let json = serde_json::to_value(&kept).unwrap();
        assert_eq!(json["keep_line_breaks"], true);

        // A record written before the field existed reads back as "merge".
        let legacy = r##"{"id":"h","page":1,"rects":[],"text":"a\nb","color":"#ffd400","note":null,"created_at":"2026-03-01T10:00:00.000Z","style":"highlight"}"##;
        let parsed: Highlight = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.keep_line_breaks, None);
        assert_eq!(display_text(&parsed), "a b");
        let back: Highlight = serde_json::from_value(serde_json::to_value(&kept).unwrap()).unwrap();
        assert_eq!(back.keep_line_breaks, Some(true));
    }
}
