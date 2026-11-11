// SPDX-License-Identifier: CC0-1.0

//! The tag plane — `TagResource`, `ListTagsForResource` and `UntagResource` — end to
//! end over the wire.
//!
//! RULE: a tag operation resolves its `ResourceArn` to the resource the ARN's
//! `resourcetype/name` resource part names and reads or writes THAT resource's own tag
//! map, so a tag write merges the keys it carries (a key already held takes the new
//! value, every other tag is left alone), an untag removes exactly the keys it names and
//! is idempotent, a tag read reports the stored map, the resource's own describe reports
//! the same map, an ARN of this service's vendor naming a resource the store does not
//! hold is refused as not found, and a value that is not a resource ARN of this
//! service's vendor is a bad request refused on the input before any lookup.
//!
//! Every call goes through the shared harness (`crate::harness`), so each one is a
//! separate HTTP request against one shared engine: a tag that survives from the write
//! call to the read call survived because it was PERSISTED.
//!
//! The tables span what the rule generalises over: the resource kind the ARN names
//! (every taggable kind the service holds), how many keys one write carries, whether a
//! key is new or already held, the character classes the modelled `TagKey`/`TagValue`
//! admit, the keys an untag names (held, absent, repeated, all of them), the resource's
//! tag count against the documented ceiling, and the shapes an ARN that names no
//! resource of this service takes (another vendor, another partition, a vendor spelling
//! next to this one, a modelled type the store holds none of, no name, no separator).

use std::collections::BTreeMap;

use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_location_locationservice::state;

// The one HTTP harness this crate's suites share, declared from `tests/all.rs`.
use crate::harness::{call, emulator, Answer, ACCOUNT, REGION};

/// The style a test map is created with, from the `MapConfiguration$Style` catalog.
const MAP_STYLE: &str = "VectorEsriStreets";

/// The data provider a test route calculator is created with.
const CALCULATOR_DATA_SOURCE: &str = "Esri";

/// The sentence a tag key carrying the reserved prefix is refused with.
const RESERVED_PREFIX_REFUSAL: &str =
    "'Tags' must not use the reserved prefix 'aws:' in a tag key.";

/// The sentence a tag write that would take a resource past the documented ceiling is
/// refused with. The ceiling is the `TagResource` doc's "You can associate up to 50 tags
/// with a resource."
const TOO_MANY_TAGS_REFUSAL: &str = "'Tags' must not take the resource past 50 tags.";

/// One taggable resource kind: how a test creates one, where its ARN comes back, where
/// its own read lives, and the ARN `resourcetype` segment its ARN carries.
struct TaggableKind {
    /// The ARN `resourcetype` segment, as the resource's `*Arn` member doc spells it
    /// (`arn:aws:geo:region:account-id:tracker/ExampleTracker`).
    arn_resource_type: &'static str,
    create_path: &'static str,
    create_body: fn(&str) -> Value,
    /// The create response member carrying the new resource's ARN.
    arn_member: &'static str,
    /// This resource's own read, which must report the same tag map the tag plane wrote.
    describe_path: fn(&str) -> String,
}

fn tracker_body(name: &str) -> Value {
    json!({ "TrackerName": name })
}

fn collection_body(name: &str) -> Value {
    json!({ "CollectionName": name })
}

fn map_body(name: &str) -> Value {
    json!({ "MapName": name, "Configuration": { "Style": MAP_STYLE } })
}

fn calculator_body(name: &str) -> Value {
    json!({ "CalculatorName": name, "DataSource": CALCULATOR_DATA_SOURCE })
}

fn tracker_path(name: &str) -> String {
    format!("/tracking/v0/trackers/{name}")
}

fn collection_path(name: &str) -> String {
    format!("/geofencing/v0/collections/{name}")
}

fn map_path(name: &str) -> String {
    format!("/maps/v0/maps/{name}")
}

fn calculator_path(name: &str) -> String {
    format!("/routes/v0/calculators/{name}")
}

/// Every resource kind this service stores tags on, with the operations that create and
/// read one. The `arn_resource_type` column is written out here rather than read from the
/// crate, so a kind the crate stops resolving fails these tests instead of shrinking
/// with them.
const TAGGABLE_KINDS: &[TaggableKind] = &[
    TaggableKind {
        arn_resource_type: "tracker",
        create_path: "/tracking/v0/trackers",
        create_body: tracker_body,
        arn_member: "TrackerArn",
        describe_path: tracker_path,
    },
    TaggableKind {
        arn_resource_type: "geofence-collection",
        create_path: "/geofencing/v0/collections",
        create_body: collection_body,
        arn_member: "CollectionArn",
        describe_path: collection_path,
    },
    TaggableKind {
        arn_resource_type: "map",
        create_path: "/maps/v0/maps",
        create_body: map_body,
        arn_member: "MapArn",
        describe_path: map_path,
    },
    TaggableKind {
        arn_resource_type: "route-calculator",
        create_path: "/routes/v0/calculators",
        create_body: calculator_body,
        arn_member: "CalculatorArn",
        describe_path: calculator_path,
    },
];

/// The RFC 3986 unreserved set: every other byte is percent-encoded.
const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Percent-encode a value the way an SDK encodes one it puts in a single-segment
/// `@httpLabel` or an `@httpQuery` value, so an ARN's `:` and `/` cannot be mistaken
/// for path or query structure.
fn encode(value: &str) -> String {
    percent_encode(value.as_bytes(), UNRESERVED).to_string()
}

/// `TagResource`: `POST /tags/{ResourceArn}` with the map in the body.
async fn tag(router: &MultiServiceRouter, arn: &str, tags: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", encode(arn)),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// `ListTagsForResource`: `GET /tags/{ResourceArn}`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", encode(arn)), None).await
}

/// `UntagResource`: `DELETE /tags/{ResourceArn}?tagKeys=…`, one `tagKeys` pair per key,
/// exactly as the model binds them.
async fn untag(router: &MultiServiceRouter, arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", encode(arn)),
        None,
    )
    .await
}

/// Create one resource of `kind` and answer the ARN its create reported.
async fn create_resource(router: &MultiServiceRouter, kind: &TaggableKind, name: &str) -> String {
    let created = call(
        router,
        "POST",
        kind.create_path,
        Some((kind.create_body)(name)),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: creating the resource must succeed, got {} {:?}",
        kind.arn_resource_type, created.status, created.body
    );
    let arn = created
        .json()
        .get(kind.arn_member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the create must report {}, got {:?}",
                kind.arn_resource_type, kind.arn_member, created.body
            )
        })
        .to_string();
    let expected = format!(
        "arn:aws:geo:{REGION}:{ACCOUNT}:{}/{name}",
        kind.arn_resource_type
    );
    assert_eq!(
        arn, expected,
        "case {:?}: expected the create to report {expected}, got {arn:?}",
        kind.arn_resource_type
    );
    arn
}

/// The `Tags` member of an answer, as a comparable map. The member is projected
/// unconditionally by every tag-carrying read in this crate, so its absence is a failure
/// rather than "no tags".
fn reported_tags(case: &str, answer: &Answer) -> BTreeMap<String, String> {
    let body = answer.json();
    let tags = body.get("Tags").unwrap_or_else(|| {
        panic!("case {case:?}: the answer must carry a Tags member, got {body}")
    });
    let map = tags
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: Tags must be a map, got {tags}"));
    map.iter()
        .map(|(key, value)| {
            let text = value.as_str().unwrap_or_else(|| {
                panic!("case {case:?}: tag {key:?} must carry a string, got {value}")
            });
            (key.clone(), text.to_string())
        })
        .collect()
}

fn expected_tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn tag_body(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// Assert one refused request: the status and code the model declares for a bad member,
/// the sentence, and the two members the model marks required beside `message`.
fn assert_field_refusal(case: &str, refused: &Answer, member: &str, sentence: &str) {
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected a 400 refusal, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case {case:?}: expected error code ValidationException, got {:?} ({:?})",
        refused.error_code(),
        refused.body
    );
    assert_eq!(
        refused.message(),
        sentence,
        "case {case:?}: expected message {sentence:?}, got {:?}",
        refused.message()
    );
    let body = refused.json();
    assert_eq!(
        body.get("reason").and_then(Value::as_str),
        Some("FieldValidationFailed"),
        "case {case:?}: expected reason FieldValidationFailed, got {body}"
    );
    assert_eq!(
        body.get("fieldList")
            .and_then(Value::as_array)
            .map(|entries| entries
                .iter()
                .map(|entry| (
                    entry.get("name").and_then(Value::as_str).unwrap_or_default(),
                    entry
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ))
                .collect::<Vec<_>>()),
        Some(vec![(member, sentence)]),
        "case {case:?}: fieldList must name {member} and repeat the sentence, got {body}"
    );
}

/// Assert one refused tag write, whose offending member is always `Tags`.
fn assert_tags_refusal(case: &str, refused: &Answer, sentence: &str) {
    assert_field_refusal(case, refused, "Tags", sentence);
}

/// Assert one 404: the status, the modelled code, and the sentence.
fn assert_not_found(case: &str, refused: &Answer, sentence: &str) {
    assert_eq!(
        refused.status, 404,
        "case {case:?}: expected a 404, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected error code ResourceNotFoundException, got {:?} ({:?})",
        refused.error_code(),
        refused.body
    );
    assert_eq!(
        refused.message(),
        sentence,
        "case {case:?}: expected message {sentence:?}, got {:?}",
        refused.message()
    );
}

// ---------------------------------------------------------------------------
// The tag plane over every taggable resource kind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_table_covers_every_resource_kind_an_arn_resolves_to() {
    // The crate's resolvable set must be exactly the set these tests drive: a kind added
    // to the crate without a row here, or a row whose `resourcetype` the crate stopped
    // resolving, shows up here rather than in a client's failed tag call.
    let mut resolvable: Vec<&str> = state::TaggedKind::ALL
        .iter()
        .map(|kind| kind.arn_resource_type())
        .collect();
    resolvable.sort_unstable();
    let mut covered: Vec<&str> = TAGGABLE_KINDS
        .iter()
        .map(|kind| kind.arn_resource_type)
        .collect();
    covered.sort_unstable();
    assert_eq!(
        covered, resolvable,
        "the table must cover every resource kind an ARN resolves to, got {covered:?}"
    );
}

#[tokio::test]
async fn tag_list_and_untag_round_trip_on_every_taggable_resource_kind() {
    for kind in TAGGABLE_KINDS {
        let case = kind.arn_resource_type;
        let router = emulator();
        let arn = create_resource(&router, kind, "tagged").await;

        // A resource nobody has tagged reports an EMPTY map, never an absent member.
        let listed = list_tags(&router, &arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListTagsForResource must succeed, got {} {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            reported_tags(case, &listed),
            expected_tags(&[]),
            "case {case:?}: an untagged resource must report no tags, got {:?}",
            listed.body
        );

        // The first write: two new keys.
        let tagged = tag(&router, &arn, tag_body(&[("env", "dev"), ("team", "fleet")])).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: TagResource must succeed, got {} {:?}",
            tagged.status, tagged.body
        );
        assert_eq!(
            reported_tags(case, &list_tags(&router, &arn).await),
            expected_tags(&[("env", "dev"), ("team", "fleet")]),
            "case {case:?}: the written tags must read back"
        );

        // The second write MERGES: `env` takes the new value, `cost.center` is appended,
        // and `team` — which this request does not name — survives untouched.
        let merged = tag(
            &router,
            &arn,
            tag_body(&[("env", "prod"), ("cost.center", "42")]),
        )
        .await;
        assert_eq!(
            merged.status, 200,
            "case {case:?}: a second TagResource must succeed, got {} {:?}",
            merged.status, merged.body
        );
        let after_merge = expected_tags(&[("cost.center", "42"), ("env", "prod"), ("team", "fleet")]);
        assert_eq!(
            reported_tags(case, &list_tags(&router, &arn).await),
            after_merge,
            "case {case:?}: a repeated key must take the new value and the others must survive"
        );

        // The tag plane writes the resource's OWN map: its describe reports the same one.
        let described = call(&router, "GET", &(kind.describe_path)("tagged"), None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the resource's own read must succeed, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            reported_tags(case, &described),
            after_merge,
            "case {case:?}: the resource's own read must report the tags the tag plane wrote, got {:?}",
            described.body
        );

        // One held key and one absent key in ONE request: the held one goes, the absent
        // one is a no-op, and the request still succeeds.
        let untagged = untag(&router, &arn, &["team", "never-set"]).await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: UntagResource must succeed, got {} {:?}",
            untagged.status, untagged.body
        );
        let after_untag = expected_tags(&[("cost.center", "42"), ("env", "prod")]);
        assert_eq!(
            reported_tags(case, &list_tags(&router, &arn).await),
            after_untag,
            "case {case:?}: untagging must remove exactly the keys it names"
        );

        // `@idempotent`: the same removal again answers the same way and changes nothing.
        let again = untag(&router, &arn, &["team", "never-set"]).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: repeating UntagResource must succeed, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            reported_tags(case, &list_tags(&router, &arn).await),
            after_untag,
            "case {case:?}: repeating UntagResource must leave the tags as they were"
        );
    }
}

// ---------------------------------------------------------------------------
// What one write may carry
// ---------------------------------------------------------------------------

/// One row of the merge table: the tags a resource already carries, the map one write
/// carries, and what the resource must report afterwards.
struct MergeCase {
    label: &'static str,
    held: &'static [(&'static str, &'static str)],
    write: &'static [(&'static str, &'static str)],
    after: &'static [(&'static str, &'static str)],
}

/// The merge table. Rows differ in how many keys the write carries (1, 2, 3), in whether
/// each key is new or already held, and in the character classes the modelled `TagKey`
/// and `TagValue` patterns admit (`^([\p{L}\p{Z}\p{N}_.,:/=+\-@]*)$`): the punctuation
/// the member doc lists (`+ - = . _ : / @`), a space, a comma, a digit-only key, a
/// non-ASCII letter, and the empty value `TagValue`'s `len 0..256` allows.
const MERGE_CASES: &[MergeCase] = &[
    MergeCase {
        label: "one new key onto none",
        held: &[],
        write: &[("env", "dev")],
        after: &[("env", "dev")],
    },
    MergeCase {
        label: "a held key replaced",
        held: &[("env", "dev"), ("keep", "me")],
        write: &[("env", "prod")],
        after: &[("env", "prod"), ("keep", "me")],
    },
    MergeCase {
        label: "a held key replaced beside a new one",
        held: &[("env", "dev")],
        write: &[("env", "staging"), ("owner", "fleet")],
        after: &[("env", "staging"), ("owner", "fleet")],
    },
    MergeCase {
        label: "the punctuation the member doc lists",
        held: &[],
        write: &[
            ("cost+center", "a=b"),
            ("path/key", "ns:value"),
            ("dotted.key_1", "at@sign-here"),
        ],
        after: &[
            ("cost+center", "a=b"),
            ("dotted.key_1", "at@sign-here"),
            ("path/key", "ns:value"),
        ],
    },
    MergeCase {
        label: "spaces, commas, digits and a non-ASCII letter",
        held: &[("kept", "1")],
        write: &[("team name", "payments, core"), ("42", "año")],
        after: &[("42", "año"), ("kept", "1"), ("team name", "payments, core")],
    },
    MergeCase {
        label: "an empty value",
        held: &[("empty", "not yet")],
        write: &[("empty", "")],
        after: &[("empty", "")],
    },
];

#[tokio::test]
async fn a_tag_write_merges_the_keys_it_carries_and_leaves_the_rest_alone() {
    for case in MERGE_CASES {
        let label = case.label;
        let router = emulator();
        let arn = create_resource(&router, &TAGGABLE_KINDS[0], "merged").await;

        if !case.held.is_empty() {
            let held = tag(&router, &arn, tag_body(case.held)).await;
            assert_eq!(
                held.status, 200,
                "case {label:?}: storing the held tags must succeed, got {} {:?}",
                held.status, held.body
            );
        }

        let written = tag(&router, &arn, tag_body(case.write)).await;
        assert_eq!(
            written.status, 200,
            "case {label:?}: TagResource must succeed, got {} {:?}",
            written.status, written.body
        );
        assert_eq!(
            reported_tags(label, &list_tags(&router, &arn).await),
            expected_tags(case.after),
            "case {label:?}: expected the merge to leave {:?}",
            case.after
        );
    }
}

/// One row of the untag table: the tags held, the keys one request names, what survives.
struct UntagCase {
    label: &'static str,
    held: &'static [(&'static str, &'static str)],
    remove: &'static [&'static str],
    after: &'static [(&'static str, &'static str)],
}

/// The untag table. Rows differ in how many keys the request names (1, 2, 3, all), in
/// whether each is held or absent, in their order and multiplicity, in key case (keys
/// are compared exactly), and in the punctuation a key can carry.
const UNTAG_CASES: &[UntagCase] = &[
    UntagCase {
        label: "the only key",
        held: &[("only", "one")],
        remove: &["only"],
        after: &[],
    },
    UntagCase {
        label: "an absent key alone",
        held: &[("env", "dev")],
        remove: &["nope"],
        after: &[("env", "dev")],
    },
    UntagCase {
        label: "no tags held at all",
        held: &[],
        remove: &["env"],
        after: &[],
    },
    UntagCase {
        label: "one of three",
        held: &[("a", "1"), ("b", "2"), ("c", "3")],
        remove: &["b"],
        after: &[("a", "1"), ("c", "3")],
    },
    UntagCase {
        label: "the same key twice in one request",
        held: &[("dup", "x"), ("keep", "y")],
        remove: &["dup", "dup"],
        after: &[("keep", "y")],
    },
    UntagCase {
        label: "every key, named in another order",
        held: &[("one", "1"), ("two", "2"), ("three", "3")],
        remove: &["two", "three", "one"],
        after: &[],
    },
    UntagCase {
        label: "keys are compared exactly",
        held: &[("env", "lower"), ("Env", "mixed"), ("ENV", "upper")],
        remove: &["env"],
        after: &[("ENV", "upper"), ("Env", "mixed")],
    },
    UntagCase {
        label: "a key carrying the documented punctuation",
        held: &[("path/key", "v"), ("cost+center", "w"), ("ns:key", "x")],
        remove: &["path/key", "ns:key"],
        after: &[("cost+center", "w")],
    },
];

#[tokio::test]
async fn untagging_removes_exactly_the_named_keys() {
    for case in UNTAG_CASES {
        let label = case.label;
        let router = emulator();
        let arn = create_resource(&router, &TAGGABLE_KINDS[1], "untagged").await;

        if !case.held.is_empty() {
            let held = tag(&router, &arn, tag_body(case.held)).await;
            assert_eq!(
                held.status, 200,
                "case {label:?}: storing the held tags must succeed, got {} {:?}",
                held.status, held.body
            );
        }

        let removed = untag(&router, &arn, case.remove).await;
        assert_eq!(
            removed.status, 200,
            "case {label:?}: UntagResource must succeed, got {} {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            reported_tags(label, &list_tags(&router, &arn).await),
            expected_tags(case.after),
            "case {label:?}: expected {:?} to survive",
            case.after
        );
    }
}

// ---------------------------------------------------------------------------
// The two checks the request boundary cannot make
// ---------------------------------------------------------------------------

/// Tag keys the service reserves, as a request could spell them. The `Tags` member doc:
/// 'Cannot use "aws:" as a prefix for a key.'
const RESERVED_KEYS: &[&str] = &["aws:cloudformation:stack-name", "aws:created-by", "aws:"];

#[tokio::test]
async fn a_reserved_tag_key_is_refused_by_a_tag_write_and_stores_nothing() {
    for key in RESERVED_KEYS {
        let router = emulator();
        let arn = create_resource(&router, &TAGGABLE_KINDS[0], "reserved").await;
        let held = tag(&router, &arn, tag_body(&[("env", "dev")])).await;
        assert_eq!(
            held.status, 200,
            "case {key:?}: the positive control must be stored, got {} {:?}",
            held.status, held.body
        );

        // A reserved key beside an acceptable one refuses the WHOLE write.
        let refused = tag(&router, &arn, tag_body(&[(key, "x"), ("fine", "y")])).await;
        assert_tags_refusal(key, &refused, RESERVED_PREFIX_REFUSAL);
        assert_eq!(
            reported_tags(key, &list_tags(&router, &arn).await),
            expected_tags(&[("env", "dev")]),
            "case {key:?}: a refused write must store nothing"
        );
    }
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_by_every_create_that_takes_tags() {
    // The restriction is stated by the `Tags` member doc of each create as well, so a
    // create is refused the same way and stores no resource.
    for kind in TAGGABLE_KINDS {
        for key in RESERVED_KEYS {
            let case = format!("{} {key}", kind.arn_resource_type);
            let router = emulator();
            let mut body = (kind.create_body)("reserved");
            body["Tags"] = tag_body(&[(key, "x")]);
            let refused = call(&router, "POST", kind.create_path, Some(body)).await;
            assert_tags_refusal(&case, &refused, RESERVED_PREFIX_REFUSAL);

            let described = call(&router, "GET", &(kind.describe_path)("reserved"), None).await;
            assert_eq!(
                described.status, 404,
                "case {case:?}: a refused create must store nothing, got {} {:?}",
                described.status, described.body
            );
        }
    }
}

/// `n` distinct tags, as a request body carries them.
fn generated_tags(prefix: &str, count: usize) -> Value {
    Value::Object(
        (0..count)
            .map(|index| (format!("{prefix}{index}"), json!("v")))
            .collect::<Map<String, Value>>(),
    )
}

#[tokio::test]
async fn a_merge_past_the_documented_tag_ceiling_is_refused_and_writes_nothing() {
    let ceiling = state::MAX_TAGS_PER_RESOURCE;
    // The rows walk the ceiling: the last key that fits, the first that does not, a
    // batch that would cross it, and a write that only REPLACES keys at the ceiling —
    // which never grows the map and so is never refused.
    let rows: &[(&str, usize, usize, bool)] = &[
        ("one short of the ceiling", ceiling - 1, 1, true),
        ("exactly the ceiling", ceiling, 0, true),
        ("one key past the ceiling", ceiling, 1, false),
        ("a batch that crosses the ceiling", ceiling - 1, 2, false),
    ];

    for (label, held, added, accepted) in rows {
        let router = emulator();
        let arn = create_resource(&router, &TAGGABLE_KINDS[3], "bounded").await;
        let stored = tag(&router, &arn, generated_tags("held", *held)).await;
        assert_eq!(
            stored.status, 200,
            "case {label:?}: storing {held} tags must succeed, got {} {:?}",
            stored.status, stored.body
        );

        if *added > 0 {
            let more = tag(&router, &arn, generated_tags("more", *added)).await;
            if *accepted {
                assert_eq!(
                    more.status, 200,
                    "case {label:?}: {added} more tags must be accepted, got {} {:?}",
                    more.status, more.body
                );
            } else {
                assert_tags_refusal(label, &more, TOO_MANY_TAGS_REFUSAL);
            }
        }

        let expected = if *accepted { held + added } else { *held };
        assert_eq!(
            reported_tags(label, &list_tags(&router, &arn).await).len(),
            expected,
            "case {label:?}: expected the resource to hold {expected} tags"
        );
    }

    // At the ceiling, a write that only replaces values is still accepted: the merge
    // adds no key.
    let router = emulator();
    let arn = create_resource(&router, &TAGGABLE_KINDS[3], "replaced").await;
    let stored = tag(&router, &arn, generated_tags("held", ceiling)).await;
    assert_eq!(
        stored.status, 200,
        "case \"replace at the ceiling\": storing {ceiling} tags must succeed, got {} {:?}",
        stored.status, stored.body
    );
    let replaced = tag(&router, &arn, tag_body(&[("held0", "replacement")])).await;
    assert_eq!(
        replaced.status, 200,
        "case \"replace at the ceiling\": replacing a held key must succeed, got {} {:?}",
        replaced.status, replaced.body
    );
    let tags = reported_tags("replace at the ceiling", &list_tags(&router, &arn).await);
    assert_eq!(
        tags.len(),
        ceiling,
        "case \"replace at the ceiling\": the map must still hold {ceiling} tags"
    );
    assert_eq!(
        tags.get("held0").map(String::as_str),
        Some("replacement"),
        "case \"replace at the ceiling\": the replaced value must be the new one, got {tags:?}"
    );
}

// ---------------------------------------------------------------------------
// An ARN that names no resource this service holds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_not_found_on_every_tag_operation() {
    for kind in TAGGABLE_KINDS {
        let router = emulator();
        // One resource of this kind exists, under another name: the lookup is by the
        // exact name the ARN carries.
        create_resource(&router, kind, "exists").await;

        for name in ["missing", "Exists", "exists.2"] {
            let arn = format!(
                "arn:aws:geo:{REGION}:{ACCOUNT}:{}/{name}",
                kind.arn_resource_type
            );
            let case = format!("{} {name}", kind.arn_resource_type);
            let sentence = format!(
                "{} not found: {name}.",
                state::TaggedKind::ALL
                    .iter()
                    .find(|resolvable| resolvable.arn_resource_type() == kind.arn_resource_type)
                    .expect("the table's kind is one the crate resolves")
                    .resource_label()
            );

            assert_not_found(
                &format!("{case} TagResource"),
                &tag(&router, &arn, tag_body(&[("env", "dev")])).await,
                &sentence,
            );
            assert_not_found(
                &format!("{case} ListTagsForResource"),
                &list_tags(&router, &arn).await,
                &sentence,
            );
            assert_not_found(
                &format!("{case} UntagResource"),
                &untag(&router, &arn, &["env"]).await,
                &sentence,
            );
        }
    }
}

#[tokio::test]
async fn an_arn_of_this_vendor_naming_a_resource_the_store_lacks_is_not_found() {
    // Rows: a `resourcetype` this service models but holds no resources of (each of
    // those creates is a separate scaffolded operation), a type no model names at all, a
    // resource part with no name, and one with no `resourcetype` separator. Each is a
    // well-formed ARN of this service's vendor, so the refusal is a not-found rather than
    // a bad request.
    let arns = [
        format!("arn:aws:geo:{REGION}:{ACCOUNT}:place-index/anything"),
        format!("arn:aws:geo:{REGION}:{ACCOUNT}:key/anything"),
        format!("arn:aws:geo:{REGION}:{ACCOUNT}:job/anything"),
        format!("arn:aws:geo:{REGION}:{ACCOUNT}:not-a-location-type/anything"),
        format!("arn:aws:geo:{REGION}:{ACCOUNT}:tracker/"),
        format!("arn:aws:geo:{REGION}:{ACCOUNT}:tracker"),
    ];

    let router = emulator();
    // A tracker exists, so a refusal cannot be "the store is empty".
    create_resource(&router, &TAGGABLE_KINDS[0], "exists").await;

    for arn in &arns {
        let sentence = format!("Resource not found: {arn}.");
        assert_not_found(
            &format!("{arn} TagResource"),
            &tag(&router, arn, tag_body(&[("env", "dev")])).await,
            &sentence,
        );
        assert_not_found(
            &format!("{arn} ListTagsForResource"),
            &list_tags(&router, arn).await,
            &sentence,
        );
        assert_not_found(
            &format!("{arn} UntagResource"),
            &untag(&router, arn, &["env"]).await,
            &sentence,
        );
    }
}

#[tokio::test]
async fn an_arn_of_another_vendor_is_a_bad_request_and_reaches_no_resource() {
    // The vendor (third) field says whose API can address the resource at all, so an ARN
    // of another vendor names nothing in this service's namespace and is refused on the
    // input. The rows deliberately include other vendors carrying THIS service's
    // resource type and the name of a resource that exists, so a parse that ignored the
    // vendor would tag it: another partition, a neighbouring vendor spelling, and a
    // vendor whose ARNs carry no resource type at all.
    let arns = [
        format!("arn:aws:s3:{REGION}:{ACCOUNT}:tracker/exists"),
        format!("arn:aws-cn:s3:cn-north-1:{ACCOUNT}:tracker/exists"),
        format!("arn:aws:geo-maps:{REGION}:{ACCOUNT}:tracker/exists"),
        format!("arn:aws:iam::{ACCOUNT}:role/example"),
        format!("arn:aws:s3:::bucket-{ACCOUNT}"),
    ];
    let sentence = "'ResourceArn' must be the ARN of an Amazon Location resource.";

    let router = emulator();
    let own_arn = create_resource(&router, &TAGGABLE_KINDS[0], "exists").await;
    let held = tag(&router, &own_arn, tag_body(&[("env", "dev")])).await;
    assert_eq!(
        held.status, 200,
        "the positive control must be stored, got {} {:?}",
        held.status, held.body
    );

    for arn in &arns {
        assert_field_refusal(
            &format!("{arn} TagResource"),
            &tag(&router, arn, tag_body(&[("env", "tagged-by-mistake")])).await,
            "ResourceArn",
            sentence,
        );
        assert_field_refusal(
            &format!("{arn} ListTagsForResource"),
            &list_tags(&router, arn).await,
            "ResourceArn",
            sentence,
        );
        assert_field_refusal(
            &format!("{arn} UntagResource"),
            &untag(&router, arn, &["env"]).await,
            "ResourceArn",
            sentence,
        );

        // None of those calls reached this service's own resource of that name.
        assert_eq!(
            reported_tags(arn, &list_tags(&router, &own_arn).await),
            expected_tags(&[("env", "dev")]),
            "case {arn:?}: an ARN of another vendor must reach no resource of this service"
        );
    }
}

#[tokio::test]
async fn reading_tags_does_not_change_what_a_later_reader_sees() {
    let router = emulator();
    let arn = create_resource(&router, &TAGGABLE_KINDS[2], "stable").await;
    tag(&router, &arn, tag_body(&[("env", "dev"), ("owner", "maps")])).await;

    let first = list_tags(&router, &arn).await.json();
    for round in 1..=3 {
        let again = list_tags(&router, &arn).await;
        assert_eq!(
            again.status, 200,
            "round {round}: the read must keep succeeding, got {} {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.json(),
            first,
            "round {round}: ListTagsForResource must report the same tags every time, got {:?}",
            again.body
        );
    }
    // The resource's own read is unaffected too: a tag read writes nothing.
    let described = call(&router, "GET", &(TAGGABLE_KINDS[2].describe_path)("stable"), None).await;
    assert_eq!(
        reported_tags("stable", &described),
        expected_tags(&[("env", "dev"), ("owner", "maps")]),
        "the resource's own read must still report the stored tags, got {:?}",
        described.body
    );
}
