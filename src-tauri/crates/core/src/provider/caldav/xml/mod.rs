//! WebDAV XML: the request bodies the provider sends, and reading the `multistatus` answers
//! (RFC 4918 §13, RFC 4791, RFC 6578).
//!
//! An answer is read into a small tree first and queried afterwards. Answers are a few hundred
//! kilobytes at most, and a tree keeps the reading code free of parser state. Elements are
//! matched by namespace and local name, never by prefix: servers pick their own prefixes.

#[cfg(test)]
mod tests;

use quick_xml::escape::{escape, resolve_predefined_entity};
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::{NsReader, XmlVersion};

pub(super) const DAV: &str = "DAV:";
pub(super) const CALDAV: &str = "urn:ietf:params:xml:ns:caldav";
/// Apple's CalendarServer: `getctag`.
pub(super) const CS: &str = "http://calendarserver.org/ns/";
/// Apple's iCal: `calendar-color`.
pub(super) const ICAL: &str = "http://apple.com/ns/ical/";

/// Discovery: what a URL is, and where it points (RFC 6764 §6, RFC 4791 §6.2.1).
pub(super) const PROPFIND_DISCOVER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
<d:prop><d:resourcetype/><d:current-user-principal/><c:calendar-home-set/></d:prop>
</d:propfind>"#;

/// The properties of a calendar collection, for the calendar list.
pub(super) const PROPFIND_CALENDARS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/">
<d:prop><d:resourcetype/><d:displayname/><a:calendar-color/><c:calendar-timezone/><c:supported-calendar-component-set/><d:current-user-privilege-set/></d:prop>
</d:propfind>"#;

/// A collection's version tags: its ctag and sync-token, and with `Depth: 1` each member's
/// ETag.
pub(super) const PROPFIND_VERSIONS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/">
<d:prop><d:resourcetype/><d:getetag/><cs:getctag/><d:sync-token/></d:prop>
</d:propfind>"#;

/// The changes since `token` (RFC 6578 §3.2).
pub(super) fn sync_collection(token: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:sync-collection xmlns:d="DAV:">
<d:sync-token>{}</d:sync-token><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop>
</d:sync-collection>"#,
        escape(token)
    )
}

/// The data of the resources at `hrefs` (RFC 4791 §7.9).
pub(super) fn multiget(hrefs: &[String]) -> String {
    let hrefs: String = hrefs
        .iter()
        .map(|href| format!("<d:href>{}</d:href>", escape(href.as_str())))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<c:calendar-multiget xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
<d:prop><d:getetag/><c:calendar-data/></d:prop>{hrefs}
</c:calendar-multiget>"#
    )
}

/// Why an answer could not be read.
#[derive(Debug, thiserror::Error)]
#[error("the server's answer is not a WebDAV multistatus: {0}")]
pub(super) struct XmlError(String);

/// One element: its namespace and local name, attributes, children and text.
#[derive(Debug, Default)]
pub(super) struct Element {
    ns: String,
    name: String,
    /// Unprefixed attributes only; that is all CalDAV uses (`comp name="VEVENT"`).
    attrs: Vec<(String, String)>,
    children: Vec<Element>,
    /// The text directly inside, entities resolved and CDATA included.
    text: String,
}

impl Element {
    fn is(&self, ns: &str, name: &str) -> bool {
        self.ns == ns && self.name == name
    }

    pub fn child(&self, ns: &str, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.is(ns, name))
    }

    pub fn children<'a>(&'a self, ns: &'a str, name: &'a str) -> impl Iterator<Item = &'a Element> {
        self.children.iter().filter(move |c| c.is(ns, name))
    }

    /// Whether it has a child `ns:name`, as a `resourcetype` has `calendar`.
    pub fn has(&self, ns: &str, name: &str) -> bool {
        self.child(ns, name).is_some()
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The text without surrounding whitespace.
    pub fn text(&self) -> &str {
        self.text.trim()
    }

    /// The text as it is, for `calendar-data`.
    pub fn raw_text(&self) -> &str {
        &self.text
    }

    /// The text of the first `DAV:href` inside, as in `current-user-principal`.
    pub fn href(&self) -> Option<String> {
        self.child(DAV, "href")
            .map(|h| h.text().to_owned())
            .filter(|h| !h.is_empty())
    }
}

/// One `response` of a multistatus.
#[derive(Debug)]
pub(super) struct Response {
    /// As the server wrote it: a path, or rarely a full URL.
    pub href: String,
    /// The status of the response itself. Only set when it has no `propstat`, as for a member
    /// that was deleted.
    pub status: Option<u16>,
    /// The properties from the `propstat`s with a 2xx status. Missing ones come with 404 and
    /// are left out.
    props: Vec<Element>,
}

impl Response {
    pub fn prop(&self, ns: &str, name: &str) -> Option<&Element> {
        self.props.iter().find(|p| p.is(ns, name))
    }
}

/// A parsed `multistatus`.
#[derive(Debug)]
pub(super) struct Multistatus {
    pub responses: Vec<Response>,
    /// The new token of a sync report.
    pub sync_token: Option<String>,
}

impl Multistatus {
    pub fn parse(xml: &str) -> Result<Multistatus, XmlError> {
        let root = parse(xml)?;
        if !root.is(DAV, "multistatus") {
            return Err(XmlError(format!("the root element is {}", root.name)));
        }
        let mut responses = Vec::new();
        let mut sync_token = None;
        // Rust note: `into_iter` on the owned tree moves each child out instead of copying it,
        // so the properties end up in the responses without a clone.
        for child in root.children {
            if child.is(DAV, "response") {
                responses.extend(Response::of(child));
            } else if child.is(DAV, "sync-token") && !child.text().is_empty() {
                sync_token = Some(child.text().to_owned());
            }
        }
        Ok(Multistatus {
            responses,
            sync_token,
        })
    }
}

impl Response {
    /// `None` for a response without an `href`, which no server sends.
    fn of(element: Element) -> Option<Response> {
        let href = element.child(DAV, "href")?.text().to_owned();
        let status = element.child(DAV, "status").and_then(status_code);
        let props = element
            .children
            .into_iter()
            .filter(|ps| {
                ps.is(DAV, "propstat")
                    && ps
                        .child(DAV, "status")
                        .and_then(status_code)
                        .is_some_and(ok)
            })
            .flat_map(|ps| ps.children)
            .filter(|p| p.is(DAV, "prop"))
            .flat_map(|p| p.children)
            .collect();
        Some(Response {
            href,
            status,
            props,
        })
    }
}

fn ok(code: u16) -> bool {
    (200..300).contains(&code)
}

/// The code in a `status` element: `HTTP/1.1 404 Not Found` gives 404.
fn status_code(status: &Element) -> Option<u16> {
    status.text().split_whitespace().nth(1)?.parse().ok()
}

/// Reads a whole document into its root element.
fn parse(xml: &str) -> Result<Element, XmlError> {
    let mut reader = NsReader::from_str(xml);
    // The open elements, innermost last. The bottom one collects the root.
    let mut stack: Vec<Element> = vec![Element::default()];
    loop {
        let (ns, event) = reader
            .read_resolved_event()
            .map_err(|e| XmlError(e.to_string()))?;
        match event {
            Event::Start(start) => {
                let element = open(ns, &start)?;
                stack.push(element);
            }
            Event::Empty(start) => {
                let element = open(ns, &start)?;
                top(&mut stack).children.push(element);
            }
            Event::End(_) => {
                let element = stack.pop().expect("an End has a Start");
                if stack.is_empty() {
                    return Err(XmlError("unbalanced elements".into()));
                }
                top(&mut stack).children.push(element);
            }
            Event::Text(text) => top(&mut stack)
                .text
                .push_str(&text.xml_content(XmlVersion::Implicit1_0)),
            Event::CData(data) => top(&mut stack)
                .text
                .push_str(&data.xml_content(XmlVersion::Implicit1_0)),
            // Rust note: since quick-xml 0.38 an entity such as `&amp;` arrives as its own
            // event between two pieces of text, instead of being resolved inside the text.
            Event::GeneralRef(reference) => {
                let resolved = match reference.resolve_char_ref() {
                    Ok(Some(c)) => c.to_string(),
                    _ => resolve_predefined_entity(&reference.xml_content(XmlVersion::Implicit1_0))
                        .ok_or_else(|| XmlError("an unknown entity".into()))?
                        .to_owned(),
                };
                top(&mut stack).text.push_str(&resolved);
            }
            Event::Eof => break,
            // The declaration, comments, processing instructions and a doctype carry nothing.
            _ => {}
        }
    }
    let mut document = stack.pop().expect("the bottom element stays");
    if !stack.is_empty() {
        return Err(XmlError("the document ends inside an element".into()));
    }
    match document.children.pop() {
        Some(root) if document.children.is_empty() => Ok(root),
        _ => Err(XmlError("no single root element".into())),
    }
}

fn top(stack: &mut [Element]) -> &mut Element {
    stack.last_mut().expect("the bottom element stays")
}

fn open(ns: ResolveResult<'_>, start: &BytesStart<'_>) -> Result<Element, XmlError> {
    let ns = match ns {
        ResolveResult::Bound(ns) => ns.0.to_owned(),
        // No namespace, or a prefix that was never declared: neither is a DAV element.
        _ => String::new(),
    };
    let name = start.local_name().as_ref().to_owned();
    let attrs = start
        .attributes()
        .filter_map(Result::ok)
        .filter(|a| a.key.prefix().is_none())
        .filter_map(|a| {
            let key = a.key.as_ref().to_owned();
            let value = a
                .normalized_value(XmlVersion::Implicit1_0)
                .ok()?
                .into_owned();
            Some((key, value))
        })
        .collect();
    Ok(Element {
        ns,
        name,
        attrs,
        ..Element::default()
    })
}
