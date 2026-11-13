// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `TagResource`: the rule that it applies EXACTLY the
//! key/value pairs the request carries to the tag set of the resource its
//! `ResourceArn` names — a key the resource does not hold becomes one of its tags, a
//! key it already holds takes the value the request carries, and every key the
//! request does not name stays as it was — so every later read of that resource
//! reports the set the call left.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises
//! over: the resource type the ARN names (every type this service mints an ARN for —
//! a registry, one of a registry's schemas, and a discoverer), the number of tags the
//! resource already carries, how many of the applied keys are new and how many
//! overwrite a held one, the alphabet, length and emptiness of the keys and values,
//! and the number of pairs the request carries (none, one, several, many).
//!
//! The bounds a tag set is kept within are asserted on each published bound and one
//! past it — the number of tags one resource may hold, the key length and the value
//! length — from the service's own constants, with a refused write leaving the stored
//! set exactly as it was.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The
//! HTTP harness is the crate's one harness, imported from
//! `registry_schema_lifecycle`, and the tag helpers (the ARN route, the `@httpLabel`
//! percent-encoding, the create-tagged table and the reported tag map) are the ones
//! `resource_tags.rs` already carries for the read side. Nothing here touches the
//! state struct directly, so a handler that answered without writing the store would
//! still have to make the following read agree.

use std::collections::BTreeMap;

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::{
    aws_managed_schemas, AWS_MANAGED_REGISTRIES, CODE_FORBIDDEN, CODE_MALFORMED_REQUEST,
    CODE_RESOURCE_NOT_FOUND, MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

use crate::registry_schema_lifecycle::{
    create_registry, registry_uri, schema_uri, Emulator, ACCOUNT, REGION,
};
use crate::resource_tags::{
    create_tagged, reported_tags, resource_name, source_arn, tags, tags_now, tags_uri, Created,
    Kind, CONTENT, DISCOVERERS_URI,
};

/// Apply one tag set to one resource through `TagResource`
/// (`POST /tags/{ResourceArn}` with the `Tags` map under its `tags` wire name).
async fn tag(
    emulator: &Emulator,
    arn: &str,
    pairs: &BTreeMap<String, String>,
) -> crate::registry_schema_lifecycle::Answer {
    emulator
        .call("POST", &tags_uri(arn), Some(json!({"tags": pairs})))
        .await
}

/// One row: a resource created carrying `held`, and the pairs one `TagResource`
/// applies to it.
#[derive(Debug)]
struct Case {
    case: &'static str,
    held: &'static [(&'static str, &'static str)],
    apply: &'static [(&'static str, &'static str)],
}

/// The table. Rows differ along the number of tags the resource already carries, how
/// many applied keys are new versus already held, the alphabet, length and emptiness
/// of the keys and values, and how many pairs the request carries. `Tags` is a map of
/// plain `__string` to plain `__string` with no length, pattern or count constraint on
/// either (`smithy_query(query="shape",shape="Tags")`), so no row sits outside a
/// modelled bound.
const CASES: &[Case] = &[
    Case {
        case: "one-new-key-on-an-untagged-resource",
        held: &[],
        apply: &[("team", "eventing")],
    },
    Case {
        case: "one-new-key-beside-keys-a-create-stored",
        held: &[("env", "test"), ("owner", "platform")],
        apply: &[("team", "eventing")],
    },
    Case {
        case: "a-key-the-resource-already-holds-takes-the-new-value",
        held: &[("team", "payments"), ("env", "test")],
        apply: &[("team", "eventing")],
    },
    Case {
        case: "new-keys-and-overwritten-keys-together",
        held: &[("team", "payments"), ("env", "test"), ("keep", "v")],
        apply: &[("team", "eventing"), ("env", "prod"), ("zone", "c")],
    },
    Case {
        case: "every-key-the-resource-holds-overwritten-at-once",
        held: &[("a", "1"), ("b", "2"), ("c", "3")],
        apply: &[("a", "9"), ("b", "8"), ("c", "7")],
    },
    Case {
        case: "a-pair-whose-value-is-already-what-it-would-be-set-to",
        held: &[("team", "eventing")],
        apply: &[("team", "eventing")],
    },
    Case {
        case: "no-pair-at-all",
        held: &[("team", "payments")],
        apply: &[],
    },
    Case {
        case: "keys-differing-only-in-case",
        held: &[("team", "lower")],
        apply: &[("Team", "title"), ("TEAM", "upper"), ("team", "replaced")],
    },
    Case {
        case: "keys-and-values-outside-ascii",
        held: &[("keep", "v")],
        apply: &[
            ("\u{e9}quipe", "paiements"),
            ("\u{30c1}\u{30fc}\u{30e0}", "\u{6c5f}\u{6238}"),
        ],
    },
    Case {
        case: "a-value-with-no-characters-in-it",
        held: &[("note", "something")],
        apply: &[("note", ""), ("blank", "")],
    },
    Case {
        case: "keys-and-values-carrying-separators-a-path-would-need-encoded",
        held: &[],
        apply: &[
            ("path/like", "a/b"),
            ("with space", "two words"),
            ("plus+key", "a+b"),
            ("colon:key", "arn:like:value"),
        ],
    },
    Case {
        case: "a-key-and-a-value-longer-than-one-path-segment",
        held: &[("short", "v")],
        apply: &[(
            "a-rather-long-tag-key-that-no-single-path-segment-would-hold-comfortably",
            "a-rather-long-tag-value-that-no-single-path-segment-would-hold-comfortably",
        )],
    },
    Case {
        case: "many-pairs-at-once",
        held: &[("key-00", "stale"), ("held", "v")],
        apply: &[
            ("key-00", "value-00"),
            ("key-01", "value-01"),
            ("key-02", "value-02"),
            ("key-03", "value-03"),
            ("key-04", "value-04"),
            ("key-05", "value-05"),
            ("key-06", "value-06"),
            ("key-07", "value-07"),
            ("key-08", "value-08"),
            ("key-09", "value-09"),
            ("key-10", "value-10"),
            ("key-11", "value-11"),
        ],
    },
];

/// The tag set a row's resource is left with, computed from the row itself rather
/// than from anything the handler did: the held pairs, with every applied key either
/// added or taking the applied value.
fn expected_tags(case: &Case) -> BTreeMap<String, String> {
    let mut expected = tags(case.held);
    for (key, value) in case.apply {
        expected.insert((*key).to_string(), (*value).to_string());
    }
    expected
}

/// RULE: `TagResource` applies exactly the pairs the request carries to the resource
/// its ARN names — adding a key it does not hold, replacing the value of one it does,
/// keeping every key the request does not name — and answers 204 with no body, framed
/// with the `content-length: 0` an answer with no body states, for every resource type
/// this service mints an ARN for. Sent TWICE: applying a tag is not a toggle, so the
/// repeat answers the same way and leaves the same tag set.
///
/// The framing is the rule `empty_response_framing.rs` states for every operation of
/// this service whose output is `Unit`; this suite carries `TagResource`'s row, because
/// the resource it writes to has to be created first.
#[tokio::test]
async fn tag_resource_applies_exactly_the_named_pairs_to_every_taggable_resource() {
    let emulator = Emulator::new();

    for kind in Kind::ALL {
        for (row, case) in CASES.iter().enumerate() {
            let label = format!("{kind:?}/{}", case.case);
            let created = create_tagged(
                &emulator,
                &label,
                kind,
                &resource_name(kind, row),
                &tags(case.held),
            )
            .await;
            assert_eq!(
                tags_now(&emulator, &label, &created).await,
                tags(case.held),
                "case {label:?}: the create must store the row's tags"
            );

            for attempt in 1..=2 {
                let applied = tag(&emulator, &created.arn, &tags(case.apply)).await;
                assert_eq!(
                    applied.status, 204,
                    "case {label:?} attempt {attempt}: expected 204 from TagResource, \
                     got {} {:?}",
                    applied.status, applied.body
                );
                assert!(
                    applied.raw.is_empty(),
                    "case {label:?} attempt {attempt}: a 204 carries no body, got {:?}",
                    applied.raw
                );
                assert_eq!(
                    applied.header("content-length"),
                    Some("0"),
                    "case {label:?} attempt {attempt}: an answer with no body must state \
                     its length, got {:?}",
                    applied.header("content-length")
                );

                // The read-after-write: this is what the stub answered nothing about.
                assert_eq!(
                    tags_now(&emulator, &label, &created).await,
                    expected_tags(case),
                    "case {label:?} attempt {attempt}: the resource must report exactly \
                     the tags the application left"
                );
            }
        }
    }
}

/// RULE: the pairs applied reach the ONE resource the ARN names; every other
/// resource keeps the tags it carries, even where it carries the same keys.
#[tokio::test]
async fn tagging_one_resource_leaves_every_other_resources_tags_alone() {
    let shared = tags(&[("team", "payments"), ("keep", "v")]);
    // Rows: each resource type in turn is the one tagged, while all three are read
    // back — one of them the resource written to, the other two not.
    for (row, tagged) in Kind::ALL.into_iter().enumerate() {
        let emulator = Emulator::new();
        let label = format!("tagging-the-{tagged:?}");
        let mut created: Vec<(Kind, Created)> = Vec::new();
        for kind in Kind::ALL {
            created.push((
                kind,
                create_tagged(
                    &emulator,
                    &label,
                    kind,
                    &resource_name(kind, 300 + row),
                    &shared,
                )
                .await,
            ));
        }

        let arn = &created
            .iter()
            .find(|(kind, _)| *kind == tagged)
            .expect("every resource type is in the table")
            .1
            .arn;
        let applied = tag(
            &emulator,
            arn,
            &tags(&[("team", "eventing"), ("added", "yes")]),
        )
        .await;
        assert_eq!(
            applied.status, 204,
            "case {label:?}: expected 204 from TagResource, got {} {:?}",
            applied.status, applied.body
        );

        for (kind, resource) in &created {
            let mut expected = shared.clone();
            if *kind == tagged {
                expected.insert("team".to_string(), "eventing".to_string());
                expected.insert("added".to_string(), "yes".to_string());
            }
            let read_label = format!("{label}/reading-the-{kind:?}");
            assert_eq!(
                tags_now(&emulator, &read_label, resource).await,
                expected,
                "case {read_label:?}: only the tagged resource gains the pairs"
            );
        }
    }
}

/// A `ResourceArn` a refusal row sends, and the answer it owes.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    arn: String,
    status: u16,
    error_type: &'static str,
    code: &'static str,
}

/// The refusal table: a `ResourceArn` that is no resource ARN of this service is a
/// 400 on its form, and one naming a resource this store does not hold — including
/// the stored resource's ARN with another region, account or partition, and a schema
/// of the service-owned catalogue, which carries no ARN at all — is a 404.
fn refusal_cases(stored_registry_arn: &str, stored_schema_arn: &str) -> Vec<RefusalCase> {
    let bad_request = |case: &'static str, arn: String| RefusalCase {
        case,
        arn,
        status: 400,
        error_type: "BadRequestException",
        code: CODE_MALFORMED_REQUEST,
    };
    let not_found = |case: &'static str, arn: String| RefusalCase {
        case,
        arn,
        status: 404,
        error_type: "NotFoundException",
        code: CODE_RESOURCE_NOT_FOUND,
    };
    let managed_schema = aws_managed_schemas()
        .first()
        .expect("the service-owned catalogue holds schemas")
        .name
        .clone();
    let managed_registry = AWS_MANAGED_REGISTRIES
        .first()
        .expect("the store seeds a service-owned registry");
    vec![
        bad_request("not-an-arn-at-all", "tagged-registry".to_string()),
        bad_request(
            "an-arn-with-an-empty-resource-field",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:"),
        ),
        bad_request(
            "another-services-resource-arn",
            format!("arn:aws:events:{REGION}:{ACCOUNT}:event-bus/default"),
        ),
        bad_request(
            "a-resource-type-this-service-does-not-have",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:codebinding/tagged-registry"),
        ),
        bad_request(
            "a-registry-arn-naming-no-registry",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/"),
        ),
        bad_request(
            "a-schema-arn-naming-no-schema",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/tagged"),
        ),
        not_found(
            "a-registry-the-store-does-not-hold",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:registry/absent-registry"),
        ),
        not_found(
            "the-stored-registry-in-another-region",
            stored_registry_arn.replace(REGION, "eu-west-1"),
        ),
        not_found(
            "the-stored-registry-in-another-account",
            stored_registry_arn.replace(ACCOUNT, "999999999999"),
        ),
        not_found(
            "the-stored-registry-in-another-partition",
            stored_registry_arn.replacen("arn:aws:", "arn:aws-cn:", 1),
        ),
        not_found(
            "a-schema-the-registry-does-not-hold",
            format!("arn:aws:schemas:{REGION}:{ACCOUNT}:schema/tagged/absent.schema"),
        ),
        not_found(
            "the-stored-schema-in-another-region",
            stored_schema_arn.replace(REGION, "eu-west-1"),
        ),
        not_found(
            "a-schema-of-the-service-owned-catalogue-which-has-no-arn",
            format!(
                "arn:aws:schemas:{REGION}:{ACCOUNT}:schema/{managed_registry}/{managed_schema}"
            ),
        ),
        not_found(
            "a-discoverer-id-the-service-never-minted",
            format!(
                "arn:aws:schemas:{REGION}:{ACCOUNT}:discoverer/00000000000000000000000000000000"
            ),
        ),
    ]
}

/// RULE: a `ResourceArn` that names no tag set this caller may write is refused — 400
/// for a value that is no resource ARN of this service, 404 for one naming a resource
/// this store does not hold — and a refused call applies nothing.
#[tokio::test]
async fn tagging_an_arn_that_names_no_writable_resource_is_refused_and_applies_nothing() {
    let emulator = Emulator::new();
    let stored = tags(&[("team", "payments"), ("env", "test")]);
    let registry = create_tagged(&emulator, "refusals", Kind::Registry, "tagged", &stored).await;
    let schema =
        create_tagged(&emulator, "refusals", Kind::Schema, "tagged.schema", &stored).await;

    for case in refusal_cases(&registry.arn, &schema.arn) {
        let refused = tag(
            &emulator,
            &case.arn,
            &tags(&[("team", "eventing"), ("added", "yes")]),
        )
        .await;
        assert_eq!(
            refused.status, case.status,
            "case {:?}: expected {} from TagResource, got {} {:?}",
            case.case, case.status, refused.status, refused.body
        );
        assert_eq!(
            refused.error_type(),
            case.error_type,
            "case {:?}: expected the {} shape, got {:?}",
            case.case,
            case.error_type,
            refused.header("x-amzn-errortype")
        );
        assert_eq!(
            refused.str_at("Code"),
            case.code,
            "case {:?}: expected the {} condition code, got {:?}",
            case.case,
            case.code,
            refused.body
        );

        // Nothing was applied: both resources still carry exactly what their create
        // stored.
        for (label, resource) in [("registry", &registry), ("schema", &schema)] {
            let read_label = format!("{}/{label}", case.case);
            assert_eq!(
                tags_now(&emulator, &read_label, resource).await,
                stored,
                "case {read_label:?}: a refused application must leave the stored tags \
                 alone"
            );
        }
    }
}

/// RULE: what is in a service-owned registry is AWS's, so a caller may read it but
/// not write to it: applying a tag to one is refused 403, and the registry is still
/// there to read, carrying no tag the refused call named.
#[tokio::test]
async fn tagging_a_service_owned_registry_is_refused() {
    let emulator = Emulator::new();
    // Source: `crate::state::AWS_MANAGED_REGISTRIES` — the service-owned registries
    // every account sees.
    for registry in AWS_MANAGED_REGISTRIES {
        let described = emulator.call("GET", &registry_uri(registry), None).await;
        assert_eq!(
            described.status, 200,
            "case {registry:?}: expected 200 from DescribeRegistry, got {} {:?}",
            described.status, described.body
        );
        let arn = described.str_at("RegistryArn");

        // Rows: one pair and several — a write to AWS-owned content is refused on who
        // owns it, not on how much it would change.
        for pairs in [&[("team", "eventing")][..], &[("a", "1"), ("b", "2")][..]] {
            let refused = tag(&emulator, &arn, &tags(pairs)).await;
            assert_eq!(
                refused.status, 403,
                "case {registry:?}/{pairs:?}: expected 403 from TagResource, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type(),
                "ForbiddenException",
                "case {registry:?}/{pairs:?}: expected the ForbiddenException shape, got {:?}",
                refused.header("x-amzn-errortype")
            );
            assert_eq!(
                refused.str_at("Code"),
                CODE_FORBIDDEN,
                "case {registry:?}/{pairs:?}: expected the {CODE_FORBIDDEN} condition code, \
                 got {:?}",
                refused.body
            );
        }

        let listed = emulator.call("GET", &tags_uri(&arn), None).await;
        assert_eq!(
            listed.status, 200,
            "case {registry:?}: the service-owned registry is still readable, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_tags(&listed, registry),
            BTreeMap::new(),
            "case {registry:?}: a refused application leaves the seeded registry as it \
             was, got {:?}",
            listed.body.get("tags")
        );
    }
}

/// RULE: a tag set is kept within the bounds a resource's tag set has — at most
/// `MAX_TAGS_PER_RESOURCE` tags, a key of at most `MAX_TAG_KEY_CHARS` characters and a
/// value of at most `MAX_TAG_VALUE_CHARS` — and a write past one of them is refused
/// `BadRequestException` (400) with the stored set left exactly as it was.
///
/// Source: the service's own constants, each asserted AT its bound and one past it.
/// Replacing the value of a key the resource already holds does not grow the set, so a
/// resource at the count bound still takes one.
#[tokio::test]
async fn a_tag_write_is_refused_past_the_bounds_a_tag_set_is_kept_within() {
    let emulator = Emulator::new();
    let held = tags(&[("env", "test")]);

    // Each row: the pairs one request carries, and whether the bounds admit them.
    let at_key_bound = "k".repeat(MAX_TAG_KEY_CHARS);
    let past_key_bound = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let at_value_bound = "v".repeat(MAX_TAG_VALUE_CHARS);
    let past_value_bound = "v".repeat(MAX_TAG_VALUE_CHARS + 1);
    // The resource already holds one tag, so a request naming one key short of the
    // ceiling fills it exactly and one more key is past it.
    let fills_the_count: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE - held.len())
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let mut past_the_count = fills_the_count.clone();
    past_the_count.insert("one-key-too-many".to_string(), "v".to_string());

    let rows: Vec<(&str, BTreeMap<String, String>, bool)> = vec![
        (
            "a key at the key bound",
            BTreeMap::from([(at_key_bound.clone(), "v".to_string())]),
            true,
        ),
        (
            "a key one character past the key bound",
            BTreeMap::from([(past_key_bound, "v".to_string())]),
            false,
        ),
        (
            "a value at the value bound",
            BTreeMap::from([("k".to_string(), at_value_bound)]),
            true,
        ),
        (
            "a value one character past the value bound",
            BTreeMap::from([("k".to_string(), past_value_bound)]),
            false,
        ),
        (
            "pairs that fill the count bound exactly",
            fills_the_count,
            true,
        ),
        (
            "one pair past the count bound",
            past_the_count,
            false,
        ),
    ];

    for (row, (case, applied, admitted)) in rows.into_iter().enumerate() {
        let created = create_tagged(
            &emulator,
            case,
            Kind::Registry,
            &resource_name(Kind::Registry, 400 + row),
            &held,
        )
        .await;
        let answered = tag(&emulator, &created.arn, &applied).await;
        if admitted {
            assert_eq!(
                answered.status, 204,
                "case {case:?}: a write within the bounds is served, got {} {:?}",
                answered.status, answered.body
            );
            let mut expected = held.clone();
            expected.extend(applied.clone());
            assert_eq!(
                tags_now(&emulator, case, &created).await,
                expected,
                "case {case:?}: the resource must report the tags the write left"
            );
            continue;
        }
        assert_eq!(
            answered.status, 400,
            "case {case:?}: a write past a bound is refused 400, got {} {:?}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.error_type(),
            "BadRequestException",
            "case {case:?}: expected the BadRequestException shape, got {:?}",
            answered.header("x-amzn-errortype")
        );
        assert_eq!(
            answered.str_at("Code"),
            CODE_MALFORMED_REQUEST,
            "case {case:?}: expected the malformed-request condition code, got {:?}",
            answered.body
        );
        assert_eq!(
            tags_now(&emulator, case, &created).await,
            held,
            "case {case:?}: a refused write must leave the stored tags alone"
        );
    }
}

/// RULE: the same bounds hold for the tag set a CREATE seeds, so a resource may hold
/// no more through its create than through a later tag write — asserted at the count
/// bound and one past it, for every resource type that carries a tag set.
#[tokio::test]
async fn a_create_is_refused_past_the_bounds_a_tag_set_is_kept_within() {
    let at_bound: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let mut past_bound = at_bound.clone();
    past_bound.insert("one-key-too-many".to_string(), "v".to_string());

    for (row, kind) in Kind::ALL.into_iter().enumerate() {
        let emulator = Emulator::new();
        // At the bound the create is served and the whole set reads back.
        let created = create_tagged(
            &emulator,
            &format!("{kind:?}/at the count bound"),
            kind,
            &resource_name(kind, 500 + row),
            &at_bound,
        )
        .await;
        assert_eq!(
            tags_now(&emulator, &format!("{kind:?}/at the count bound"), &created).await,
            at_bound,
            "case {kind:?}: a create at the count bound stores every tag it names"
        );

        // One past it the create is refused on its form, before anything is stored.
        let name = resource_name(kind, 600 + row);
        let refused = create_past_the_bound(&emulator, kind, &name, &past_bound).await;
        assert_eq!(
            refused.status, 400,
            "case {kind:?}: a create past the count bound is refused 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type(),
            "BadRequestException",
            "case {kind:?}: expected the BadRequestException shape, got {:?}",
            refused.header("x-amzn-errortype")
        );
    }
}

/// Send the create of `kind` with a tag set the bounds refuse, without asserting it
/// succeeded — which is what [`create_tagged`] does.
async fn create_past_the_bound(
    emulator: &Emulator,
    kind: Kind,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> crate::registry_schema_lifecycle::Answer {
    match kind {
        Kind::Registry => {
            emulator
                .call(
                    "POST",
                    &registry_uri(name),
                    Some(json!({"tags": tags})),
                )
                .await
        }
        Kind::Schema => {
            let registry = format!("{name}-registry");
            create_registry(emulator, &registry).await;
            emulator
                .call(
                    "POST",
                    &schema_uri(&registry, name),
                    Some(json!({"Content": CONTENT, "Type": "OpenApi3", "tags": tags})),
                )
                .await
        }
        Kind::Discoverer => {
            emulator
                .call(
                    "POST",
                    DISCOVERERS_URI,
                    Some(json!({"SourceArn": source_arn(name), "tags": tags})),
                )
                .await
        }
    }
}

/// RULE: replacing the value of a key a resource already holds does not grow its tag
/// set, so a resource holding the most tags it may still takes a write that names only
/// keys it holds — the count bound is on what the write would LEAVE, not on the
/// request.
#[tokio::test]
async fn a_resource_at_the_count_bound_still_takes_a_write_that_adds_no_key() {
    let emulator = Emulator::new();
    let at_bound: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let created = create_tagged(
        &emulator,
        "replacement at the count bound",
        Kind::Registry,
        "tags-at-the-count-bound",
        &at_bound,
    )
    .await;

    // The first and the LAST key the resource holds, both derived from the bound, so
    // this row keeps exercising a full tag set whatever the bound becomes.
    let last_held = format!("key-{:03}", MAX_TAGS_PER_RESOURCE - 1);
    let replacement = BTreeMap::from([
        ("key-000".to_string(), "replaced".to_string()),
        (last_held, "also-replaced".to_string()),
    ]);
    let applied = tag(&emulator, &created.arn, &replacement).await;
    assert_eq!(
        applied.status, 204,
        "a write that replaces held values is served at the count bound, got {} {:?}",
        applied.status, applied.body
    );
    let mut expected = at_bound.clone();
    expected.extend(replacement);
    assert_eq!(
        tags_now(&emulator, "replacement at the count bound", &created).await,
        expected,
        "the resource must report the replaced values and no extra key"
    );

    // And one NEW key on top of a full set is still refused.
    let refused = tag(&emulator, &created.arn, &tags(&[("one-more", "v")])).await;
    assert_eq!(
        refused.status, 400,
        "a new key past the count bound is refused 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        tags_now(&emulator, "refused at the count bound", &created).await,
        expected,
        "the refused write must leave the stored tags alone"
    );
}

/// RULE: an application is a write to the resource's tag set and to nothing else —
/// the resource's other members read back exactly as its create stored them.
#[tokio::test]
async fn an_application_changes_no_member_but_the_tags() {
    let emulator = Emulator::new();
    let created = create_tagged(
        &emulator,
        "other-members",
        Kind::Schema,
        "tagged-untouched.schema",
        &tags(&[("env", "test")]),
    )
    .await;
    let before = emulator.call("GET", &created.describe_uri, None).await;

    let applied = tag(&emulator, &created.arn, &tags(&[("team", "eventing")])).await;
    assert_eq!(
        applied.status, 204,
        "expected 204 from TagResource, got {} {:?}",
        applied.status, applied.body
    );

    let after = emulator.call("GET", &created.describe_uri, None).await;
    for member in [
        "SchemaArn",
        "SchemaName",
        "SchemaVersion",
        "Type",
        "Content",
        "LastModified",
        "VersionCreatedDate",
    ] {
        assert_eq!(
            after.body.get(member),
            before.body.get(member),
            "member {member:?}: a tag application must not touch it, got {:?} where it \
             was {:?}",
            after.body.get(member),
            before.body.get(member)
        );
    }
    assert_eq!(
        reported_tags(&after, "other-members"),
        tags(&[("env", "test"), ("team", "eventing")]),
        "the tag set is the one member the application changes, got {:?}",
        after.body.get("tags")
    );
}

/// RULE: the tagging operations of this service are read-after-write with respect to
/// one another over one tag set — a key `TagResource` applies is reported by the next
/// `ListTagsForResource` of the same ARN, until `UntagResource` removes it, and the
/// keys already held are carried through both.
#[tokio::test]
async fn an_applied_key_is_read_back_until_it_is_removed() {
    let emulator = Emulator::new();
    let created = create_tagged(
        &emulator,
        "round-trip",
        Kind::Registry,
        "tag-round-trip",
        &tags(&[("env", "test"), ("owner", "platform")]),
    )
    .await;

    let applied = tag(&emulator, &created.arn, &tags(&[("team", "eventing")])).await;
    assert_eq!(
        applied.status, 204,
        "expected 204 from TagResource, got {} {:?}",
        applied.status, applied.body
    );
    assert_eq!(
        tags_now(&emulator, "after-tagging", &created).await,
        tags(&[("env", "test"), ("owner", "platform"), ("team", "eventing")]),
        "an applied key is reported beside the keys already held"
    );

    let removed = emulator
        .call(
            "DELETE",
            &format!("{}?tagKeys=team", tags_uri(&created.arn)),
            None,
        )
        .await;
    assert_eq!(
        removed.status, 204,
        "expected 204 from UntagResource, got {} {:?}",
        removed.status, removed.body
    );
    assert_eq!(
        tags_now(&emulator, "after-removal", &created).await,
        tags(&[("env", "test"), ("owner", "platform")]),
        "the removal takes back exactly the applied key"
    );
}
