// SPDX-License-Identifier: CC0-1.0

//! Unit tests for the readings [`super`] states over this service's own state: the
//! sentences, resolutions and limits that need no HTTP request to pin. The wire behaviour
//! they underlie is pinned end to end in this crate's `tests/`.

use super::*;

/// The keys a tag write may store, and the three ways a key is one Lambda refuses:
/// empty, in the reserved namespace, or named twice in one request.
#[test]
fn a_tag_key_is_storable_unless_it_is_empty_reserved_or_repeated() {
    let cases: &[(&str, &[&str], bool)] = &[
        ("no tags at all", &[], true),
        ("one ordinary key", &["Project"], true),
        (
            "several ordinary keys, including the punctuation the key pattern admits",
            &["Project", "cost.centre", "team/payments", "owner@example"],
            true,
        ),
        // `aws:` is a PREFIX rule, not a substring one: a key that merely contains it,
        // or starts with the letters without the colon, is the caller's own.
        ("a key whose name starts with the letters only", &["awsome"], true),
        ("a key carrying the prefix further in", &["cost:aws:share"], true),
        ("an empty key", &[""], false),
        ("an empty key beside an ordinary one", &["Project", ""], false),
        ("a reserved key", &["aws:cloudformation:stack-name"], false),
        (
            "a reserved key written in another case",
            &["AWS:CloudFormation:stack-name"],
            false,
        ),
        ("the prefix alone", &["aws:"], false),
        ("a reserved key beside an ordinary one", &["Project", "aws:created-by"], false),
        ("one key named twice", &["Project", "Project"], false),
    ];

    for (case, keys, expected) in cases {
        let actual = tag_keys_are_storable(keys.iter().copied());
        assert_eq!(
            actual, *expected,
            "case {case:?}: expected storable={expected}, got {actual:?} for {keys:?}"
        );
    }
}

/// Only an exact identifier the catalog names is deprecated, and nothing the catalog
/// names is one of the supported runtimes the rows below hold.
///
/// The supported set is written out here rather than read from the model: what is
/// under test is the CATALOG's membership, and a runtime identifier the model declares
/// is supported until the deprecation policy names it.
#[test]
fn a_runtime_is_deprecated_only_when_the_catalog_names_it_exactly() {
    // Values of `Runtime` (`smithy_query(query="shape", shape="Runtime")`) that the
    // deprecation policy does not name, so none of them is deprecated.
    let supported = [
        "nodejs20.x",
        "nodejs22.x",
        "python3.12",
        "python3.13",
        "java21",
        "dotnet8",
        "ruby3.3",
        "provided.al2",
        "provided.al2023",
    ];
    for runtime in supported {
        assert!(
            !is_deprecated_runtime(runtime),
            "case {runtime:?}: a supported runtime must not be read as deprecated"
        );
    }
    for runtime in DEPRECATED_RUNTIMES {
        assert!(
            is_deprecated_runtime(runtime),
            "case {runtime:?}: the catalog's own entry must be read as deprecated"
        );
        assert!(
            !supported.contains(runtime),
            "case {runtime:?}: a runtime cannot be both supported and deprecated"
        );
    }
    // The match is exact: a near miss of a catalogued identifier is a different
    // runtime, not a deprecated one.
    for runtime in ["python3", "python3.80", "java", "go1", "nodejs18"] {
        assert!(
            !is_deprecated_runtime(runtime),
            "case {runtime:?}: only an exact identifier is deprecated"
        );
    }
}

#[test]
fn a_reported_instant_sorts_as_a_string_exactly_as_it_does_as_an_instant() {
    // `LastModified` is reported as a string, so a caller ordering two reads of it
    // has only the string to order them by. The offsets are the subsecond values a
    // trimming formatter renders at different widths: none at all, one nanosecond,
    // a value whose trimmed rendering is a PREFIX of the next one, and each end of
    // the field.
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_793_000_000);
    let offsets = [
        Duration::ZERO,
        Duration::from_nanos(1),
        Duration::from_millis(848),
        Duration::from_nanos(848_766_000),
        Duration::from_millis(999),
        Duration::new(0, 999_999_999),
        Duration::from_secs(1),
    ];
    let width = "2026-10-23T17:13:43.848+0000".len();
    let mut earlier: Option<(Duration, String)> = None;
    for offset in offsets {
        let rendered = iso8601(base + offset);
        assert_eq!(
            rendered.len(),
            width,
            "case {offset:?}: a reported instant is rendered at a fixed width, got \
                 {rendered:?}"
        );
        if let Some((before, rendered_before)) = &earlier {
            assert!(
                rendered_before.as_str() <= rendered.as_str(),
                "case {offset:?}: {rendered:?} is at or after {rendered_before:?} \
                     ({before:?}) as an instant, so it must be as a string too"
            );
        }
        earlier = Some((offset, rendered));
    }
}

#[test]
fn every_documented_function_name_form_resolves_to_one_record() {
    let cases = [
        ("my-function", "my-function", None),
        ("my-function:2", "my-function", Some("2")),
        ("function:my-function", "my-function", None),
        ("000000000000:function:my-function", "my-function", None),
        (
            "000000000000:function:my-function:4",
            "my-function",
            Some("4"),
        ),
        // A function name may itself be twelve digits, and the documented partial
        // ARN always carries the `function:` literal, so this is a qualified name
        // rather than an account id.
        ("123456789012:2", "123456789012", Some("2")),
        (
            "arn:aws:lambda:us-east-1:000000000000:function:my-function",
            "my-function",
            None,
        ),
        (
            "arn:aws:lambda:us-east-1:000000000000:function:my-function:$LATEST",
            "my-function",
            Some("$LATEST"),
        ),
    ];
    for (input, name, qualifier) in cases {
        let parsed = parse_function_reference(input)
            .unwrap_or_else(|| panic!("case {input:?}: expected a parse, got None"));
        assert_eq!(
            parsed.name, name,
            "case {input:?}: expected name {name:?}, got {:?}",
            parsed.name
        );
        assert_eq!(
            parsed.qualifier.as_deref(),
            qualifier,
            "case {input:?}: expected qualifier {qualifier:?}, got {:?}",
            parsed.qualifier
        );
    }
}

#[test]
fn a_taggable_resource_arn_names_the_resource_it_carries() {
    // One row per alternative the `TaggableResource` pattern admits
    // (`smithy_query(query="operation", operation="ListTags")`), plus a value that
    // is no `lambda` resource ARN at all.
    let cases: Vec<(&str, Option<TaggableResourceRef>)> = vec![
        (
            "arn:aws:lambda:us-east-1:000000000000:function:my-function",
            Some(TaggableResourceRef::Function("my-function".to_string())),
        ),
        (
            "arn:aws-us-gov:lambda:us-gov-west-1:000000000000:function:my-function",
            Some(TaggableResourceRef::Function("my-function".to_string())),
        ),
        (
            "arn:aws:lambda:us-east-1:000000000000:function:my-function:3",
            Some(TaggableResourceRef::FunctionQualifier {
                name: "my-function".to_string(),
                qualifier: "3".to_string(),
            }),
        ),
        (
            "arn:aws:lambda:us-east-1:000000000000:function:my-function:$LATEST",
            Some(TaggableResourceRef::FunctionQualifier {
                name: "my-function".to_string(),
                qualifier: "$LATEST".to_string(),
            }),
        ),
        (
            "arn:aws:lambda:us-east-1:000000000000:function:my-function:blue",
            Some(TaggableResourceRef::FunctionQualifier {
                name: "my-function".to_string(),
                qualifier: "blue".to_string(),
            }),
        ),
        (
            "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0123456789abcdefg",
            Some(TaggableResourceRef::CodeSigningConfig(
                "arn:aws:lambda:us-east-1:000000000000:code-signing-config:\
                     csc-0123456789abcdefg"
                    .to_string(),
            )),
        ),
        (
            "arn:aws:lambda:us-east-1:000000000000:event-source-mapping:\
                 12345678-1234-1234-1234-123456789012",
            Some(TaggableResourceRef::EventSourceMapping(
                "12345678-1234-1234-1234-123456789012".to_string(),
            )),
        ),
        (
            "arn:aws:lambda:us-east-1:000000000000:capacity-provider:fleet-1",
            Some(TaggableResourceRef::CapacityProvider("fleet-1".to_string())),
        ),
        (
            "arn:aws:lambda:us-east-1:000000000000:network-connector:link-1",
            Some(TaggableResourceRef::Other("network-connector".to_string())),
        ),
        // `layer:([a-zA-Z0-9-_]+)` — a layer is a taggable Lambda resource this
        // store holds no records of, so it is named by its resource type.
        (
            "arn:aws:lambda:us-east-1:000000000000:layer:my-layer",
            Some(TaggableResourceRef::Other("layer".to_string())),
        ),
        ("arn:aws:sqs:us-east-1:000000000000:queue:q", None),
        ("my-function", None),
    ];
    for (input, expected) in cases {
        let actual = parse_taggable_resource(input);
        assert_eq!(
            actual, expected,
            "case {input:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[test]
fn a_value_that_is_no_documented_form_does_not_resolve() {
    for input in [
        "",
        ":",
        "my-function:",
        "arn:aws:s3:us-east-1:000000000000:function:my-function",
        "arn:aws:lambda:us-east-1:000000000000:layer:my-layer",
        "arn:aws:lambda:us-east-1:000000000000:my-function",
        "my-function:1:2:3",
    ] {
        assert!(
            parse_function_reference(input).is_none(),
            "case {input:?}: expected None, got {:?}",
            parse_function_reference(input)
        );
    }
}
