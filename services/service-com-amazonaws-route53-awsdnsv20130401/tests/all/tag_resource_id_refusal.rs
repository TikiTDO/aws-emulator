// SPDX-License-Identifier: CC0-1.0

//! The field a tag operation's refusal of a RESOURCE ID names.
//!
//! The rule under test: across Route 53's tag surface — `ListTagsForResource`,
//! `ListTagsForResources` and `ChangeTagsForResource`, which all take a `ResourceType`
//! plus the id(s) of resources of that type, each typed `TagResourceId` (`@length 0..64`,
//! smithy_query(query="shape",shape="TagResourceId")) — a resource id past that maximum
//! refuses the whole call with HTTP 400 `InvalidInput`, and the constraint clause states
//! the id field of the resource TYPE the request names (`healthCheckId` for
//! `healthcheck`, `hostedZoneId` for `hostedzone`, the member each of
//! `GetHealthCheckRequest`/`GetDNSSECRequest` spells for that resource's own id, in
//! lowerCamel) with the submitted value quoted — never the request's own list path or a
//! position in it.
//!
//! So the tables below differ along what that rule generalises over: which of the three
//! tag operations is called, which of the two taggable resource types it addresses, how
//! far past the maximum the value is and what it is made of, WHERE in a batch the
//! offending id sits, and how many of a batch's ids violate the bound (which the clause
//! count states). An id AT the maximum is the other side of the rule: it is no
//! constraint violation at all, so it is answered by the resolution the operation does.
//!
//! Every test drives the service through the real router, using the SAME harness the
//! other tag suites use, so what is asserted is the HTTP/restXml behaviour a Route 53
//! client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::health_checks::{create_health_check, health_check_id};
use crate::hosted_zone_records::{call, create_zone_ok, router, Reply};
use crate::resource_tags::{change_tags, list_tags};
use crate::resource_tags_batch::{list_tags_batch, list_tags_batch_body};

// ---------------------------------------------------------------------------
// The message the rule prescribes
// ---------------------------------------------------------------------------

/// The maximum length a tag operation's resource id may have. From the model, not from
/// this test: every one of them targets `TagResourceId`, `@length min 0 max 64`
/// (smithy_query(query="shape",shape="TagResourceId")).
const MAX_RESOURCE_ID: usize = 64;

/// The id field a resource type reports its OWN id at — the member the model spells for
/// it where an operation takes that id directly (`GetHealthCheckRequest$HealthCheckId`,
/// `GetDNSSECRequest$HostedZoneId`), in the lowerCamel a member path is spelled in.
///
/// The catalog is the value set of `TagResourceType`
/// (smithy_query(query="shape",shape="TagResourceType")), so a type outside it is a test
/// bug rather than a case.
fn id_field(resource_type: &str) -> &'static str {
    match resource_type {
        "healthcheck" => "healthCheckId",
        "hostedzone" => "hostedZoneId",
        other => panic!("{other:?} is not a modelled TagResourceType"),
    }
}

/// The clause a value past the resource-id maximum earns, stated at `field`.
fn length_clause(value: &str, field: &str) -> String {
    format!(
        "Value '{value}' at '{field}' failed to satisfy constraint: Member must have length \
         less than or equal to {MAX_RESOURCE_ID}"
    )
}

/// The whole message `clauses` are stated in: the AWS constraint-violation envelope,
/// whose count is the number of clauses.
fn envelope(clauses: &[String]) -> String {
    match clauses {
        [single] => format!("1 validation error detected: {single}"),
        many => format!(
            "{} validation errors detected: {}",
            many.len(),
            many.join("; ")
        ),
    }
}

/// An id of `length` characters built from `seed`, so two rows of a table differ in the
/// VALUE they submit as well as in its length.
fn over_long_id(seed: &str, length: usize) -> String {
    seed.chars().cycle().take(length).collect()
}

/// Assert that `reply` is the refusal the rule prescribes for `clauses`.
fn assert_refused_with(reply: &Reply, case: &str, clauses: &[String]) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "InvalidInput"),
        "case {case:?}: expected HTTP 400 InvalidInput, got {} / {} with body {}",
        reply.status,
        reply.error_code(),
        reply.body
    );
    let message = reply.first("Error/Message").unwrap_or_default();
    assert_eq!(
        message,
        envelope(clauses),
        "case {case:?}: expected the constraint violation stated at the resource type's own \
         id field, got {message:?}"
    );
    assert!(
        !message.contains("resourceIds") && !message.contains(".member"),
        "case {case:?}: the message must name no request list path, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// One over-long id, across the operations and the resource types
// ---------------------------------------------------------------------------

/// One row of the single-violation table: which operation is called, which resource type
/// it addresses, and the id it is given.
#[derive(Debug, Clone)]
struct OneRow {
    case: &'static str,
    operation: Operation,
    resource_type: &'static str,
    /// The seed the submitted id is built from, and its length.
    seed: &'static str,
    length: usize,
}

/// Which tag operation a row calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    /// `ListTagsForResources`, whose ids travel in the request body as a list.
    Batch,
    /// `ListTagsForResource`, whose id is a URI label.
    Single,
    /// `ChangeTagsForResource`, whose id is a URI label and which also carries tags.
    Change,
}

/// Call `row`'s operation with `id` as the only resource id it names.
async fn call_with_id(router: &MultiServiceRouter, row: &OneRow, id: &str) -> Reply {
    match row.operation {
        Operation::Batch => list_tags_batch(router, row.resource_type, &[id]).await,
        Operation::Single => list_tags(router, row.resource_type, id).await,
        Operation::Change => {
            change_tags(router, row.resource_type, id, &[("Owner", "dns-team")], &[]).await
        }
    }
}

#[tokio::test]
async fn an_over_long_resource_id_is_refused_at_the_resource_types_id_field() {
    let rows = [
        OneRow {
            case: "the batch read, one character past the maximum",
            operation: Operation::Batch,
            resource_type: "healthcheck",
            seed: "z",
            length: MAX_RESOURCE_ID + 1,
        },
        OneRow {
            case: "the batch read, far past the maximum",
            operation: Operation::Batch,
            resource_type: "healthcheck",
            seed: "0123456789",
            length: 100,
        },
        OneRow {
            case: "the batch read, a hosted-zone id past the maximum",
            operation: Operation::Batch,
            resource_type: "hostedzone",
            seed: "Z1A2B3C4",
            length: 200,
        },
        OneRow {
            case: "the single read, one character past the maximum",
            operation: Operation::Single,
            resource_type: "healthcheck",
            seed: "abc",
            length: MAX_RESOURCE_ID + 1,
        },
        OneRow {
            case: "the single read, a hosted-zone id past the maximum",
            operation: Operation::Single,
            resource_type: "hostedzone",
            seed: "Z9",
            length: 96,
        },
        OneRow {
            case: "the tag change, a health-check id past the maximum",
            operation: Operation::Change,
            resource_type: "healthcheck",
            seed: "9f8e7d6c",
            length: 65,
        },
        OneRow {
            case: "the tag change, a hosted-zone id past the maximum",
            operation: Operation::Change,
            resource_type: "hostedzone",
            seed: "ZABC123",
            length: 130,
        },
    ];

    let router = router();
    for row in &rows {
        let id = over_long_id(row.seed, row.length);
        assert_eq!(
            id.chars().count(),
            row.length,
            "case {:?}: the submitted id must be {} characters",
            row.case,
            row.length
        );
        let reply = call_with_id(&router, row, &id).await;
        assert_refused_with(
            &reply,
            row.case,
            &[length_clause(&id, id_field(row.resource_type))],
        );
    }
}

// ---------------------------------------------------------------------------
// Wherever in a batch the over-long id sits, and however many there are
// ---------------------------------------------------------------------------

/// One row of the batch-position table: the ids the batch names, with `{known}` standing
/// for an existing resource's id and `{over:<seed>:<length>}` for an over-long one.
#[derive(Debug, Clone)]
struct BatchRow {
    case: &'static str,
    ids: &'static [&'static str],
}

/// One id of a [`BatchRow`], resolved against the existing resource's id.
fn batch_id(spelling: &str, known: &str) -> String {
    if spelling == "{known}" {
        return known.to_string();
    }
    let inner = spelling
        .strip_prefix("{over:")
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_else(|| panic!("{spelling:?} is not a batch-id spelling this table defines"));
    let (seed, length) = inner
        .split_once(':')
        .unwrap_or_else(|| panic!("{spelling:?} must name a seed and a length"));
    over_long_id(
        seed,
        length
            .parse()
            .unwrap_or_else(|_| panic!("{spelling:?} must name a numeric length")),
    )
}

#[tokio::test]
async fn a_batch_states_the_id_field_wherever_the_over_long_ids_sit() {
    // The rule: the clause names the resource type's id field and no position, so the
    // same message is stated wherever in the batch the offending id sits; a batch with
    // several offending ids states one clause per offending id, in request order, and the
    // envelope counts them.
    let rows = [
        BatchRow {
            case: "the only id is over-long",
            ids: &["{over:z:65}"],
        },
        BatchRow {
            case: "the first of two ids is over-long",
            ids: &["{over:a:70}", "{known}"],
        },
        BatchRow {
            case: "the last of two ids is over-long",
            ids: &["{known}", "{over:b:80}"],
        },
        BatchRow {
            case: "an id in the middle of a batch is over-long",
            ids: &["{known}", "{over:c:90}", "{known}"],
        },
        BatchRow {
            case: "two of three ids are over-long",
            ids: &["{over:d:65}", "{known}", "{over:e:120}"],
        },
        BatchRow {
            case: "every id of a three-id batch is over-long",
            ids: &["{over:f:65}", "{over:g:66}", "{over:h:67}"],
        },
    ];

    let router = router();
    let known = create_zone_ok(&router, "clause-position.example.com", "tag-clause-pos").await;
    let tagged = change_tags(&router, "hostedzone", &known, &[("Env", "prod")], &[]).await;
    assert_eq!(
        tagged.status, 200,
        "tagging the known zone must succeed, got {} with body {}",
        tagged.status, tagged.body
    );

    for row in &rows {
        let ids: Vec<String> = row.ids.iter().map(|id| batch_id(id, &known)).collect();
        let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
        let reply = list_tags_batch(&router, "hostedzone", &borrowed).await;
        let clauses: Vec<String> = ids
            .iter()
            .filter(|id| id.chars().count() > MAX_RESOURCE_ID)
            .map(|id| length_clause(id, id_field("hostedzone")))
            .collect();
        assert_refused_with(&reply, row.case, &clauses);
        assert!(
            !reply.body.contains("<ResourceTagSet>"),
            "case {:?}: a refused batch must answer no tag set; body {}",
            row.case,
            reply.body
        );
    }

    // None of those refusals touched the zone the batches named beside the bad ids.
    let still = list_tags(&router, "hostedzone", &known).await;
    assert_eq!(
        still.status, 200,
        "the known zone must still read back, got {} with body {}",
        still.status, still.body
    );
    assert!(
        still.body.contains("<Key>Env</Key>") && still.body.contains("<Value>prod</Value>"),
        "a refused batch must leave the tags of the resources it named as they were; body {}",
        still.body
    );
}

// ---------------------------------------------------------------------------
// The other side of the bound
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_at_the_maximum_is_no_constraint_violation() {
    // The bound is inclusive (`@length min 0 max 64`), so an id AT it is a well-formed
    // id that simply names no resource: the operation's own resolution answers it, with
    // the not-found error of the type the request addresses and no validation message.
    let rows = [
        ("healthcheck", "NoSuchHealthCheck"),
        ("hostedzone", "NoSuchHostedZone"),
    ];

    let router = router();
    for (resource_type, code) in rows {
        let at_bound = over_long_id("x", MAX_RESOURCE_ID);
        for (case, reply) in [
            (
                "the batch read",
                list_tags_batch(&router, resource_type, &[at_bound.as_str()]).await,
            ),
            (
                "the single read",
                list_tags(&router, resource_type, &at_bound).await,
            ),
        ] {
            assert_eq!(
                (reply.status, reply.error_code()),
                (404, code),
                "case {resource_type:?}/{case:?}: an id at the maximum must be resolved, not \
                 refused; got {} / {} with body {}",
                reply.status,
                reply.error_code(),
                reply.body
            );
            let message = reply.first("Error/Message").unwrap_or_default();
            assert!(
                !message.contains("validation error"),
                "case {resource_type:?}/{case:?}: expected no constraint violation, got \
                 {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Only the resource id is restated
// ---------------------------------------------------------------------------

#[tokio::test]
async fn another_members_violation_keeps_its_own_member_path() {
    // The rule is about the resource id, so a clause about another member of the SAME
    // request keeps the member path that member is reported at: the ids a batch may name
    // is a bound on the LIST (`TagResourceIdList`, `@length min 1 max 10`), and a tag
    // key's length is a bound on the key.
    let router = router();
    let zone = create_zone_ok(&router, "other-member.example.com", "tag-other-member").await;

    // Eleven ids, one past the list maximum, every one of them well-formed.
    let many: Vec<String> = (0..11).map(|index| format!("Z{index:010}")).collect();
    let borrowed: Vec<&str> = many.iter().map(String::as_str).collect();
    let over_sized = list_tags_batch(&router, "hostedzone", &borrowed).await;
    assert_eq!(
        (over_sized.status, over_sized.error_code()),
        (400, "InvalidInput"),
        "eleven ids must be refused, got {} / {} with body {}",
        over_sized.status,
        over_sized.error_code(),
        over_sized.body
    );
    let message = over_sized.first("Error/Message").unwrap_or_default();
    assert!(
        message.contains("at 'resourceIds'") && !message.contains("hostedZoneId"),
        "a bound on the id LIST must be stated at the list member, got {message:?}"
    );

    // `TagKey` is `@length 0..128` (smithy_query(query="shape",shape="TagKey")), and a
    // key past it is reported at the tag's own key member, not at the resource id.
    let long_key = over_long_id("k", 129);
    let refused = change_tags(
        &router,
        "hostedzone",
        &zone,
        &[(long_key.as_str(), "value")],
        &[],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "InvalidInput"),
        "a tag key past its maximum must be refused, got {} / {} with body {}",
        refused.status,
        refused.error_code(),
        refused.body
    );
    let message = refused.first("Error/Message").unwrap_or_default();
    assert!(
        message.contains("at 'addTags.1.member.key'") && !message.contains("hostedZoneId"),
        "a tag key's bound must be stated at the key's own member path, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusal reaches the wire as the service's own envelope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_refusal_is_answered_in_this_services_error_envelope() {
    // The restated clause travels in the `<ErrorResponse>` document every other refusal
    // of this service travels in, naming the request it answers, and the batch request
    // that earned it is a no-op: a health check created beside it still reads back.
    let router = router();
    let created = create_health_check(
        &router,
        "tag-clause-envelope",
        "<Type>HTTP</Type><FullyQualifiedDomainName>example.com\
         </FullyQualifiedDomainName><Port>80</Port><ResourcePath>/</ResourcePath>\
         <RequestInterval>30</RequestInterval><FailureThreshold>3</FailureThreshold>",
    )
    .await;
    let check = health_check_id(&created, "the health check the batch names");

    let id = over_long_id("z", 100);
    let reply = call(
        &router,
        "POST",
        "/2013-04-01/tags/healthcheck",
        &list_tags_batch_body(&[check.as_str(), id.as_str()]),
    )
    .await;
    assert_refused_with(
        &reply,
        "a batch naming a known id and an over-long one",
        &[length_clause(&id, "healthCheckId")],
    );
    assert!(
        reply.body.contains("<ErrorResponse")
            && reply.body.contains("<Code>InvalidInput</Code>")
            && reply.first("ErrorResponse/RequestId").is_some_and(|id| !id.is_empty()),
        "the refusal must be answered in this service's error envelope, naming the request; \
         body {}",
        reply.body
    );

    // The refused call changed nothing: the health check it named still carries no tags.
    let after = list_tags(&router, "healthcheck", &check).await;
    assert_eq!(
        after.status, 200,
        "the health check must still read back, got {} with body {}",
        after.status, after.body
    );
    assert!(
        after.body.contains("<Tags/>") || after.body.contains("<Tags></Tags>"),
        "the refused batch must have added no tag; body {}",
        after.body
    );
}
