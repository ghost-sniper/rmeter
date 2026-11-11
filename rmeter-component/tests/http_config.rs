use std::time::Duration;

use rmeter_component::{
    HTTPRequestDefaults, HTTPSampler, HttpNode, HttpRequestConfig, HttpRequestDefaults,
    HttpSampler, Setting, compile_http_nodes,
};
use rmeter_core::{
    context::UserContext,
    controller::{CursorStep, ExecutionCursor},
    plan::{LoopCount, Node},
};

#[test]
fn explicit_values_override_defaults_without_losing_other_settings() {
    let defaults = HttpRequestDefaults {
        host: Some("example.test".into()),
        path: Some("/default".into()),
        follow_redirects: Some(true),
        request_timeout: Setting::Value(Some(Duration::from_secs(5))),
        headers: Some(vec![
            ("X-Shared".into(), "kept".into()),
            ("X-Override".into(), "outer".into()),
        ]),
        query: Some(vec![
            ("kept".into(), "1".into()),
            ("key".into(), "outer".into()),
        ]),
        ..HttpRequestDefaults::default()
    };
    let request = HttpRequestDefaults {
        path: Some("/request".into()),
        follow_redirects: Some(false),
        request_timeout: Setting::Value(None),
        headers: Some(vec![("x-override".into(), "inner".into())]),
        query: Some(vec![
            ("key".into(), "one".into()),
            ("key".into(), "two".into()),
        ]),
        ..HttpRequestDefaults::default()
    };
    let merged = defaults.merged_with(&request);
    assert_eq!(merged.host, defaults.host);
    assert_eq!(merged.path.as_deref(), Some("/request"));
    assert_eq!(merged.follow_redirects, Some(false));
    assert_eq!(merged.request_timeout, Setting::Value(None));
    assert_eq!(
        merged.headers.unwrap(),
        vec![
            ("X-Shared".into(), "kept".into()),
            ("x-override".into(), "inner".into()),
        ]
    );
    assert_eq!(
        merged.query.unwrap(),
        vec![
            ("kept".into(), "1".into()),
            ("key".into(), "one".into()),
            ("key".into(), "two".into()),
        ]
    );
}

#[test]
fn sampler_lists_replace_independently_and_keep_scalar_inheritance() {
    let defaults = HttpRequestDefaults {
        host: Some("example.test".into()),
        path: Some("/default".into()),
        request_timeout: Setting::Value(Some(Duration::from_secs(5))),
        headers: Some(vec![("X-Default".into(), "outer".into())]),
        query: Some(vec![("default".into(), "outer".into())]),
        ..Default::default()
    };
    for headers in [
        None,
        Some(vec![]),
        Some(vec![
            ("X-Own".into(), "one".into()),
            ("X-Own".into(), "two".into()),
        ]),
    ] {
        for query in [
            None,
            Some(vec![]),
            Some(vec![
                ("own".into(), "one".into()),
                ("own".into(), "two".into()),
            ]),
        ] {
            let mut request = HttpRequestConfig::new("override");
            request.settings.headers = headers.clone();
            request.settings.query = query.clone();
            request.settings.path = Some("/own".into());
            request.settings.follow_redirects = Some(false);
            let sampler = HttpSampler::with_defaults(request, &defaults).unwrap();
            let actual = &sampler.config().settings;
            assert_eq!(
                actual.headers,
                headers.clone().or_else(|| defaults.headers.clone())
            );
            assert_eq!(actual.query, query.or_else(|| defaults.query.clone()));
            assert_eq!(actual.host, defaults.host);
            assert_eq!(actual.path.as_deref(), Some("/own"));
            assert_eq!(actual.request_timeout, defaults.request_timeout);
            assert_eq!(actual.follow_redirects, Some(false));
        }
    }
}

#[test]
fn empty_collections_clear_inherited_values() {
    let outer = HttpRequestDefaults {
        headers: Some(vec![("X-Default".into(), "value".into())]),
        query: Some(vec![("key".into(), "value".into())]),
        ..HttpRequestDefaults::default()
    };
    let inner = HttpRequestDefaults {
        headers: Some(vec![]),
        query: Some(vec![]),
        ..HttpRequestDefaults::default()
    };
    assert_eq!(outer.merged_with(&inner).headers, Some(vec![]));
    assert_eq!(outer.merged_with(&inner).query, Some(vec![]));
}

#[test]
fn scoped_defaults_apply_regardless_of_position_and_do_not_leak() {
    let inherited = HTTPRequestDefaults {
        host: Some("example.test".into()),
        ..Default::default()
    };
    let mut explicit = HttpRequestConfig::new("explicit");
    explicit.settings.path = Some("/explicit".into());
    let nodes = compile_http_nodes(
        vec![
            HttpNode::Request(HttpRequestConfig::new("before")),
            HttpNode::Sequence(vec![
                HttpNode::Request(HttpRequestConfig::new("inner")),
                HttpNode::Defaults(HttpRequestDefaults {
                    path: Some("/inner".into()),
                    ..Default::default()
                }),
                HttpNode::Request(explicit),
            ]),
            HttpNode::Loop {
                count: LoopCount::Finite(2),
                children: vec![
                    HttpNode::Defaults(HttpRequestDefaults {
                        host: Some("loop.test".into()),
                        ..Default::default()
                    }),
                    HttpNode::Request(HttpRequestConfig::new("loop")),
                ],
            },
            HttpNode::Request(HttpRequestConfig::new("after")),
            HttpNode::Defaults(HttpRequestDefaults {
                path: Some("/old".into()),
                ..Default::default()
            }),
            HttpNode::Defaults(HttpRequestDefaults {
                path: Some("/outer".into()),
                ..Default::default()
            }),
        ],
        &inherited,
    )
    .unwrap();
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = UserContext::new("test", 0);
    let mut configs = Vec::new();
    loop {
        match cursor.step(&mut context).unwrap() {
            CursorStep::Sample(sampler) => configs.push((
                sampler.config().label.as_str(),
                sampler.config().settings.host.as_deref().unwrap(),
                sampler.config().settings.path.as_deref().unwrap(),
            )),
            CursorStep::Yield => continue,
            CursorStep::Done => break,
        }
    }
    assert_eq!(
        configs,
        vec![
            ("before", "example.test", "/outer"),
            ("inner", "example.test", "/inner"),
            ("explicit", "example.test", "/explicit"),
            ("loop", "loop.test", "/outer"),
            ("loop", "loop.test", "/outer"),
            ("after", "example.test", "/outer"),
        ]
    );
    assert!(matches!(nodes[1], Node::Sequence(_)));
    assert!(matches!(
        nodes[2],
        Node::Loop {
            count: LoopCount::Finite(2),
            ..
        }
    ));
}

#[test]
fn rejects_invalid_static_requests_and_templates_before_execution() {
    assert!(HttpSampler::new(HttpRequestConfig::new("missing host")).is_err());
    for url in [
        "ftp://example.test/",
        "not a URL",
        "http://",
        "http://${}/",
        "http://${host/",
    ] {
        assert!(
            HttpSampler::new(HttpRequestConfig::with_url("invalid", url)).is_err(),
            "{url}"
        );
    }
    for host in [
        "http://example.test",
        "example.test:80",
        "user@example.test",
        "example.test/path",
    ] {
        let mut request = HttpRequestConfig::new("invalid host");
        request.settings.host = Some(host.into());
        assert!(HttpSampler::new(request).is_err(), "{host}");
    }
    let mut request = HttpRequestConfig::with_url("header", "http://example.test/");
    request.settings.headers = Some(vec![("Bad Header".into(), "value".into())]);
    assert!(HttpSampler::new(request).is_err());
    let mut request = HttpRequestConfig::with_url("timeout", "http://example.test/");
    request.settings.request_timeout = Setting::Value(Some(Duration::MAX));
    assert!(HttpSampler::new(request).is_err());
}

#[test]
fn accepts_https_ipv6_and_absolute_url_overrides() {
    let _: HTTPSampler =
        HttpSampler::new(HttpRequestConfig::with_url("tls", "https://example.test/")).unwrap();
    let mut request = HttpRequestConfig::new("ipv6");
    request.settings.host = Some("::1".into());
    request.settings.port = Some(8080);
    HttpSampler::new(request).unwrap();
    let defaults = HttpRequestDefaults {
        host: Some("ignored/invalid".into()),
        ..Default::default()
    };
    HttpSampler::with_defaults(
        HttpRequestConfig::with_url("absolute", "http://example.test/"),
        &defaults,
    )
    .unwrap();
}
