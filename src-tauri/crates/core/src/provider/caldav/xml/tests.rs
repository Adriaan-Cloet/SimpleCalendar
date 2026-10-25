use super::*;

const RADICALE_HOME: &str = "<?xml version='1.0' encoding='utf-8'?>\n\
<multistatus xmlns=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\"><response><href>/alice/</href>\
<propstat><prop><C:calendar-home-set><href>/alice/</href></C:calendar-home-set></prop>\
<status>HTTP/1.1 200 OK</status></propstat></response></multistatus>";

#[test]
fn reads_responses_with_a_default_namespace() {
    let ms = Multistatus::parse(RADICALE_HOME).unwrap();
    assert_eq!(ms.responses.len(), 1);
    let r = &ms.responses[0];
    assert_eq!(r.href, "/alice/");
    assert_eq!(r.status, None);
    let home = r.prop(CALDAV, "calendar-home-set").unwrap();
    assert_eq!(home.href().as_deref(), Some("/alice/"));
}

#[test]
fn names_are_matched_by_namespace_not_prefix() {
    // The same element under two prefixes, and an element with the right name in a foreign
    // namespace, which must not match.
    let xml = r#"<x:multistatus xmlns:x="DAV:" xmlns:o="urn:other">
        <x:response><x:href>/a/</x:href><x:propstat><x:prop>
            <o:displayname>wrong</o:displayname>
            <y:displayname xmlns:y="DAV:">right</y:displayname>
        </x:prop><x:status>HTTP/1.1 200 OK</x:status></x:propstat></x:response>
    </x:multistatus>"#;
    let ms = Multistatus::parse(xml).unwrap();
    let name = ms.responses[0].prop(DAV, "displayname").unwrap();
    assert_eq!(name.text(), "right");
}

#[test]
fn only_properties_of_successful_propstats_are_kept() {
    let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:cs="http://calendarserver.org/ns/">
        <d:response><d:href>/c/</d:href>
        <d:propstat><d:prop><d:displayname>Work</d:displayname></d:prop>
            <d:status>HTTP/1.1 200 OK</d:status></d:propstat>
        <d:propstat><d:prop><cs:getctag/></d:prop>
            <d:status>HTTP/1.1 404 Not Found</d:status></d:propstat>
        </d:response></d:multistatus>"#;
    let r = &Multistatus::parse(xml).unwrap().responses[0];
    assert!(r.prop(DAV, "displayname").is_some());
    assert!(r.prop(CS, "getctag").is_none());
}

#[test]
fn a_response_status_is_read() {
    // A deleted member in a sync report: a status instead of a propstat.
    let xml = r#"<multistatus xmlns="DAV:"><response><href>/c/gone.ics</href>
        <status>HTTP/1.1 404 Not Found</status></response>
        <sync-token>http://example.com/sync/5</sync-token></multistatus>"#;
    let ms = Multistatus::parse(xml).unwrap();
    assert_eq!(ms.responses[0].status, Some(404));
    assert_eq!(ms.sync_token.as_deref(), Some("http://example.com/sync/5"));
}

#[test]
fn text_with_entities_and_cdata_is_joined_and_unescaped() {
    let xml = "<multistatus xmlns=\"DAV:\" xmlns:C=\"urn:ietf:params:xml:ns:caldav\">\
        <response><href>/c/a%20b.ics</href><propstat><prop>\
        <getetag>&quot;abc&quot;</getetag>\
        <C:calendar-data>BEGIN:VCALENDAR\r\nSUMMARY:Tom &amp; Jerry &#x263A;\r\n<![CDATA[X-A:<b>\r\n]]>END:VCALENDAR\r\n</C:calendar-data>\
        </prop><status>HTTP/1.1 200 OK</status></propstat></response></multistatus>";
    let r = &Multistatus::parse(xml).unwrap().responses[0];
    assert_eq!(r.href, "/c/a%20b.ics");
    assert_eq!(r.prop(DAV, "getetag").unwrap().text(), "\"abc\"");
    // XML turns every `\r\n` into `\n` (XML 1.0 §2.11); the iCalendar parser accepts both.
    assert_eq!(
        r.prop(CALDAV, "calendar-data").unwrap().raw_text(),
        "BEGIN:VCALENDAR\nSUMMARY:Tom & Jerry \u{263A}\nX-A:<b>\nEND:VCALENDAR\n"
    );
}

#[test]
fn resource_types_and_components() {
    let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:cal="urn:ietf:params:xml:ns:caldav">
        <d:response><d:href>/c/</d:href><d:propstat><d:prop>
        <d:resourcetype><d:collection/><cal:calendar/></d:resourcetype>
        <cal:supported-calendar-component-set><cal:comp name="VEVENT"/><cal:comp name="VTODO"/>
        </cal:supported-calendar-component-set>
        </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    let r = &Multistatus::parse(xml).unwrap().responses[0];
    let types = r.prop(DAV, "resourcetype").unwrap();
    assert!(types.has(CALDAV, "calendar"));
    assert!(types.has(DAV, "collection"));
    assert!(!types.has(DAV, "principal"));
    let comps: Vec<_> = r
        .prop(CALDAV, "supported-calendar-component-set")
        .unwrap()
        .children(CALDAV, "comp")
        .filter_map(|c| c.attr("name"))
        .collect();
    assert_eq!(comps, ["VEVENT", "VTODO"]);
}

#[test]
fn not_xml_is_an_error() {
    assert!(Multistatus::parse("<html><body>Login</body>").is_err());
    assert!(Multistatus::parse("not xml at all").is_err());
}

#[test]
fn escapes_text_for_request_bodies() {
    let body = sync_collection("http://x/sync?a=1&b=<2>");
    assert!(body.contains("<d:sync-token>http://x/sync?a=1&amp;b=&lt;2&gt;</d:sync-token>"));
    let body = multiget(&["/c/a&b.ics".to_owned()]);
    assert!(body.contains("<d:href>/c/a&amp;b.ics</d:href>"));
}
