//! Google descriptions as plain text (`docs/architecture.md` §7.5).
//!
//! Notes written in Google's web interface arrive as a little HTML: `<br>`, `<b>`, lists and
//! links. The frontend shows descriptions as text and never as HTML, because a shared calendar
//! could otherwise inject markup, so the core turns them into plain text here. This is not an
//! HTML parser: it keeps the text, turns line-breaking tags into newlines and links into
//! "text (address)", and drops every other tag.

/// `value` as plain text if it looks like HTML, else unchanged.
pub(crate) fn plain_text(value: &str) -> String {
    if !looks_like_html(value) {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    // The address of the link being read, until its closing tag.
    let mut href: Option<String> = None;
    let mut link_start = 0;
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        out.push_str(&decode(&rest[..open]));
        let Some(close) = rest[open..].find('>') else {
            // A lone `<` is text.
            out.push_str(&decode(&rest[open..]));
            rest = "";
            break;
        };
        let tag = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match name.as_str() {
            "br" => out.push('\n'),
            "p" | "div" | "ul" | "ol" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if closing => {
                out.push('\n')
            }
            "li" if !closing => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("• ");
            }
            "a" if !closing => {
                href = attribute(tag, "href").map(|h| decode(&h));
                link_start = out.len();
            }
            "a" => {
                // "map (https://…)", unless the text already is the address.
                if let Some(href) = href.take()
                    && !href.is_empty()
                    && out[link_start..].trim() != href
                {
                    out.push_str(&format!(" ({href})"));
                }
            }
            _ => {}
        }
    }
    out.push_str(&decode(rest));
    tidy(&out)
}

/// A tag such as `<br>`, `</b>` or `<a href=…>` somewhere in the text.
fn looks_like_html(value: &str) -> bool {
    value.match_indices('<').any(|(i, _)| {
        let next = value[i + 1..].chars().next();
        matches!(next, Some(c) if c.is_ascii_alphabetic() || c == '/') && value[i..].contains('>')
    })
}

/// The value of attribute `name` in the inside of a tag, quoted or not.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let value = &tag[at..];
    let (quote, value) = match value.chars().next()? {
        q @ ('"' | '\'') => (Some(q), &value[1..]),
        _ => (None, value),
    };
    let end = match quote {
        Some(q) => value.find(q)?,
        None => value.find(char::is_whitespace).unwrap_or(value.len()),
    };
    Some(value[..end].to_owned())
}

/// Replaces the entities Google writes, and numeric ones. Unknown entities stay as written.
fn decode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest.find(';').filter(|&end| end <= 10).and_then(|end| {
            let entity = &rest[1..end];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => entity.strip_prefix('#').and_then(|n| {
                    let code = match n.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => n.parse().ok(),
                    };
                    code.and_then(char::from_u32)
                }),
            };
            c.map(|c| (c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Trims every line and the whole, and keeps at most one empty line in a row.
fn tidy(text: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    for line in text.lines().map(str::trim_end) {
        if line.trim().is_empty() && lines.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        lines.push(if line.trim().is_empty() { "" } else { line });
    }
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::plain_text;

    #[test]
    fn plain_text_stays_as_it_is() {
        for text in [
            "Bring snacks",
            "a < b and c > d",
            "Fish & chips &amp; more",
            "Line one\n\n\nLine two  ",
            "<3 you",
        ] {
            assert_eq!(plain_text(text), text);
        }
    }

    #[test]
    fn breaks_and_paragraphs_become_newlines() {
        assert_eq!(
            plain_text("one<br>two<BR/>three<br />four"),
            "one\ntwo\nthree\nfour"
        );
        assert_eq!(plain_text("<p>First</p><p>Second</p>"), "First\nSecond");
        assert_eq!(
            plain_text("<div>a</div><div><br></div><div>b</div>"),
            "a\n\nb"
        );
    }

    #[test]
    fn formatting_tags_are_dropped() {
        assert_eq!(
            plain_text("<b>Bold</b>, <i>italic</i> and <u>under</u><span style=\"x\">!</span>"),
            "Bold, italic and under!"
        );
    }

    #[test]
    fn links_keep_their_address() {
        assert_eq!(
            plain_text("Join <a href=\"https://meet.example/abc\" target=\"_blank\">here</a>."),
            "Join here (https://meet.example/abc)."
        );
        // Google often links a pasted address to itself: no need to say it twice.
        assert_eq!(
            plain_text("<a href=\"https://example.com\">https://example.com</a>"),
            "https://example.com"
        );
        assert_eq!(
            plain_text("<a href='https://a.test/?x=1&amp;y=2'>map</a>"),
            "map (https://a.test/?x=1&y=2)"
        );
    }

    #[test]
    fn lists_become_bullets() {
        assert_eq!(
            plain_text("Bring:<ul><li>tent</li><li>stove</li></ul>Thanks"),
            "Bring:\n• tent\n• stove\nThanks"
        );
    }

    #[test]
    fn entities_are_decoded_in_html() {
        assert_eq!(
            plain_text("<b>Fish &amp; chips</b> &lt;3&nbsp;&#8364;&#x20AC; &copy;"),
            "Fish & chips <3 €€ &copy;"
        );
    }

    #[test]
    fn markup_is_never_passed_on() {
        assert_eq!(
            plain_text("<script>alert(1)</script><img src=x onerror=alert(2)>hi"),
            "alert(1)hi"
        );
    }

    #[test]
    fn an_unclosed_tag_is_text() {
        assert_eq!(plain_text("<b>a</b> <c"), "a <c");
    }
}
