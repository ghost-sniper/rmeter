use std::time::{Duration, SystemTime};

use rmeter_component::{JsonAssertion, JsonAssertionConfig, JsonMatcher, MatchSelection};
use rmeter_core::{
    assertion::{Assertion, AssertionInput},
    context::UserContext,
    sampler::SampleResult,
};
use serde_json::{Value, json};

fn assertion(path: &str, matcher: JsonMatcher, selection: MatchSelection) -> JsonAssertion {
    JsonAssertion::new(JsonAssertionConfig {
        name: "check".into(),
        path: path.into(),
        matcher,
        selection,
    })
    .unwrap()
}

fn sample() -> SampleResult {
    SampleResult {
        label: "http".into(),
        started_at: SystemTime::now(),
        elapsed: Duration::ZERO,
        success: true,
        response_code: Some("200".into()),
        response_message: Some("OK".into()),
        bytes_sent: 0,
        bytes_received: 0,
        response_body: None,
        assertions: vec![],
    }
}

#[test]
fn equality_preserves_json_types_and_structures() {
    for value in [
        json!("text"),
        json!(42),
        json!(true),
        Value::Null,
        json!([1, 2]),
        json!({"nested": [false, null]}),
    ] {
        let assertion = assertion(
            "$.value",
            JsonMatcher::Equals(value.clone()),
            MatchSelection::One,
        );
        assert!(assertion.evaluate_json(&json!({"value": value})).success);
    }
    let assertion = assertion(
        "$.value",
        JsonMatcher::Equals(json!(42)),
        MatchSelection::One,
    );
    assert!(!assertion.evaluate_json(&json!({"value": "42"})).success);
    let assertion = JsonAssertion::new(JsonAssertionConfig::equals(
        "array",
        "$.value",
        json!([1, 2]),
    ))
    .unwrap();
    assert!(!assertion.evaluate_json(&json!({"value": [2, 1]})).success);
}

#[test]
fn missing_nodes_are_distinct_from_json_null() {
    let document = json!({"value": null});
    let null = assertion(
        "$.value",
        JsonMatcher::Equals(Value::Null),
        MatchSelection::One,
    );
    assert!(null.evaluate_json(&document).success);
    let missing = assertion(
        "$.missing",
        JsonMatcher::Equals(Value::Null),
        MatchSelection::One,
    );
    assert!(!missing.evaluate_json(&document).success);
    for (path, exists) in [("$.value", true), ("$.missing", false)] {
        assert_eq!(
            assertion(path, JsonMatcher::Exists, MatchSelection::One)
                .evaluate_json(&document)
                .success,
            exists
        );
        assert_eq!(
            assertion(path, JsonMatcher::NotExists, MatchSelection::One)
                .evaluate_json(&document)
                .success,
            !exists
        );
    }
}

#[test]
fn selection_requires_one_any_or_nonempty_all_matches() {
    let document = json!({"values": [1, 2, 2]});
    let matcher = JsonMatcher::Equals(json!(2));
    for (selection, expected) in [
        (MatchSelection::One, false),
        (MatchSelection::Any, true),
        (MatchSelection::All, false),
    ] {
        assert_eq!(
            assertion("$.values[*]", matcher.clone(), selection)
                .evaluate_json(&document)
                .success,
            expected
        );
    }
    assert!(
        assertion("$.values[1:]", matcher.clone(), MatchSelection::All)
            .evaluate_json(&document)
            .success
    );
    for selection in [
        MatchSelection::One,
        MatchSelection::Any,
        MatchSelection::All,
    ] {
        assert!(
            !assertion("$.missing[*]", matcher.clone(), selection)
                .evaluate_json(&document)
                .success
        );
    }
}

#[test]
fn supports_filters_recursive_selection_and_root_values() {
    let document = json!({"items": [{"id": 1, "name": "one"}, {"id": 2, "name": "two"}]});
    assert!(
        assertion(
            "$.items[?@.id == 2].name",
            JsonMatcher::Equals(json!("two")),
            MatchSelection::One
        )
        .evaluate_json(&document)
        .success
    );
    assert!(
        assertion(
            "$..name",
            JsonMatcher::Equals(json!("one")),
            MatchSelection::Any
        )
        .evaluate_json(&document)
        .success
    );
    assert!(
        assertion("$", JsonMatcher::Equals(json!(true)), MatchSelection::One)
            .evaluate_json(&json!(true))
            .success
    );
}

#[test]
fn regex_only_matches_string_nodes_and_respects_anchors() {
    let regex = assertion(
        "$.value",
        JsonMatcher::Regex("^user-[0-9]+$".into()),
        MatchSelection::One,
    );
    assert!(regex.evaluate_json(&json!({"value": "user-42"})).success);
    assert!(
        !regex
            .evaluate_json(&json!({"value": "prefix user-42"}))
            .success
    );
    assert!(!regex.evaluate_json(&json!({"value": 42})).success);
    assert!(
        assertion(
            "$.value",
            JsonMatcher::Regex("user".into()),
            MatchSelection::One
        )
        .evaluate_json(&json!({"value": "prefix-user"}))
        .success
    );
}

#[test]
fn invalid_paths_and_regexes_are_configuration_errors() {
    for path in ["$.items[", "not a path"] {
        let error = JsonAssertion::new(JsonAssertionConfig::equals("invalid path", path, json!(1)))
            .unwrap_err();
        assert_eq!(error.name, "invalid path");
        assert!(error.message.contains("JSONPath"));
    }
    let error = JsonAssertion::new(JsonAssertionConfig {
        name: "invalid regex".into(),
        path: "$".into(),
        matcher: JsonMatcher::Regex("[".into()),
        selection: MatchSelection::One,
    })
    .unwrap_err();
    assert!(error.message.contains("regex"));
}

#[test]
fn complete_body_is_independent_of_saved_response_body() {
    let mut sample = sample();
    sample.response_body = Some(b"{".to_vec());
    let assertion = assertion(
        "$.value",
        JsonMatcher::Equals(json!(42)),
        MatchSelection::One,
    );
    let input = AssertionInput {
        sample: &sample,
        response_body: Some(br#"{"value":42}"#),
        unavailable_reason: None,
    };
    assert!(
        assertion
            .evaluate(&input, &UserContext::new("test", 0))
            .success
    );
}

#[test]
fn unavailable_incomplete_and_invalid_json_are_failures() {
    let sample = sample();
    let assertion = assertion("$.missing", JsonMatcher::NotExists, MatchSelection::One);
    for input in [
        AssertionInput {
            sample: &sample,
            response_body: None,
            unavailable_reason: None,
        },
        AssertionInput {
            sample: &sample,
            response_body: Some(b"not JSON"),
            unavailable_reason: None,
        },
        AssertionInput {
            sample: &sample,
            response_body: Some(b"{}"),
            unavailable_reason: Some("incomplete response"),
        },
    ] {
        let result = assertion.evaluate(&input, &UserContext::new("test", 0));
        assert!(!result.success);
        assert!(result.message.is_some());
    }
}

#[test]
fn diagnostics_identify_the_assertion_path_expected_and_actual() {
    let result = JsonAssertion::new(JsonAssertionConfig::equals(
        "status check",
        "$.code",
        json!(0),
    ))
    .unwrap()
    .evaluate_json(&json!({"code": 7}));
    assert_eq!(result.name, "status check");
    let message = result.message.unwrap();
    assert!(message.contains("$.code"));
    assert!(message.contains("equals 0"));
    assert!(message.contains("[7]"));
}

#[test]
fn passing_assertions_never_restore_failed_samples() {
    let assertion = assertion(
        "$.value",
        JsonMatcher::Equals(json!(42)),
        MatchSelection::One,
    );
    let mut sample = sample();
    sample.success = false;
    sample.add_assertion(assertion.evaluate_json(&json!({"value": 42})));
    assert!(!sample.success);
    assert!(sample.assertions[0].success);
    sample.success = true;
    sample.assertions.clear();
    sample.add_assertion(assertion.evaluate_json(&json!({"value": 0})));
    assert!(!sample.success);
}
