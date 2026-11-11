// SPDX-License-Identifier: CC0-1.0

//! The unit tests of [`super`]: what this service's state module decides on its own,
//! asserted against the functions that decide it rather than over the wire.
//!
//! Split out of [`super`] so the module root holds the state and the rules and this file
//! holds their tests; the end-to-end assertions live in the crate's `tests/` suites.

use super::*;

/// Every not-found sentence names the KIND of resource and never a resource name,
/// and carries the discriminator for that kind.
#[test]
fn a_not_found_names_the_resource_kind_and_no_identifier() {
    for (kind, expected_type, noun) in [
        (ResourceKind::ChannelGroup, "CHANNEL_GROUP", "channel group"),
        (ResourceKind::Channel, "CHANNEL", "channel"),
        (
            ResourceKind::OriginEndpoint,
            "ORIGIN_ENDPOINT",
            "origin endpoint",
        ),
    ] {
        let error = kind.not_found();
        let message = error.message.clone().unwrap_or_default();
        assert!(
            message.contains(noun),
            "case {kind:?}: expected a message naming {noun:?}, got {message:?}"
        );
        assert_eq!(
            error.resource_type_not_found.as_deref(),
            Some(expected_type),
            "case {kind:?}: wrong ResourceTypeNotFound, got {:?}",
            error.resource_type_not_found
        );
    }
    // "an origin endpoint", not "a origin endpoint".
    let endpoint = ResourceKind::OriginEndpoint
        .not_found()
        .message
        .unwrap_or_default();
    assert!(
        endpoint.contains("add an origin endpoint"),
        "case OriginEndpoint: expected the `an` article, got {endpoint:?}"
    );
}

/// Each constraint class maps to its own `ValidationExceptionType`, and the message
/// names the member path that failed without echoing the value that failed it.
#[test]
fn a_violation_sets_both_modeled_members() {
    let cases = [
        (
            ViolationKind::Pattern("^[a-zA-Z0-9_-]+$".to_string()),
            "MEMBER_DOES_NOT_MATCH_PATTERN",
        ),
        (ViolationKind::MissingRequiredMember, "MEMBER_MISSING"),
        (ViolationKind::MinLength(1), "MEMBER_MIN_LENGTH"),
        (ViolationKind::MaxLength(256), "MEMBER_MAX_LENGTH"),
        (
            ViolationKind::MinValue("1".to_string()),
            "MEMBER_MIN_VALUE",
        ),
        (
            ViolationKind::MaxValue("100".to_string()),
            "MEMBER_MAX_VALUE",
        ),
        (
            ViolationKind::EnumValue(vec!["CMAF".to_string()]),
            "MEMBER_INVALID_ENUM_VALUE",
        ),
        (ViolationKind::UniqueItems, "MEMBER_INVALID"),
    ];
    for (kind, expected_type) in cases {
        let violation = Violation {
            path: "channelGroupName".to_string(),
            kind: kind.clone(),
            value: Some("bad name!".to_string()),
            sensitive: false,
        };
        let error = member_validation_failed(&violation);
        assert_eq!(
            error.validation_exception_type.as_deref(),
            Some(expected_type),
            "case {kind:?}: wrong ValidationExceptionType, got {:?}",
            error.validation_exception_type
        );
        let message = error.message.clone().unwrap_or_default();
        assert!(
            message.starts_with("Validation error encountered at request.channelGroupName: "),
            "case {kind:?}: expected the member path in the message, got {message:?}"
        );
        assert!(
            !message.contains("bad name!"),
            "case {kind:?}: the rejected value must not be echoed, got {message:?}"
        );
    }
}

/// The character-set rendering a `@pattern` refusal states, over the pattern forms
/// this service's model uses plus the ones it must decline to describe.
#[test]
fn a_character_class_pattern_is_stated_as_its_supported_characters() {
    let cases = [
        // `ResourceName`, the shape every resource-name member targets.
        (
            "^[a-zA-Z0-9_-]+$",
            Some("A-Z, a-z, 0-9, _ (underscore), and - (hyphen)"),
        ),
        // A two-item class, which joins with `and` and no comma.
        ("^[a-z0-9]+$", Some("a-z and 0-9")),
        // A one-item class.
        ("^[A-Z]+$", Some("A-Z")),
        // A leading `-` is the literal hyphen, and the ranges are listed first.
        ("^[-a-z]+$", Some("a-z and - (hyphen)")),
        // A range that is not one run of letters or digits (regex reads `_-x` as the
        // code points between them) has no listing, so the whole pattern declines.
        ("^[a-z_-x]+$", None),
        // A descending range is not a range a message can state either.
        ("^[z-a]+$", None),
        // `EntityTag` / `IdempotencyToken`: an escape names no listable set.
        (r"^[\S]+$", None),
        // A negated class states what is forbidden, not what is supported.
        ("^[^a-z]+$", None),
        // Not a single character class at all.
        ("^arn:aws:mediapackagev2:", None),
    ];
    for (pattern, expected) in cases {
        let rendered = pattern_character_set(pattern);
        assert_eq!(
            rendered.as_deref(),
            expected,
            "case {pattern:?}: expected {expected:?}, got {rendered:?}"
        );
    }
}

/// A `@pattern` violation is stated as the characters the member may hold, and every
/// other constraint class as the bound it broke — each followed by the sentence
/// telling the caller to check the value it sent.
#[test]
fn a_constraint_detail_states_the_bound_that_was_broken() {
    let cases = [
        (
            ViolationKind::Pattern("^[a-zA-Z0-9_-]+$".to_string()),
            "Member must contain only supported characters. Supported characters are \
                 A-Z, a-z, 0-9, _ (underscore), and - (hyphen). Check the value you \
                 specified in your request and then try again",
        ),
        (
            ViolationKind::Pattern(r"^[\S]+$".to_string()),
            "Member must satisfy regular expression pattern: ^[\\S]+$. Check the \
                 value you specified in your request and then try again",
        ),
        (
            ViolationKind::MaxLength(256),
            "Member must have length less than or equal to 256. Check the value you \
                 specified in your request and then try again",
        ),
        (
            ViolationKind::MissingRequiredMember,
            "Member must not be null. Check the value you specified in your request \
                 and then try again",
        ),
    ];
    for (kind, expected) in cases {
        let detail = constraint_detail(&kind);
        assert_eq!(
            detail, expected,
            "case {kind:?}: expected {expected:?}, got {detail:?}"
        );
    }
}

/// Every collection a paged list of this service walks, as a token is bound to it.
fn page_scopes() -> Vec<PageScope<'static>> {
    vec![
        PageScope::ChannelGroups {
            region: "us-east-1",
        },
        PageScope::ChannelGroups {
            region: "eu-west-1",
        },
        PageScope::Channels {
            region: "us-east-1",
            channel_group: "grp",
        },
        PageScope::OriginEndpoints {
            region: "us-east-1",
            channel_group: "grp",
            channel: "chan",
        },
        PageScope::HarvestJobs {
            region: "us-east-1",
            channel_group: "grp",
        },
    ]
}

#[test]
fn a_page_token_round_trips_the_name_it_stopped_after() {
    for scope in page_scopes() {
        for name in ["a", "endpoint-1", "A_long_Name-with-9-digits"] {
            let token = page_token(scope, name);
            assert_eq!(
                page_resume_after(scope, &token).as_deref(),
                Some(name),
                "case {scope:?}/{name:?}: the token {token:?} must decode back to the name"
            );
        }
    }
}

#[test]
fn a_token_this_service_did_not_issue_does_not_decode() {
    let scope = PageScope::ChannelGroups {
        region: "us-east-1",
    };
    let issued = page_token(scope, "grp");
    // Tokens differing in FORM, none of them one this service issued: prose, non-hex
    // characters, an odd-length hex run, a base64-shaped string, a bare hex run that
    // decodes to a name but carries no binding digest, a digest with no cursor behind
    // it, and an issued token with its cursor truncated.
    for token in [
        "not-a-token",
        "zzzz",
        "abc",
        "eyJrZXkiOiAidmFsdWUifQ==",
        "6d",
        &issued[..PAGE_TOKEN_DIGEST_LEN],
        &issued[..issued.len() - 2],
    ] {
        assert!(
            page_resume_after(scope, token).is_none(),
            "case {token:?}: a token this service did not issue must not decode"
        );
    }
    // And a token this service DID issue, for another of its collections: the digest
    // binds the cursor to the collection, so it is not readable outside it.
    for other in page_scopes().into_iter().skip(1) {
        let token = page_token(other, "grp");
        assert!(
            page_resume_after(scope, &token).is_none(),
            "case {other:?}: a token issued for another collection must not decode, \
                 got {:?}",
            page_resume_after(scope, &token)
        );
    }
}

#[test]
fn a_policy_document_is_a_json_object_with_a_statement() {
    for accepted in [
        r#"{"Statement":[]}"#,
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow"}]}"#,
        "{\n  \"Statement\": {\"Effect\": \"Deny\"}\n}",
    ] {
        assert!(
            is_policy_document(accepted),
            "case {accepted:?}: expected a valid policy document"
        );
    }
    for refused in [
        "",
        "not json at all",
        r#"{"Statement":[]"#,
        r#"[{"Statement":[]}]"#,
        r#""{\"Statement\":[]}""#,
        r#"{"Version":"2012-10-17"}"#,
    ] {
        assert!(
            !is_policy_document(refused),
            "case {refused:?}: expected to be refused as a policy document"
        );
    }
}

#[test]
fn an_arn_pattern_matches_the_way_iam_resource_patterns_do() {
    // `*` is any run of characters (including none) and `?` is exactly one; the match
    // is anchored at both ends.
    let arn = "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g/channel/c\
                   /originEndpoint/e";
    for pattern in [
        arn,
        "*",
        "arn:aws:mediapackagev2:*",
        "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g/channel/c*",
        "arn:*:mediapackagev2:*:*:channelGroup/g/*/originEndpoint/e",
        "arn:aws:mediapackagev2:us-west-?:000000000000:channelGroup/g/channel/c\
             /originEndpoint/e",
    ] {
        assert!(
            arn_pattern_matches(pattern, arn),
            "case {pattern:?}: expected it to match {arn}"
        );
    }
    for pattern in [
        "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g/channel/c\
             /originEndpoint/other",
        "arn:aws:s3:*",
        "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g",
        // `?` is exactly one character, so it cannot swallow a whole Region.
        "arn:aws:mediapackagev2:?:000000000000:channelGroup/g/channel/c\
             /originEndpoint/e",
    ] {
        assert!(
            !arn_pattern_matches(pattern, arn),
            "case {pattern:?}: expected it NOT to match {arn}"
        );
    }
}

#[test]
fn an_anonymous_grant_needs_an_effect_an_action_a_principal_and_a_reachable_resource() {
    let arn = "arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/g/channel/c\
                   /originEndpoint/e";
    let action = "mediapackagev2:GetObject";
    let statement = |members: &str| format!("{{\"Statement\":[{members}]}}");

    for granting in [
        r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject"}"#,
        r#"{"Effect":"Allow","Principal":{"AWS":"*"},"Action":["mediapackagev2:*"]}"#,
        r#"{"Effect":"Allow","Principal":"*","Action":"*","Resource":"*"}"#,
    ] {
        assert!(
            policy_allows_anonymous(&statement(granting), action, arn),
            "case {granting:?}: expected an anonymous grant"
        );
    }
    for refusing in [
        // A named principal is not the anonymous one.
        r#"{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},
                 "Action":"mediapackagev2:GetObject"}"#,
        // A different action.
        r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:PutObject"}"#,
        // A resource this request does not address.
        r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject",
                 "Resource":"arn:aws:mediapackagev2:us-west-2:000000000000:channelGroup/other"}"#,
        // A member the evaluator cannot judge: it must not grant past it.
        r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject",
                 "Condition":{"Bool":{"aws:SecureTransport":"true"}}}"#,
        r#"{"Effect":"Deny","Principal":"*","Action":"mediapackagev2:GetObject"}"#,
    ] {
        assert!(
            !policy_allows_anonymous(&statement(refusing), action, arn),
            "case {refusing:?}: expected no anonymous grant"
        );
    }
    // Deny beats Allow, whichever order they are written in.
    let both = r#"{"Effect":"Allow","Principal":"*","Action":"mediapackagev2:GetObject"},
             {"Effect":"Deny","Principal":"*","Action":"mediapackagev2:GetObject"}"#;
    assert!(
        !policy_allows_anonymous(&statement(both), action, arn),
        "a Deny of the same action must beat the Allow"
    );
}

#[test]
fn an_etag_separates_fields() {
    assert_ne!(
        etag(&["a".to_string(), "bc".to_string()]),
        etag(&["ab".to_string(), "c".to_string()]),
        "field boundaries must be part of the digest"
    );
}

#[test]
fn materialization_fills_only_what_the_caller_left_out() {
    let submitted = Segment {
        segment_duration_seconds: Some(12),
        ts_include_dvb_subtitles: Some(false),
        ..Segment::default()
    };
    // On a channel whose output locking mode carries no output-timestamp mode, so the
    // member stays absent; the carrying case is the table in
    // `tests/all/segment_output_timestamp_mode.rs`.
    let materialized = materialized_segment(Some(submitted), Some(DEFAULT_OUTPUT_LOCKING_MODE));
    assert_eq!(
        materialized.segment_duration_seconds,
        Some(12),
        "a submitted duration must survive materialization"
    );
    assert_eq!(
        materialized.ts_include_dvb_subtitles,
        Some(false),
        "a submitted toggle must survive materialization, even when it is off"
    );
    assert_eq!(
        materialized.segment_name.as_deref(),
        Some(DEFAULT_SEGMENT_NAME),
        "an omitted segment name must come out as the service's default"
    );
    assert!(
        materialized.scte.is_none() && materialized.output_timestamp_mode.is_none(),
        "materialization must not invent members the service reports only when configured"
    );
}

#[test]
fn the_ingest_host_shares_the_groups_dns() {
    let egress = egress_domain("us-west-2", "group");
    let endpoints = ingest_endpoints(&egress, "us-west-2", "group", "channel", DEFAULT_INPUT_TYPE);
    let suffix = egress.split_once(".egress.").expect("an egress label").1;
    assert!(
        !endpoints.is_empty(),
        "a channel that receives ingest must publish ingest endpoints"
    );
    for endpoint in &endpoints {
        let url = endpoint.url.as_deref().expect("an ingest url");
        assert!(
            url.contains(suffix),
            "ingest url {url} must share the group's DNS suffix {suffix}"
        );
    }
    assert!(
        ingest_endpoints(&egress, "us-west-2", "group", "channel", INPUT_TYPE_MULTIVIEW).is_empty(),
        "a channel that receives no ingest of its own must publish no ingest endpoint"
    );
}

/// One row of the input-type restriction table: which member a request carries, the
/// channel's input type, and the `ValidationExceptionType` the combination is refused
/// with (`None` when the input type may carry it).
type RestrictionCase = (
    &'static str,
    &'static str,
    fn(&mut SubmittedChannelMembers),
    Option<&'static str>,
);

/// A member the model restricts to particular `InputType`s is refused on every other
/// input type, with that member's own discriminator — and `CreateChannel` and
/// `UpdateChannel` read the SAME table, so the two cannot disagree about it.
///
/// One row per restriction the model states, on an input type that forbids it and on
/// one that allows it.
#[test]
fn a_member_is_refused_on_an_input_type_that_may_not_carry_it() {
    let cases: &[RestrictionCase] = &[
        (
            "output locking mode on HLS",
            "HLS",
            |present| present.output_locking_mode = true,
            Some("ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE"),
        ),
        (
            "output locking mode on CMAF",
            "CMAF",
            |present| present.output_locking_mode = true,
            None,
        ),
        (
            "output locking mode on MULTIVIEW",
            "MULTIVIEW",
            |present| present.output_locking_mode = true,
            None,
        ),
        (
            "MQCS switching on HLS",
            "HLS",
            |present| present.mqcs_input_switching = true,
            Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING"),
        ),
        (
            "MQCS switching on CMAF",
            "CMAF",
            |present| present.mqcs_input_switching = true,
            None,
        ),
        (
            "preferred input on MULTIVIEW",
            "MULTIVIEW",
            |present| present.preferred_input = true,
            Some("ONLY_CMAF_INPUT_TYPE_ALLOW_PREFERRED_INPUT_CONFIGURATION"),
        ),
        (
            "CMSD publication on HLS",
            "HLS",
            |present| present.publish_mqcs = true,
            Some("ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_OUTPUT_CONFIGURATION"),
        ),
        (
            "multiview configuration on CMAF",
            "CMAF",
            |present| present.multiview_configuration = true,
            Some("MULTIVIEW_CONFIGURATION_NOT_ALLOWED"),
        ),
        (
            "multiview configuration on MULTIVIEW",
            "MULTIVIEW",
            |present| present.multiview_configuration = true,
            None,
        ),
        (
            "no restricted member at all on HLS",
            "HLS",
            |_| {},
            None,
        ),
    ];

    for (label, input_type, carry, expected) in cases {
        let mut submitted = SubmittedChannelMembers::default();
        carry(&mut submitted);
        let violation = channel_input_type_violation(submitted, input_type);
        match expected {
            Some(expected) => {
                let violation = violation.unwrap_or_else(|| {
                    panic!("case {label:?}: expected a refusal, got none")
                });
                assert_eq!(
                    violation.validation_exception_type.as_deref(),
                    Some(*expected),
                    "case {label:?}: expected {expected}, got {violation:?}"
                );
                assert!(
                    violation
                        .message
                        .as_deref()
                        .unwrap_or_default()
                        .starts_with("Validation error encountered at request"),
                    "case {label:?}: expected this service's message form, got {:?}",
                    violation.message
                );
            }
            None => assert!(
                violation.is_none(),
                "case {label:?}: expected no refusal, got {violation:?}"
            ),
        }
    }
}

/// A VALUE an input type that CARRIES a member may not take is refused on that input
/// type and accepted on the ones the restriction does not name — the whole
/// `InputType` enum against the whole `OutputLockingMode` enum, plus the absent case.
///
/// The catalogues are the model's own
/// (`smithy_query(query="shape", shape="InputType,OutputLockingMode")`): `InputType`
/// is HLS | CMAF | MULTIVIEW and `OutputLockingMode` is EPOCH_LOCKED |
/// NON_EPOCH_LOCKED.
#[test]
fn a_value_an_input_type_may_not_take_is_refused_on_that_input_type() {
    for input_type in ["HLS", "CMAF", INPUT_TYPE_MULTIVIEW] {
        for submitted in [None, Some(DEFAULT_OUTPUT_LOCKING_MODE), Some("NON_EPOCH_LOCKED")] {
            let label = format!("{input_type} with {submitted:?}");
            let violation = channel_output_locking_mode_value_violation(submitted, input_type);
            let refused = submitted == Some(MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE)
                && input_type == INPUT_TYPE_MULTIVIEW;
            assert_eq!(
                violation.is_some(),
                refused,
                "case {label:?}: this combination being refused is {refused}, got \
                     {violation:?}"
            );
            let Some(violation) = violation else { continue };
            assert_eq!(
                violation.validation_exception_type.as_deref(),
                Some(VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED),
                "case {label:?}: expected \
                     {VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED}, got \
                     {violation:?}"
            );
            let message = violation.message.as_deref().unwrap_or_default();
            assert!(
                message.starts_with(&format!(
                    "Validation error encountered at request.{OUTPUT_LOCKING_MODE_MEMBER}:"
                )),
                "case {label:?}: expected this service's message form at the member \
                     the refusal is about, got {message:?}"
            );
        }
    }
}

/// An endpoint member the model restricts to particular parent-channel input types is
/// materialised only under a channel whose input type may carry it: the documented
/// default is applied there and withheld everywhere else, and a SUBMITTED value is
/// refused where the member may not be carried — both read off the one allowed-input-type
/// list, so they cannot disagree.
///
/// The input-type catalogue is the model's own `InputType` enum
/// (`smithy_query(query="shape", shape="InputType")` — HLS | CMAF | MULTIVIEW), and the
/// submitted modes are the whole `StreamNameOutputMode` enum.
#[test]
fn a_restricted_endpoint_member_is_materialised_only_where_it_may_be_carried() {
    for input_type in ["HLS", "CMAF", "MULTIVIEW"] {
        let carries = input_type_carries_stream_name_output_mode(input_type);
        for submitted in [None, Some("INDEX"), Some("PASSTHROUGH_NAME")] {
            let label = format!("{input_type} with {submitted:?}");
            let stored = materialized_stream_name_output_mode(
                submitted.map(str::to_string),
                input_type,
            );
            let expected = if carries {
                Some(submitted.unwrap_or(DEFAULT_STREAM_NAME_OUTPUT_MODE).to_string())
            } else {
                None
            };
            assert_eq!(
                stored, expected,
                "case {label:?}: expected {expected:?}, got {stored:?}"
            );

            let violation = stream_name_output_mode_input_type_violation(submitted, input_type);
            match (submitted, carries) {
                (Some(_), false) => {
                    let violation = violation.unwrap_or_else(|| {
                        panic!("case {label:?}: expected a refusal, got none")
                    });
                    assert_eq!(
                        violation.validation_exception_type.as_deref(),
                        Some("ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE"),
                        "case {label:?}: expected the input-type discriminator, got \
                             {violation:?}"
                    );
                }
                _ => assert!(
                    violation.is_none(),
                    "case {label:?}: expected no refusal, got {violation:?}"
                ),
            }
        }
    }
}

/// A stored origin-endpoint record, as the baseline of the table below.
fn sample_endpoint() -> OriginEndpoint {
    let at = now();
    OriginEndpoint {
        name: "endpoint".to_string(),
        channel_name: "channel".to_string(),
        group_name: "group".to_string(),
        arn: origin_endpoint_arn("us-west-2", "group", "channel", "endpoint"),
        container_type: "TS".to_string(),
        segment: materialized_segment(None, None),
        description: None,
        startover_window_seconds: materialized_startover_window_seconds(
            None,
            DEFAULT_INPUT_TYPE,
        ),
        hls_manifests: Vec::new(),
        low_latency_hls_manifests: Vec::new(),
        dash_manifests: Vec::new(),
        mss_manifests: Vec::new(),
        force_endpoint_error_configuration: None,
        uri_separator: materialized_uri_separator(None),
        stream_name_output_mode: materialized_stream_name_output_mode(
            None,
            DEFAULT_INPUT_TYPE,
        ),
        created_at: at,
        modified_at: at,
        reset_at: None,
        version: 1,
        etag: String::new(),
        tags: HashMap::new(),
        client_token: None,
        fingerprint: String::new(),
        policy: None,
        harvest_jobs: BTreeMap::new(),
    }
}

/// One row of the per-member ETag table: the member's name, and the mutation that
/// changes it on an origin-endpoint record. A `fn` pointer rather than a closure, so
/// a row carries no captured state.
type EndpointMutation = (&'static str, fn(&mut OriginEndpoint));

/// One row per member an `Update*` or a `Create*` writes on an origin endpoint.
#[test]
fn a_change_to_any_member_of_an_origin_endpoint_moves_its_entity_tag() {
    let baseline = sample_endpoint();
    let mutations: [EndpointMutation; 12] = [
        ("container_type", |e| {
            e.container_type = "CMAF".to_string()
        }),
        ("description", |e| e.description = Some("a".to_string())),
        ("startover_window_seconds", |e| {
            e.startover_window_seconds = Some(1)
        }),
        ("segment", |e| {
            e.segment = Segment {
                segment_duration_seconds: Some(6),
                ..Segment::default()
            }
        }),
        ("hls_manifests", |e| {
            e.hls_manifests = vec![stored_hls_manifest(
                CreateHlsManifestConfiguration {
                    manifest_name: "m".to_string(),
                    ..CreateHlsManifestConfiguration::default()
                },
                ManifestLocation {
                    egress_domain: "host.example",
                    region: "us-west-2",
                    group: "group",
                    channel: "channel",
                    endpoint: "endpoint",
                },
            )]
        }),
        ("low_latency_hls_manifests", |e| {
            e.low_latency_hls_manifests = vec![GetLowLatencyHlsManifestConfiguration {
                manifest_name: "m".to_string(),
                ..GetLowLatencyHlsManifestConfiguration::default()
            }]
        }),
        ("dash_manifests", |e| {
            e.dash_manifests = vec![GetDashManifestConfiguration {
                manifest_name: "m".to_string(),
                ..GetDashManifestConfiguration::default()
            }]
        }),
        ("mss_manifests", |e| {
            e.mss_manifests = vec![GetMssManifestConfiguration {
                manifest_name: "m".to_string(),
                ..GetMssManifestConfiguration::default()
            }]
        }),
        ("force_endpoint_error_configuration", |e| {
            e.force_endpoint_error_configuration = Some(ForceEndpointErrorConfiguration {
                endpoint_error_conditions: Some(vec!["STALE_MANIFEST".to_string()]),
            })
        }),
        ("uri_separator", |e| {
            e.uri_separator = "HYPHEN".to_string()
        }),
        ("stream_name_output_mode", |e| {
            e.stream_name_output_mode = Some("PASSTHROUGH_NAME".to_string())
        }),
        // An endpoint under a channel that may not carry the member has no mode at
        // all, which is a different record from one that carries the default.
        ("stream_name_output_mode absent", |e| {
            e.stream_name_output_mode = None
        }),
    ];
    for (member, mutate) in mutations {
        let mut mutated = baseline.clone();
        mutate(&mut mutated);
        assert_ne!(
            origin_endpoint_etag(&baseline),
            origin_endpoint_etag(&mutated),
            "case {member:?}: changing an endpoint's {member} must move its entity tag"
        );
    }
    // The tag is a function of the CONTENT: the same content, built again, hashes
    // the same, so a create and a later update of an unchanged record agree on it.
    let twin = OriginEndpoint {
        created_at: baseline.created_at,
        modified_at: baseline.modified_at,
        ..sample_endpoint()
    };
    assert_eq!(
        origin_endpoint_etag(&baseline),
        origin_endpoint_etag(&twin),
        "a record rebuilt with the same content must keep its entity tag"
    );
}

#[test]
fn a_token_match_tells_a_retry_from_a_changed_request() {
    let stored = [("group-a", Some("token-1"), "fingerprint-a")];
    assert_eq!(
        match_client_token(Some("token-1"), "fingerprint-a", stored.into_iter()),
        TokenMatch::Replay("group-a".to_string()),
        "the same token with the same request must replay the stored resource"
    );
    assert_eq!(
        match_client_token(Some("token-1"), "fingerprint-b", stored.into_iter()),
        TokenMatch::Mismatch,
        "the same token with a changed request must be a parameter mismatch"
    );
    assert_eq!(
        match_client_token(Some("token-2"), "fingerprint-a", stored.into_iter()),
        TokenMatch::Fresh,
        "another token must not match a stored resource"
    );
    assert_eq!(
        match_client_token(None, "fingerprint-a", stored.into_iter()),
        TokenMatch::Fresh,
        "a request with no token can never be a replay"
    );
}

#[test]
fn a_container_serves_only_its_own_manifest_families() {
    // Read off the model's `ValidationExceptionType`: TS serves the HLS families,
    // CMAF adds DASH, ISM serves MSS alone.
    for (container, family, refused) in [
        ("TS", ManifestFamily::Hls, false),
        ("TS", ManifestFamily::LowLatencyHls, false),
        ("TS", ManifestFamily::Dash, true),
        ("TS", ManifestFamily::Mss, true),
        ("CMAF", ManifestFamily::Hls, false),
        ("CMAF", ManifestFamily::LowLatencyHls, false),
        ("CMAF", ManifestFamily::Dash, false),
        ("CMAF", ManifestFamily::Mss, true),
        ("ISM", ManifestFamily::Hls, true),
        ("ISM", ManifestFamily::LowLatencyHls, true),
        ("ISM", ManifestFamily::Dash, true),
        ("ISM", ManifestFamily::Mss, false),
    ] {
        let conflict = container_manifest_conflict(container, family);
        assert_eq!(
            conflict.is_some(),
            refused,
            "case {container} + {family:?}: expected refused={refused}, got {conflict:?}"
        );
    }
}
