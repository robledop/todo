//! Conversion between a task note's plain-text editing form and the HTML body the
//! Microsoft To Do API stores. Graph returns a note as `text` (app-authored) or
//! `html` (touched in Outlook), but only accepts `html` on write - so plain text
//! is escaped on the way out and HTML is flattened to readable text on the way in.

use outlook_tasks_core::models::{BodyType, ItemBody, TodoTask};

/// Escapes plain text into an HTML body: `&`, `<`, `>` become entities and
/// newlines become `<br>`. The inverse of [`html_to_text`] for the shapes we emit.
pub fn text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push_str("<br>"),
            '\r' => {} // CRLF collapses to a single <br>
            c => out.push(c),
        }
    }
    out
}

/// Flattens an HTML note to readable plain text. Tags are removed BEFORE entities
/// are decoded, so literally-escaped markup (e.g. `&lt;b&gt;`) survives as text
/// instead of being re-interpreted as a real tag.
pub fn html_to_text(html: &str) -> String {
    let without_blocks = strip_script_style(html);
    let flattened = strip_tags(&without_blocks);
    let decoded = decode_entities(&flattened);
    collapse_blank_lines(&decoded)
}

/// The plain text to show for a note: HTML is flattened, anything else is verbatim.
pub fn note_display_text(body: &ItemBody) -> String {
    match body.content_type {
        BodyType::Html => html_to_text(&body.content),
        _ => body.content.clone(),
    }
}

/// Whether a task carries a meaningful note. Graph returns an empty body object
/// (`{ content: "", contentType: "text" }`) for ordinary tasks, so this checks for
/// non-blank rendered text rather than mere presence of `body`.
pub fn has_note(task: &TodoTask) -> bool {
    task.body.as_ref().is_some_and(|b| !note_display_text(b).trim().is_empty())
}

/// Removes `<script>...</script>` and `<style>...</style>` blocks (content and
/// all), case-insensitively, so their text can't leak into the flattened note.
fn strip_script_style(s: &str) -> String {
    // ASCII lowercasing preserves byte length, so offsets map 1:1 to `s`.
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'outer: while i < s.len() {
        for tag in ["script", "style"] {
            if lower[i..].starts_with(&format!("<{tag}")) {
                let close = format!("</{tag}>");
                match lower[i..].find(&close) {
                    Some(rel) => i += rel + close.len(),
                    None => i = s.len(),
                }
                continue 'outer;
            }
        }
        let ch = s[i..].chars().next().expect("valid char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Drops HTML tags, turning line-breaking tags into newlines. A stray `<` with no
/// closing `>` is kept literally.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        let mut tag = String::new();
        let mut closed = false;
        for tc in chars.by_ref() {
            if tc == '>' {
                closed = true;
                break;
            }
            tag.push(tc);
        }
        if !closed {
            out.push('<');
            out.push_str(&tag);
            break;
        }
        if is_break_tag(&tag) {
            out.push('\n');
        }
    }
    out
}

/// Whether a tag's inner text (between `<` and `>`) starts a new visual line:
/// `<br>` or a closing block element.
fn is_break_tag(tag: &str) -> bool {
    let inner = tag.trim().trim_end_matches('/').trim();
    let name = inner.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    matches!(
        name.as_str(),
        "br" | "/p"
            | "/div"
            | "/li"
            | "/tr"
            | "/ul"
            | "/ol"
            | "/blockquote"
            | "/h1"
            | "/h2"
            | "/h3"
            | "/h4"
            | "/h5"
            | "/h6"
    )
}

/// Decodes the HTML entities the To Do/Outlook bodies actually use, plus numeric
/// (`&#NN;`) and hex (`&#xHH;`) references. An unrecognized `&...;` is left as-is.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        // An entity is short; only look a little way ahead for the ';'.
        if let Some(semi) = after[1..].find(';').map(|p| p + 1).filter(|&p| p <= 12) {
            let name = &after[1..semi];
            if let Some(ch) = decode_one(name) {
                out.push(ch);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

fn decode_one(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '), // a normal space reads and re-escapes cleanly
        _ => decode_numeric(name),
    }
}

fn decode_numeric(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

/// Trims trailing whitespace per line, collapses runs of blank lines to at most
/// one, and trims the result.
fn collapse_blank_lines(s: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut blanks = 0;
    for line in s.split('\n') {
        let line = line.trim_end();
        if line.is_empty() {
            blanks += 1;
            if blanks <= 1 {
                out.push(line);
            }
        } else {
            blanks = 0;
            out.push(line);
        }
    }
    out.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_to_html_escapes_and_breaks() {
        assert_eq!(text_to_html("a & b < c > d\ne"), "a &amp; b &lt; c &gt; d<br>e");
    }

    #[test]
    fn text_to_html_collapses_crlf() {
        assert_eq!(text_to_html("a\r\nb"), "a<br>b");
    }

    #[test]
    fn html_to_text_decodes_entities() {
        assert_eq!(html_to_text("a &amp; b &lt;x&gt; &quot;q&quot;"), "a & b <x> \"q\"");
    }

    #[test]
    fn html_to_text_breaks_on_br_and_blocks() {
        assert_eq!(html_to_text("x<br>y"), "x\ny");
        assert_eq!(html_to_text("<p>a</p><p>b</p>"), "a\nb");
        assert_eq!(html_to_text("<ul><li>one</li><li>two</li></ul>"), "one\ntwo");
    }

    #[test]
    fn html_to_text_keeps_literally_escaped_markup() {
        // Stripping tags BEFORE decoding means escaped markup is not eaten.
        assert_eq!(html_to_text("&lt;b&gt;x&lt;/b&gt;"), "<b>x</b>");
    }

    #[test]
    fn html_to_text_decodes_numeric_and_hex() {
        assert_eq!(html_to_text("&#65;&#x42;&#39;"), "AB'");
    }

    #[test]
    fn html_to_text_maps_nbsp_to_space() {
        assert_eq!(html_to_text("a&nbsp;b"), "a b");
    }

    #[test]
    fn html_to_text_drops_script_and_style() {
        assert_eq!(html_to_text("<style>p{color:red}</style>hi<script>alert(1)</script>"), "hi");
    }

    #[test]
    fn html_to_text_leaves_unknown_entity() {
        assert_eq!(html_to_text("100&unknown;200"), "100&unknown;200");
    }

    #[test]
    fn roundtrips_text_through_html() {
        for s in ["a < b\nc & d", "plain", "line1\nline2\nline3", "5 > 3 && 2 < 4"] {
            assert_eq!(html_to_text(&text_to_html(s)), s, "roundtrip failed for {s:?}");
        }
    }

    #[test]
    fn note_display_text_flattens_html_and_keeps_text() {
        let html = ItemBody { content: "<p>hi</p>".into(), content_type: BodyType::Html };
        assert_eq!(note_display_text(&html), "hi");
        let text = ItemBody { content: "<not html>".into(), content_type: BodyType::Text };
        assert_eq!(note_display_text(&text), "<not html>");
    }

    fn task_with_body(body: Option<ItemBody>) -> TodoTask {
        TodoTask { id: "1".into(), title: "t".into(), body, ..TodoTask::default() }
    }

    #[test]
    fn has_note_true_only_for_meaningful_content() {
        assert!(has_note(&task_with_body(Some(ItemBody {
            content: "buy milk".into(),
            content_type: BodyType::Text,
        }))));
        assert!(has_note(&task_with_body(Some(ItemBody {
            content: "<p>hi</p>".into(),
            content_type: BodyType::Html,
        }))));
        // No body, empty body, whitespace, and empty-rendering HTML all read as "no note".
        assert!(!has_note(&task_with_body(None)));
        assert!(!has_note(&task_with_body(Some(ItemBody {
            content: String::new(),
            content_type: BodyType::Text,
        }))));
        assert!(!has_note(&task_with_body(Some(ItemBody {
            content: "   ".into(),
            content_type: BodyType::Text,
        }))));
        assert!(!has_note(&task_with_body(Some(ItemBody {
            content: "<br>".into(),
            content_type: BodyType::Html,
        }))));
    }
}
