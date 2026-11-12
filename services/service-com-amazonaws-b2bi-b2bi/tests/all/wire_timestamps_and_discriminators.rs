// SPDX-License-Identifier: CC0-1.0

//! The two corrections this service's `awsJson1_0` wire makes to a SUCCESS body, driven
//! end to end (`POST /` with `x-amz-target: B2BI.<Operation>`) through the same router the
//! emulator boots, so what is asserted is what a signed AWS SDK request observes.
//!
//! RULE 1 (timestamps): a timestamp member whose model declares
//! `@timestampFormat("date-time")` reaches the wire as an RFC 3339 / ISO 8601 date-time
//! STRING, never as the epoch-seconds number the protocol defaults to. Every timestamp this
//! model declares targets `CreatedDate` or `ModifiedDate`, which both carry that trait
//! (`smithy_query(query="shape",shape="CreatedDate,ModifiedDate")`), so the rule covers
//! every `createdAt` and `modifiedAt` of every operation that reports one — the creates,
//! the reads, the updates and each entry of each list.
//!
//! RULE 2 (`modifiedAt` at creation): `modifiedAt` reports when the record was last
//! written, and creating it IS a write — so a resource nobody has updated reports a
//! `modifiedAt` equal to its `createdAt`, and an update moves `modifiedAt` without moving
//! `createdAt`.
//!
//! RULE 3 (the type discriminator): a nested structure AWS serves out of this service's
//! shared internal model carries a `__type` member naming that shape in
//! `wire::COMMON_MODEL_NAMESPACE`, beside the structure's own members — the shapes of
//! `wire::DISCRIMINATED_SHAPES`, wherever in whatever operation's response body they occur
//! — and nothing else does: not the response's own top-level body (the operation names that
//! shape), not a list summary, and not the other structures a body nests.
//!
//! Each rule is asserted over a TABLE spanning its class: all four resource kinds this
//! service holds, read back through every operation that reports the members, and the
//! catalog the serializer resolves a member through is re-derived from this crate's own
//! Smithy model, so a model that grows a member targeting one of those shapes fails here
//! rather than silently losing its discriminator.
//!
//! The wire harness (`b2bi_router`, `call`, `text`, …) is the one `onboarding_loop` builds.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::service::wire;
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, capability_configuration, given_capability_buckets, given_transformer, text,
};
use crate::tagging::tag_list;

// ---------------------------------------------------------------------------
// Shared readers, used by the other suites in this target too
// ---------------------------------------------------------------------------

/// The instant `member` reports, in epoch seconds, having first asserted that it is on the
/// wire in the modelled `date-time` form: an RFC 3339 UTC string that parses as one and
/// renders back to exactly the text that was sent.
///
/// Returning the seconds is what lets a caller ORDER two instants without re-implementing
/// the parse.
pub(crate) fn date_time_seconds(body: &Value, member: &str) -> f64 {
    let rendered = body[member].as_str().unwrap_or_else(|| {
        panic!(
            "{member} must be an ISO-8601 date-time string, got {:?}",
            body[member]
        )
    });
    let parsed = DateTime::from_str(rendered, Format::DateTime).unwrap_or_else(|error| {
        panic!("{member} {rendered:?} must parse as an RFC 3339 date-time: {error}")
    });
    let canonical = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("{member} {rendered:?} must render back: {error}"));
    assert_eq!(
        canonical, rendered,
        "{member} must be the canonical RFC 3339 rendering of its instant, got {rendered:?}"
    );
    parsed.as_secs_f64()
}

/// `value` as this service's wire reports it for the structure member `member`: what the
/// request sent, plus the `__type` discriminator when that member targets one of the
/// discriminated shapes (`wire::DISCRIMINATED_MEMBERS`).
///
/// This is how a suite comparing a REPORTED structure against the one a request carried
/// states the expectation at full strength — the discriminator included, rather than read
/// out of the comparison. Idempotent, so a value taken off an earlier RESPONSE may be
/// passed through it too.
pub(crate) fn as_wire_reports(member: &str, value: &Value) -> Value {
    let Some(shape) = wire::DISCRIMINATED_MEMBERS
        .iter()
        .find(|(name, _)| *name == member)
        .map(|(_, shape)| *shape)
    else {
        return value.clone();
    };
    let Value::Object(members) = value else {
        // A member the request omitted stays absent: there is no structure to discriminate.
        return value.clone();
    };
    let mut reported = members.clone();
    reported.insert("__type".to_string(), json!(wire::discriminator(shape)));
    Value::Object(reported)
}

/// `value` with every `__type` discriminator removed, at any depth.
///
/// The discriminator is a property of the wire, not of the resource: a test comparing a
/// reported structure against the one a REQUEST carried reads it through this, while the
/// discriminator itself is asserted by this suite.
pub(crate) fn without_type_discriminators(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(name, _)| name.as_str() != "__type")
                .map(|(name, member)| (name.clone(), without_type_discriminators(member)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_type_discriminators).collect()),
        other => other.clone(),
    }
}

/// Every `__type` value anywhere inside `body`, paired with the JSON path it sits at.
fn discriminators(body: &Value) -> Vec<(String, String)> {
    fn walk(path: &str, value: &Value, found: &mut Vec<(String, String)>) {
        match value {
            Value::Object(members) => {
                for (name, member) in members {
                    if name == "__type" {
                        found.push((
                            path.to_string(),
                            member.as_str().unwrap_or("<not a string>").to_string(),
                        ));
                    } else {
                        walk(&format!("{path}.{name}"), member, found);
                    }
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    walk(&format!("{path}[{index}]"), item, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    walk("$", body, &mut found);
    found
}

/// Invoke `operation`, require a 200, and hand back the body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, answered) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} {answered}"
    );
    answered
}

// ---------------------------------------------------------------------------
// The four resource kinds, and how each is created, read, updated and listed
// ---------------------------------------------------------------------------

/// Which resource a row drives. Every b2bi resource carries the same two timestamps, so
/// each rule below is asserted on all four kinds rather than on the one a recording showed.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Profile,
    Capability,
    Partnership,
    Transformer,
}

/// The four kinds, in the order the rows run.
const KINDS: [Kind; 4] = [
    Kind::Profile,
    Kind::Capability,
    Kind::Partnership,
    Kind::Transformer,
];

impl Kind {
    /// The operations that mint, read, update and enumerate this kind, as the model binds
    /// them (`smithy_query(query="list_operations")`).
    fn operations(self) -> (&'static str, &'static str, &'static str, &'static str) {
        match self {
            Kind::Profile => ("CreateProfile", "GetProfile", "UpdateProfile", "ListProfiles"),
            Kind::Capability => (
                "CreateCapability",
                "GetCapability",
                "UpdateCapability",
                "ListCapabilities",
            ),
            Kind::Partnership => (
                "CreatePartnership",
                "GetPartnership",
                "UpdatePartnership",
                "ListPartnerships",
            ),
            Kind::Transformer => (
                "CreateTransformer",
                "GetTransformer",
                "UpdateTransformer",
                "ListTransformers",
            ),
        }
    }

    /// The member the system-assigned id of this kind is reported and addressed by.
    fn id_member(self) -> &'static str {
        match self {
            Kind::Profile => "profileId",
            Kind::Capability => "capabilityId",
            Kind::Partnership => "partnershipId",
            Kind::Transformer => "transformerId",
        }
    }

    /// The member the list operation reports its summaries under.
    fn list_member(self) -> &'static str {
        match self {
            Kind::Profile => "profiles",
            Kind::Capability => "capabilities",
            Kind::Partnership => "partnerships",
            Kind::Transformer => "transformers",
        }
    }
}

/// The references a partnership and a capability need, created once so every row shares
/// them.
struct References {
    profile_id: String,
    capability_id: String,
    transformer_id: String,
}

/// Create the buckets, the host profile, the transformer and the capability the rows below
/// reference.
async fn references(router: &MultiServiceRouter) -> References {
    given_capability_buckets(router).await;
    let profile = ok(
        router,
        "CreateProfile",
        json!({
            "name": "wire fixture host",
            "phone": "555-555-0100",
            "businessName": "Wire Fixture Corp",
            "logging": "DISABLED",
        }),
    )
    .await;
    let transformer_id = given_transformer(router, "wire-fixture-transformer").await;
    let capability = ok(
        router,
        "CreateCapability",
        json!({
            "name": "wire fixture capability",
            "type": "edi",
            "configuration": capability_configuration(&transformer_id, "X12_850", "VERSION_4010"),
        }),
    )
    .await;
    References {
        profile_id: text(&profile, "profileId"),
        capability_id: text(&capability, "capabilityId"),
        transformer_id,
    }
}

/// A create request for `kind` under `name`, carrying `tags`.
fn create_request(kind: Kind, name: &str, references: &References, tags: &[(&str, &str)]) -> Value {
    let mut request = match kind {
        Kind::Profile => json!({
            "name": name,
            "phone": "555-555-0101",
            "businessName": format!("{name} Corp"),
            "logging": "DISABLED",
        }),
        Kind::Capability => json!({
            "name": name,
            "type": "edi",
            "configuration": capability_configuration(
                &references.transformer_id,
                "X12_810",
                "VERSION_5010",
            ),
        }),
        Kind::Partnership => json!({
            "profileId": references.profile_id,
            "name": name,
            "email": "partner@example.com",
            "capabilities": [references.capability_id],
        }),
        // The one kind whose create carries the discriminated structures.
        Kind::Transformer => json!({
            "name": name,
            "inputConversion": {
                "fromFormat": "X12",
                "formatOptions": { "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" } },
            },
            "mapping": { "templateLanguage": "JSONATA", "template": "{\"a\": a}" },
            "outputConversion": {
                "toFormat": "X12",
                "formatOptions": { "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" } },
            },
        }),
    };
    if !tags.is_empty() {
        request["tags"] = tag_list(tags);
    }
    request
}

/// An update request for `kind` that renames the resource `id` names — the one member every
/// kind takes, so the rule under test is the timestamp and not the member that moved.
fn rename_request(kind: Kind, id: &str, name: &str) -> Value {
    json!({ kind.id_member(): id, "name": name })
}

// ---------------------------------------------------------------------------
// RULE 1 and RULE 2: the timestamps
// ---------------------------------------------------------------------------

/// RULE 1 + RULE 2: `createdAt` and `modifiedAt` are ISO-8601 date-time strings on every
/// operation of every resource kind that reports them, a created resource reports the two
/// as one instant, and an update moves `modifiedAt` only.
///
/// The rows span the four resource kinds, and for each one the whole set of responses that
/// carry a timestamp: the create, the read, the update and the list entry.
#[tokio::test]
async fn every_reported_timestamp_is_an_iso8601_date_time_string() {
    let router = b2bi_router();
    let references = references(&router).await;

    for kind in KINDS {
        let case = format!("{kind:?}");
        let (create, get, update, list) = kind.operations();

        let created = ok(
            &router,
            create,
            create_request(kind, &format!("wire-{}", kind.id_member()), &references, &[]),
        )
        .await;
        let id = text(&created, kind.id_member());
        let created_at = date_time_seconds(&created, "createdAt");

        // The read reports both timestamps, and a resource nobody has updated reports the
        // instant it was created as both.
        let fetched = ok(&router, get, json!({ kind.id_member(): id.clone() })).await;
        assert_eq!(
            fetched["createdAt"], created["createdAt"],
            "case {case:?}: {get} must report the create's own createdAt, got {:?} vs {:?}",
            fetched["createdAt"], created["createdAt"]
        );
        let fetched_modified = date_time_seconds(&fetched, "modifiedAt");
        assert_eq!(
            fetched["modifiedAt"], fetched["createdAt"],
            "case {case:?}: a resource nobody has updated reports modifiedAt as its \
             createdAt, got {:?} vs {:?}",
            fetched["modifiedAt"], fetched["createdAt"]
        );
        assert!(
            (fetched_modified - created_at).abs() < f64::EPSILON,
            "case {case:?}: expected the created instant {created_at}, got {fetched_modified}"
        );

        // A read changes neither: a second read reports the same two instants.
        let again = ok(&router, get, json!({ kind.id_member(): id.clone() })).await;
        for member in ["createdAt", "modifiedAt"] {
            assert_eq!(
                again[member], fetched[member],
                "case {case:?}: a read must not move {member}, got {:?} vs {:?}",
                again[member], fetched[member]
            );
        }

        // The update reports its own write: `createdAt` stands, `modifiedAt` is at or after
        // it, and both are still date-time strings.
        let updated = ok(
            &router,
            update,
            rename_request(kind, &id, &format!("wire-renamed-{}", kind.id_member())),
        )
        .await;
        assert_eq!(
            updated["createdAt"], created["createdAt"],
            "case {case:?}: {update} must not move createdAt, got {:?} vs {:?}",
            updated["createdAt"], created["createdAt"]
        );
        let updated_modified = date_time_seconds(&updated, "modifiedAt");
        assert!(
            updated_modified >= created_at,
            "case {case:?}: modifiedAt {updated_modified} must be at or after createdAt \
             {created_at}"
        );

        // The list projection reports the same two instants, in the same form.
        let listed = ok(&router, list, json!({})).await;
        let entries = listed[kind.list_member()]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: {list} must report a list, got {listed}"));
        let summary = entries
            .iter()
            .find(|entry| text(entry, kind.id_member()) == id)
            .unwrap_or_else(|| panic!("case {case:?}: {list} must report {id}, got {listed}"));
        let _ = date_time_seconds(summary, "createdAt");
        let _ = date_time_seconds(summary, "modifiedAt");
        for member in ["createdAt", "modifiedAt"] {
            assert_eq!(
                summary[member], updated[member],
                "case {case:?}: the {list} entry must report the stored {member}, got {:?} vs \
                 {:?}",
                summary[member], updated[member]
            );
        }
    }
}

/// RULE 1: the form belongs to the MEMBER, not to the instant — so a resource created and
/// updated several times over reports every one of its instants in the same form, and the
/// instants only ever move forward.
///
/// The rows are repeated renames of one profile: 1, 2 and 3 updates deep.
#[tokio::test]
async fn repeated_updates_keep_the_date_time_form_and_move_modified_at_forward() {
    let router = b2bi_router();
    let created = ok(
        &router,
        "CreateProfile",
        json!({
            "name": "repeatedly updated",
            "phone": "555-555-0102",
            "businessName": "Repeat Corp",
            "logging": "DISABLED",
        }),
    )
    .await;
    let profile_id = text(&created, "profileId");
    let created_at = date_time_seconds(&created, "createdAt");

    let mut previous = created_at;
    for round in 1..=3 {
        let case = format!("update {round}");
        let updated = ok(
            &router,
            "UpdateProfile",
            json!({ "profileId": profile_id.clone(), "name": format!("renamed-{round}") }),
        )
        .await;
        let modified = date_time_seconds(&updated, "modifiedAt");
        assert!(
            modified >= previous,
            "case {case:?}: modifiedAt {modified} must be at or after the previous {previous}"
        );
        assert_eq!(
            date_time_seconds(&updated, "createdAt"),
            created_at,
            "case {case:?}: createdAt must stay the create's instant"
        );
        previous = modified;

        let fetched = ok(
            &router,
            "GetProfile",
            json!({ "profileId": profile_id.clone() }),
        )
        .await;
        assert_eq!(
            fetched["modifiedAt"], updated["modifiedAt"],
            "case {case:?}: the read must report the modifiedAt the update wrote, got {:?} vs \
             {:?}",
            fetched["modifiedAt"], updated["modifiedAt"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: the type discriminator
// ---------------------------------------------------------------------------

/// Assert that `body`'s `member` is a structure carrying the discriminator for `shape`
/// beside at least one member of its own.
fn assert_discriminated(case: &str, body: &Value, member: &str, shape: &str) {
    let structure = body
        .get(member)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} structure, got {body}"));
    let expected = wire::discriminator(shape);
    assert_eq!(
        structure.get("__type").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {case:?}: expected {member}.__type {expected:?}, got {structure}"
    );
    let own_members = structure
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected {member} to be an object, got {structure}"))
        .keys()
        .filter(|key| key.as_str() != "__type")
        .count();
    assert!(
        own_members > 0,
        "case {case:?}: the discriminator stands BESIDE the structure's own members, and \
         {member} carries none: {structure}"
    );
}

/// RULE 3: a transformer's `inputConversion`, `outputConversion` and `mapping` carry their
/// shape's discriminator, on every operation that reports them.
///
/// The rows span which of the three the create named (each alone, and all three at once),
/// both mapping template languages the model declares
/// (`smithy_query(query="shape",shape="MappingTemplateLanguage")`: `XSLT`, `JSONATA`), and
/// each of the three responses that reports the members: the create, the read and the
/// update.
#[tokio::test]
async fn a_transformer_s_conversion_structures_carry_the_type_discriminator() {
    let router = b2bi_router();

    let input_conversion = json!({
        "fromFormat": "X12",
        "formatOptions": { "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" } },
    });
    let output_conversion = json!({
        "toFormat": "X12",
        "formatOptions": { "x12": { "transactionSet": "X12_810", "version": "VERSION_5010" } },
    });
    let jsonata = json!({ "templateLanguage": "JSONATA", "template": "{\"a\": a}" });
    let xslt = json!({
        "templateLanguage": "XSLT",
        "template": "<xsl:stylesheet version=\"1.0\"/>",
    });

    // (case, the members the create names)
    let cases: Vec<(&str, Vec<(&str, Value)>)> = vec![
        (
            "inputConversion alone",
            vec![("inputConversion", input_conversion.clone())],
        ),
        (
            "outputConversion alone",
            vec![("outputConversion", output_conversion.clone())],
        ),
        (
            "a JSONATA mapping beside an inbound conversion",
            vec![
                ("inputConversion", input_conversion.clone()),
                ("mapping", jsonata.clone()),
            ],
        ),
        (
            "an XSLT mapping beside an outbound conversion",
            vec![
                ("outputConversion", output_conversion.clone()),
                ("mapping", xslt.clone()),
            ],
        ),
        (
            "all three at once",
            vec![
                ("inputConversion", input_conversion.clone()),
                ("mapping", jsonata.clone()),
                ("outputConversion", output_conversion.clone()),
            ],
        ),
    ];

    // Which member carries which shape, from the model
    // (`smithy_query(query="shape",shape="GetTransformerResponse")`).
    let shape_of = [
        ("inputConversion", "InputConversion"),
        ("mapping", "Mapping"),
        ("outputConversion", "OutputConversion"),
    ];

    for (index, (case, members)) in cases.iter().enumerate() {
        let mut request = json!({ "name": format!("discriminated-{index}") });
        for (member, value) in members {
            request[*member] = value.clone();
        }
        let created = ok(&router, "CreateTransformer", request).await;
        let transformer_id = text(&created, "transformerId");
        let fetched = ok(
            &router,
            "GetTransformer",
            json!({ "transformerId": transformer_id.clone() }),
        )
        .await;
        // The update re-states every member the create named, so the response it answers
        // with carries them too.
        let mut update = json!({ "transformerId": transformer_id.clone() });
        for (member, value) in members {
            update[*member] = value.clone();
        }
        let updated = ok(&router, "UpdateTransformer", update).await;

        for (operation, body) in [
            ("CreateTransformer", &created),
            ("GetTransformer", &fetched),
            ("UpdateTransformer", &updated),
        ] {
            let case = format!("{case} on {operation}");
            for (member, _) in members {
                let shape = shape_of
                    .iter()
                    .find(|(name, _)| name == member)
                    .map(|(_, shape)| *shape)
                    .expect("every member of a row is one of the three");
                assert_discriminated(&case, body, member, shape);
                // The discriminator never displaces the structure's own content: the
                // members the request sent are reported unchanged beside it.
                let reported = without_type_discriminators(&body[*member]);
                let sent = members
                    .iter()
                    .find(|(name, _)| name == member)
                    .map(|(_, value)| value.clone())
                    .expect("the row's own value");
                assert_eq!(
                    reported, sent,
                    "case {case:?}: {member} must report what was stored beside its \
                     discriminator, got {reported} vs {sent}"
                );
            }
            // A member the create did not name is absent, discriminator and all.
            for (member, _) in shape_of {
                if members.iter().any(|(name, _)| *name == member) {
                    continue;
                }
                assert!(
                    body.get(member).is_none_or(Value::is_null),
                    "case {case:?}: {member} was never stored, so it must be absent, got {:?}",
                    body[member]
                );
            }
        }
    }
}

/// RULE 3: the discriminator covers the shapes the recordings show it on and nothing else —
/// not the response's own top-level body, not a list summary, and not the other structures
/// a body nests (a capability's configuration, an X12 format option, an S3 location).
///
/// The rows span every implemented response of the four resource kinds: the creates, the
/// reads, the updates, the lists and `ListTagsForResource`.
#[tokio::test]
async fn nothing_outside_the_recorded_shapes_carries_a_discriminator() {
    let router = b2bi_router();
    let references = references(&router).await;

    let mut bodies: Vec<(String, Value)> = Vec::new();
    for kind in KINDS {
        let (create, get, update, list) = kind.operations();
        let created = ok(
            &router,
            create,
            create_request(
                kind,
                &format!("boundary-{}", kind.id_member()),
                &references,
                &[("env", "prod"), ("team", "integration")],
            ),
        )
        .await;
        let id = text(&created, kind.id_member());
        let fetched = ok(&router, get, json!({ kind.id_member(): id.clone() })).await;
        let updated = ok(
            &router,
            update,
            rename_request(kind, &id, &format!("boundary-renamed-{}", kind.id_member())),
        )
        .await;
        let listed = ok(&router, list, json!({})).await;
        let tags = ok(
            &router,
            "ListTagsForResource",
            json!({ "ResourceARN": text(&fetched, &format!("{}Arn", kind.id_member().trim_end_matches("Id"))) }),
        )
        .await;
        for (operation, body) in [
            (create, created),
            (get, fetched),
            (update, updated),
            (list, listed.clone()),
            ("ListTagsForResource", tags),
        ] {
            bodies.push((format!("{operation} of a {kind:?}"), body));
        }

        // A list summary is this service's own shape: no entry carries a discriminator,
        // even though the enclosing response is serialized through the same path.
        let entries = listed[kind.list_member()]
            .as_array()
            .unwrap_or_else(|| panic!("{list} must report a list, got {listed}"))
            .clone();
        assert!(
            !entries.is_empty(),
            "the boundary is only pinned by a NON-empty {}, got {listed}",
            kind.list_member()
        );
        for (index, entry) in entries.iter().enumerate() {
            assert!(
                entry.get("__type").is_none(),
                "{list}: {}[{index}] carries no discriminator, got {entry}",
                kind.list_member()
            );
        }
    }

    for (case, body) in &bodies {
        // The top-level body never carries one: the operation names its shape.
        assert!(
            body.get("__type").is_none(),
            "case {case:?}: a success body carries no top-level __type, got {body}"
        );
        // And every discriminator anywhere inside it names one of the recorded shapes, in
        // the shared internal model's namespace.
        for (path, value) in discriminators(body) {
            let shape = value
                .strip_prefix(&format!("{}#", wire::COMMON_MODEL_NAMESPACE))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: {path} carries __type {value:?}, which is not in the \
                         shared internal model's namespace"
                    )
                });
            assert!(
                wire::DISCRIMINATED_SHAPES.contains(&shape),
                "case {case:?}: {path} carries __type {value:?}, and {shape} is not one of the \
                 shapes the recordings show a discriminator on: {:?}",
                wire::DISCRIMINATED_SHAPES
            );
        }
    }

    // The structures a capability nests — its configuration union, the X12 details inside
    // it and both S3 locations — carry none.
    let capability = bodies
        .iter()
        .find(|(case, _)| case.starts_with("GetCapability"))
        .map(|(_, body)| body.clone())
        .expect("the capability read is one of the rows");
    for pointer in [
        "/configuration",
        "/configuration/edi",
        "/configuration/edi/type",
        "/configuration/edi/type/x12Details",
        "/configuration/edi/inputLocation",
        "/configuration/edi/outputLocation",
    ] {
        let nested = capability
            .pointer(pointer)
            .unwrap_or_else(|| panic!("expected {pointer} in {capability}"));
        assert!(
            nested.get("__type").is_none(),
            "case {pointer:?}: this service's own nested structure carries no discriminator, \
             got {nested}"
        );
    }

    // …and so does the X12 format option inside a DISCRIMINATED structure: the
    // discriminator belongs to the shapes the recordings name, not to everything under one.
    let transformer = bodies
        .iter()
        .find(|(case, _)| case.starts_with("GetTransformer"))
        .map(|(_, body)| body.clone())
        .expect("the transformer read is one of the rows");
    for pointer in [
        "/inputConversion/formatOptions",
        "/inputConversion/formatOptions/x12",
    ] {
        let nested = transformer
            .pointer(pointer)
            .unwrap_or_else(|| panic!("expected {pointer} in {transformer}"));
        assert!(
            nested.get("__type").is_none(),
            "case {pointer:?}: a format option carries no discriminator, got {nested}"
        );
    }
}

// ---------------------------------------------------------------------------
// The catalog against its source
// ---------------------------------------------------------------------------

/// This crate's Smithy model — the source the catalog in `service::wire` is a transcription
/// of, read back here so the two cannot drift.
const MODEL_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/smithy-model/com-amazonaws-b2bi-b2bi.json"
);

/// The serializer is handed a structure MEMBER's schema, not its target's, so it resolves
/// the member through `wire::DISCRIMINATED_MEMBERS`. That catalog is a transcription of this
/// crate's Smithy model, and this re-derives it from the model: every structure member
/// targeting one of the discriminated shapes must be listed with that shape, every listed
/// row must exist in the model, and every discriminated shape must be a structure the model
/// declares.
#[test]
fn the_catalog_of_discriminated_members_is_the_model_s_own() {
    let source = std::fs::read_to_string(MODEL_PATH)
        .unwrap_or_else(|error| panic!("the Smithy model at {MODEL_PATH} must be readable: {error}"));
    let model: Value = serde_json::from_str(&source).expect("the Smithy model parses as JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the Smithy model carries a shapes map");

    let mut from_model: Vec<(String, String)> = Vec::new();
    for (shape_id, shape) in shapes {
        if shape.get("type").and_then(Value::as_str) != Some("structure") {
            continue;
        }
        let Some(members) = shape.get("members").and_then(Value::as_object) else {
            continue;
        };
        for (member_name, member) in members {
            let Some(target) = member.get("target").and_then(Value::as_str) else {
                continue;
            };
            let local = target.rsplit('#').next().unwrap_or(target);
            if !wire::DISCRIMINATED_SHAPES.contains(&local) {
                continue;
            }
            let case = format!("{shape_id}${member_name}");
            assert!(
                wire::DISCRIMINATED_MEMBERS
                    .iter()
                    .any(|(name, shape)| *name == member_name.as_str() && *shape == local),
                "case {case:?}: the model targets {local} from member {member_name:?}, which \
                 wire::DISCRIMINATED_MEMBERS does not carry as ({member_name:?}, {local:?}); got \
                 {:?}",
                wire::DISCRIMINATED_MEMBERS
            );
            from_model.push((member_name.clone(), local.to_string()));
        }
    }

    for (member_name, shape) in wire::DISCRIMINATED_MEMBERS {
        assert!(
            from_model
                .iter()
                .any(|(name, target)| name == member_name && target == shape),
            "case {member_name:?}: the catalog claims it targets {shape}, which no structure \
             member of the model does"
        );
    }

    for shape in wire::DISCRIMINATED_SHAPES {
        let suffix = format!("#{shape}");
        assert!(
            shapes.iter().any(|(id, declared)| id.ends_with(&suffix)
                && declared.get("type").and_then(Value::as_str) == Some("structure")),
            "case {shape:?}: a discriminated shape must be a structure the model declares"
        );
    }
}

/// RULE 1's reach: every timestamp this model declares is a `date-time` one, which is what
/// makes the one correction at the wire seam correct for the whole service — a member the
/// model gave another form would need its own.
///
/// Re-derived from the model: every shape of type `timestamp`, and every member targeting
/// one, must declare `@timestampFormat("date-time")`.
#[test]
fn every_timestamp_the_model_declares_is_a_date_time_one() {
    const DATE_TIME: &str = "date-time";
    const TIMESTAMP_FORMAT: &str = "smithy.api#timestampFormat";

    let source = std::fs::read_to_string(MODEL_PATH)
        .unwrap_or_else(|error| panic!("the Smithy model at {MODEL_PATH} must be readable: {error}"));
    let model: Value = serde_json::from_str(&source).expect("the Smithy model parses as JSON");
    let shapes = model
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the Smithy model carries a shapes map");

    let declared_format = |shape: &Value| -> Option<String> {
        shape
            .get("traits")
            .and_then(|traits| traits.get(TIMESTAMP_FORMAT))
            .and_then(Value::as_str)
            .map(str::to_string)
    };

    let mut timestamps = 0usize;
    for (shape_id, shape) in shapes {
        if shape.get("type").and_then(Value::as_str) != Some("timestamp") {
            continue;
        }
        timestamps += 1;
        assert_eq!(
            declared_format(shape).as_deref(),
            Some(DATE_TIME),
            "case {shape_id:?}: a timestamp shape of this model declares the date-time form"
        );
    }
    assert!(
        timestamps > 0,
        "the model must declare at least one timestamp shape, or this rule is about nothing"
    );
}
