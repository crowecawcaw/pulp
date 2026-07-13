// backend/src/collectors/rss_parse.rs
//
// Minimal, dependency-free RSS/Atom helpers shared by collectors. The project
// deliberately hand-rolls feed parsing instead of pulling in an XML crate; this
// module factors the small primitives so the Reddit collector (Atom) and any
// other feed collector can reuse them.

/// Split a feed body into its `<item>` (RSS) or `<entry>` (Atom) blocks.
pub fn extract_items(xml: &str) -> Vec<String> {
    let mut items = Vec::new();
    for tag in &["item", "entry"] {
        let open = format!("<{}", tag);
        let close = format!("</{}>", tag);
        let mut start = 0;
        while let Some(s) = xml[start..].find(&open) {
            let s = start + s;
            // Guard against matching a longer tag name (e.g. `<entryfoo>`): the
            // char after the tag name must be whitespace or `>`.
            let after = xml[s + open.len()..].chars().next();
            if !matches!(after, Some(c) if c.is_whitespace() || c == '>' || c == '/') {
                start = s + open.len();
                continue;
            }
            if let Some(e) = xml[s..].find(&close) {
                let end = s + e + close.len();
                items.push(xml[s..end].to_string());
                start = end;
            } else {
                break;
            }
        }
    }
    items
}

/// Extract the inner text of the first `<tag>...</tag>` (CDATA-stripped, trimmed,
/// entity-decoded).
pub fn extract_tag(xml: &str, tag: &str) -> Option<String> {
    // Match `<tag>` or `<tag attr=...>` but require a word boundary after the
    // tag name so `<id>` doesn't match `<identifier>`.
    let open_prefix = format!("<{}", tag);
    let mut search_from = 0;
    loop {
        let rel = xml[search_from..].find(&open_prefix)?;
        let tag_start = search_from + rel;
        let after_name = tag_start + open_prefix.len();
        let next_char = xml[after_name..].chars().next();
        if !matches!(next_char, Some(c) if c.is_whitespace() || c == '>') {
            search_from = after_name;
            continue;
        }
        // Find the end of the opening tag.
        let gt = xml[after_name..].find('>')? + after_name;
        // Self-closing tag has no inner text.
        if xml[..gt].ends_with('/') {
            return None;
        }
        let inner_start = gt + 1;
        let close = format!("</{}>", tag);
        let end = xml[inner_start..].find(&close)? + inner_start;
        return Some(
            decode_entities(&strip_cdata(&xml[inner_start..end]))
                .trim()
                .to_string(),
        );
    }
}

/// Extract an attribute value from the first occurrence of `<tag ...attr="value"...>`.
pub fn extract_attr(xml: &str, tag: &str, attr: &str) -> Option<String> {
    let open_prefix = format!("<{}", tag);
    let mut search_from = 0;
    loop {
        let rel = xml[search_from..].find(&open_prefix)?;
        let tag_start = search_from + rel;
        let after_name = tag_start + open_prefix.len();
        let next_char = xml[after_name..].chars().next();
        if !matches!(next_char, Some(c) if c.is_whitespace() || c == '>' || c == '/') {
            search_from = after_name;
            continue;
        }
        let gt = xml[after_name..].find('>')? + after_name;
        let tag_str = &xml[tag_start..gt];
        let attr_key = format!("{}=\"", attr);
        if let Some(rel_attr) = tag_str.find(&attr_key) {
            let attr_start = rel_attr + attr_key.len();
            let attr_end = tag_str[attr_start..].find('"')? + attr_start;
            return Some(decode_entities(&tag_str[attr_start..attr_end]));
        }
        search_from = gt;
    }
}

pub fn strip_cdata(s: &str) -> String {
    let t = s.trim();
    if t.starts_with("<![CDATA[") && t.ends_with("]]>") {
        t[9..t.len() - 3].to_string()
    } else {
        s.to_string()
    }
}

/// Strip HTML tags from a snippet, leaving plain text. Each tag becomes a
/// space so adjacent blocks don't run together (`<p>a</p><p>b</p>` -> "a b"
/// rather than "ab"); the resulting whitespace runs are then collapsed.
pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Decode entities first (so an entity-encoded space counts), then collapse
    // the whitespace the tag stripping introduced and trim the ends.
    decode_entities(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Decode the XML/HTML entities that appear in feeds: the named set plus
/// numeric character references in decimal (`&#39;`) and hex (`&#x2F;`) form.
/// HN in particular encodes apostrophes/slashes as hex (`&#x27;`, `&#x2F;`),
/// which a fixed named-entity table misses.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        // Look for the closing `;` within a short window (entities are short);
        // a bare `&` with no nearby `;` is literal text.
        match after[1..].find(';').filter(|&i| i > 0 && i <= 10) {
            Some(semi) => {
                let body = &after[1..1 + semi];
                if let Some(ch) = decode_one(body) {
                    out.push(ch);
                } else {
                    // Unknown entity — keep it verbatim.
                    out.push_str(&after[..semi + 2]);
                }
                rest = &after[semi + 2..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Decode the inside of a single `&…;` entity (without the `&`/`;`).
fn decode_one(body: &str) -> Option<char> {
    match body {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "amp" => Some('&'),
        "nbsp" => Some(' '),
        _ => {
            let code =
                if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()?
                } else if let Some(dec) = body.strip_prefix('#') {
                    dec.parse::<u32>().ok()?
                } else {
                    return None;
                };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_atom_entries() {
        let xml = "<feed><entry><title>A</title></entry><entry><title>B</title></entry></feed>";
        let items = extract_items(xml);
        assert_eq!(items.len(), 2);
        assert!(items[0].contains("<title>A</title>"));
    }

    #[test]
    fn tag_boundary_is_respected() {
        // `<id>` must not match `<identifier>`.
        let xml = "<entry><identifier>nope</identifier><id>t3_real</id></entry>";
        assert_eq!(extract_tag(xml, "id").as_deref(), Some("t3_real"));
    }

    #[test]
    fn extracts_attr_from_self_closing() {
        let xml = r#"<entry><link href="https://x/y" rel="alternate"/></entry>"#;
        assert_eq!(
            extract_attr(xml, "link", "href").as_deref(),
            Some("https://x/y")
        );
    }

    #[test]
    fn extracts_category_term() {
        let xml = r#"<entry><category term="rust" label="r/rust"/></entry>"#;
        assert_eq!(
            extract_attr(xml, "category", "term").as_deref(),
            Some("rust")
        );
    }

    #[test]
    fn strips_html_and_decodes() {
        assert_eq!(strip_html("<p>hi &amp; bye</p>"), "hi & bye");
    }

    #[test]
    fn adjacent_blocks_get_a_space() {
        // Paragraph boundaries must not glue sentences together.
        assert_eq!(strip_html("<p>them.</p><p>As for</p>"), "them. As for");
        // Inline tags collapse cleanly without leaving double spaces.
        assert_eq!(strip_html("see <a href=\"x\">this</a> now"), "see this now");
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(decode_entities("a &amp; b &lt;c&gt;"), "a & b <c>");
    }

    #[test]
    fn decodes_numeric_character_references() {
        // HN encodes apostrophe/slash as hex; also handle decimal and uppercase X.
        assert_eq!(decode_entities("I&#x27;m on US&#x2F;EU"), "I'm on US/EU");
        assert_eq!(decode_entities("a&#39;b&#32;c"), "a'b c");
        assert_eq!(decode_entities("&#X2F;"), "/");
        // Unknown / malformed entities are left verbatim; bare & is literal.
        assert_eq!(decode_entities("Tom & Jerry"), "Tom & Jerry");
        assert_eq!(decode_entities("a &bogus; b"), "a &bogus; b");
        assert_eq!(decode_entities("100&#xZZ; off"), "100&#xZZ; off");
        // No ampersand: returned unchanged.
        assert_eq!(decode_entities("plain text"), "plain text");
    }
}
