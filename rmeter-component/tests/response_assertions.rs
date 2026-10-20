use std::time::{Duration, SystemTime};

use rmeter_component::{
    AssertionConfig, JsonAssertionConfig, MatchSelection, RegexAssertion, RegexAssertionConfig,
    ResponseAssertion, XmlAssertion, XmlAssertionConfig, XmlMatcher,
};
use rmeter_core::{
    assertion::{Assertion, AssertionInput, AssertionResult},
    context::UserContext,
    sampler::SampleResult,
};

fn check(assertion: &impl Assertion, body: Option<&[u8]>, reason: Option<&str>) -> AssertionResult {
    let sample = SampleResult {
        label: "response".into(),
        started_at: SystemTime::now(),
        elapsed: Duration::ZERO,
        success: true,
        response_code: Some("200".into()),
        response_message: None,
        bytes_sent: 0,
        bytes_received: 0,
        response_body: Some(b"truncated report".to_vec()),
        assertions: vec![],
    };
    assertion.evaluate(
        &AssertionInput {
            sample: &sample,
            response_body: body,
            unavailable_reason: reason,
        },
        &UserContext::new("g", 0),
    )
}
fn regex(pattern: &str, invert: bool) -> RegexAssertion {
    RegexAssertion::new(RegexAssertionConfig {
        name: "body regex".into(),
        pattern: pattern.into(),
        invert,
    })
    .unwrap()
}
fn xml(xpath: &str, matcher: XmlMatcher, selection: MatchSelection) -> XmlAssertion {
    let mut config = XmlAssertionConfig::equals("XML check", xpath, "");
    config.matcher = matcher;
    config.selection = selection;
    XmlAssertion::new(config).unwrap()
}

#[test]
fn regex_works_on_raw_text_xml_and_multiline_bodies_without_json() {
    for (pattern, body) in [
        ("ready", "service ready"),
        ("(?m)^status=ok$", "header\nstatus=ok\n"),
        ("(?s)^a.*b$", "a\nb"),
        ("<code>0</code>", "<root><code>0</code></root>"),
        ("中文", "中文响应"),
    ] {
        assert!(check(&regex(pattern, false), Some(body.as_bytes()), None).success);
    }
    assert!(!check(&regex("^ready$", false), Some(b"service ready"), None).success);
    assert!(check(&regex("ERROR", true), Some(b"service ready"), None).success);
    assert!(!check(&regex("ready", true), Some(b"service ready"), None).success);
    assert!(check(&regex("^$", false), Some(b""), None).success);
    assert!(!check(&regex(".", true), Some(&[0xff]), None).success);
}

#[test]
fn xml_selects_elements_attributes_text_and_scalar_xpath_results() {
    let body = br#"<root id="42"><code>0</code><name>A&amp;B</name><empty/><nested>A<b>B</b>C</nested></root>"#;
    for (path, expected) in [
        ("/root/code", "0"),
        ("/root/@id", "42"),
        ("/root/name/text()", "A&B"),
        ("/root/empty", ""),
        ("/root/nested", "ABC"),
        ("count(/root/code)", "1"),
        ("boolean(/root/code)", "true"),
        ("string(/root/name)", "A&B"),
    ] {
        let result = check(
            &xml(
                path,
                XmlMatcher::Equals(expected.into()),
                MatchSelection::One,
            ),
            Some(body),
            None,
        );
        assert!(result.success, "{path}: {:?}", result.message);
    }
    assert!(
        !check(
            &xml(
                "/root/code",
                XmlMatcher::Equals(" 0 ".into()),
                MatchSelection::One
            ),
            Some(body),
            None
        )
        .success
    );
}

#[test]
fn xml_cardinality_and_missing_nodes_are_explicit() {
    let body = b"<root><item>ok</item><item>bad</item><empty/></root>";
    let equals = || XmlMatcher::Equals("ok".into());
    assert!(
        !check(
            &xml("/root/item", equals(), MatchSelection::One),
            Some(body),
            None
        )
        .success
    );
    assert!(
        check(
            &xml("/root/item", equals(), MatchSelection::Any),
            Some(body),
            None
        )
        .success
    );
    assert!(
        !check(
            &xml("/root/item", equals(), MatchSelection::All),
            Some(body),
            None
        )
        .success
    );
    assert!(
        check(
            &xml(
                "/root/item",
                XmlMatcher::Regex("^(ok|bad)$".into()),
                MatchSelection::All
            ),
            Some(body),
            None
        )
        .success
    );
    for selection in [
        MatchSelection::One,
        MatchSelection::Any,
        MatchSelection::All,
    ] {
        assert!(!check(&xml("/missing", equals(), selection), Some(body), None).success);
    }
    assert!(
        check(
            &xml("/root/empty", XmlMatcher::Exists, MatchSelection::One),
            Some(body),
            None
        )
        .success
    );
    assert!(
        !check(
            &xml("/root/empty", XmlMatcher::NotExists, MatchSelection::One),
            Some(body),
            None
        )
        .success
    );
    assert!(
        check(
            &xml("/missing", XmlMatcher::NotExists, MatchSelection::One),
            Some(body),
            None
        )
        .success
    );
    assert!(
        !check(
            &xml("false()", XmlMatcher::NotExists, MatchSelection::One),
            Some(body),
            None
        )
        .success
    );
}

#[test]
fn xml_namespaces_and_bom_are_supported() {
    let mut config = XmlAssertionConfig::equals("namespace", "/d:root/d:code/@value", "0");
    config.namespaces.insert("d".into(), "urn:demo".into());
    let assertion = XmlAssertion::new(config).unwrap();
    assert!(
        check(
            &assertion,
            Some("\u{feff}<root xmlns='urn:demo'><code value='0'/></root>".as_bytes()),
            None
        )
        .success
    );
    let error =
        XmlAssertion::new(XmlAssertionConfig::equals("unbound", "/x:root", "")).unwrap_err();
    assert!(error.message.contains("undefined XPath namespace prefix"));
    let literal = XmlAssertion::new(XmlAssertionConfig::equals(
        "literal",
        "/root[@uri='urn:test']",
        "ok",
    ))
    .unwrap();
    assert!(check(&literal, Some(b"<root uri='urn:test'>ok</root>"), None).success);
    assert!(
        check(
            &xml(
                "/root/@xml:lang",
                XmlMatcher::Equals("en".into()),
                MatchSelection::One
            ),
            Some(b"<root xml:lang='en'/>"),
            None
        )
        .success
    );
    let mut axis = XmlAssertionConfig::equals("axis", "descendant::d-ns:code", "0");
    axis.namespaces.insert("d-ns".into(), "urn:demo".into());
    assert!(
        check(
            &XmlAssertion::new(axis).unwrap(),
            Some(b"<root xmlns='urn:demo'><code>0</code></root>"),
            None
        )
        .success
    );
}

#[test]
fn invalid_static_configuration_is_rejected_before_sampling() {
    let error = RegexAssertion::new(RegexAssertionConfig::new("bad regex", "[")).unwrap_err();
    assert!(error.to_string().contains("bad regex"));
    for path in ["", "/root[", "///"] {
        assert!(
            XmlAssertion::new(XmlAssertionConfig::equals("bad XPath", path, "0")).is_err(),
            "{path}"
        );
    }
    let mut config = XmlAssertionConfig::equals("bad XML regex", "/root", "");
    config.matcher = XmlMatcher::Regex("[".into());
    assert!(XmlAssertion::new(config).is_err());
    for (prefix, uri) in [
        ("", "urn:test"),
        ("x", ""),
        ("xmlns", "urn:test"),
        ("xml", "urn:wrong"),
    ] {
        let mut config = XmlAssertionConfig::equals("namespace", "/root", "");
        config.namespaces.insert(prefix.into(), uri.into());
        assert!(XmlAssertion::new(config).is_err());
    }
}

#[test]
fn unavailable_or_partial_input_never_passes_even_negative_assertions() {
    let assertions = [
        ResponseAssertion::new(
            RegexAssertionConfig {
                name: "negative regex".into(),
                pattern: "ERROR".into(),
                invert: true,
            }
            .into(),
        )
        .unwrap(),
        ResponseAssertion::new(xml_config_missing().into()).unwrap(),
        ResponseAssertion::new(
            JsonAssertionConfig {
                name: "missing JSON".into(),
                path: "$.missing".into(),
                matcher: rmeter_component::JsonMatcher::NotExists,
                selection: MatchSelection::One,
            }
            .into(),
        )
        .unwrap(),
    ];
    for assertion in assertions {
        assert!(!check(&assertion, None, None).success);
        let result = check(
            &assertion,
            Some(b"<root/>"),
            Some("response body is incomplete"),
        );
        assert!(!result.success);
        assert!(result.message.unwrap().contains("incomplete"));
    }
}
fn xml_config_missing() -> XmlAssertionConfig {
    let mut config = XmlAssertionConfig::equals("missing XML", "/missing", "");
    config.matcher = XmlMatcher::NotExists;
    config
}

#[test]
fn invalid_xml_and_doctype_fail_but_literal_markers_do_not() {
    let assertion = XmlAssertion::new(xml_config_missing()).unwrap();
    for body in [
        b"<root>".as_slice(),
        b"not XML",
        b"",
        &[0xff],
        b"<!DOCTYPE root [<!ENTITY e 'value'>]><root>&e;</root>",
        b"<!DOCTYPE root SYSTEM 'file:///missing'><root/>",
    ] {
        assert!(!check(&assertion, Some(body), None).success, "{body:?}");
    }
    for body in [
        b"<root><!-- <!DOCTYPE root> --></root>".as_slice(),
        b"<root><![CDATA[<!DOCTYPE root>]]></root>",
    ] {
        assert!(check(&assertion, Some(body), None).success);
    }
}

#[test]
fn components_remain_send_sync_and_xml_can_be_evaluated_on_multiple_threads() {
    fn thread_safe<T: Send + Sync>() {}
    thread_safe::<ResponseAssertion>();
    thread_safe::<AssertionConfig>();
    let assertion = std::sync::Arc::new(
        XmlAssertion::new(XmlAssertionConfig::equals("code", "/root/code", "0")).unwrap(),
    );
    let tasks = (0..4)
        .map(|_| {
            let assertion = assertion.clone();
            std::thread::spawn(move || {
                for _ in 0..10 {
                    assert!(
                        check(
                            assertion.as_ref(),
                            Some(b"<root><code>0</code></root>"),
                            None
                        )
                        .success
                    );
                }
            })
        })
        .collect::<Vec<_>>();
    for task in tasks {
        task.join().unwrap();
    }
}
