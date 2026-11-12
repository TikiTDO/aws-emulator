// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule of ordering: a refusal that is a fact of the REQUEST
//! alone is answered before the store is consulted, so it does not depend on whether the
//! `ResourceARN` beside it names a record this service holds.
//!
//! The two tag-pair rules this model states only in prose — a key "can't start with
//! `aws:`" (`Tag$Key`) and "Spaces are not allowed in tag values" (the `Tag` shape,
//! `smithy_query(query="shape",shape="Tag")`) — are read off the pair and off nothing
//! else. So `TagResource` answers them with the modelled `ValidationException` at HTTP
//! 400, in the very same wording, whether the ARN names a live resource, a resource that
//! was never created, a resource of a type this service does not name, or a resource of
//! another region; and nothing is written either way. The rules a handler can only judge
//! against the RESOURCE — the per-resource tag ceiling — stay on the other side of the
//! lookup, so an ARN that names nothing is still refused as the missing resource it is.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the `MultiServiceRouter`,
//! reusing the harness of `gateway_lifecycle`, so what is asserted is the wire behaviour.
//!
//! The tables below span that class rather than one request: offending pairs differing in
//! which rule they break, in the case of the reserved prefix and in which space character
//! a value carries; the offending pair sitting alone and in the middle of a longer `Tags`
//! list; and ARNs differing in resource type, in how they came to name nothing and in
//! region — crossed with resources that really exist, which is the answer the rest must
//! match.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{
    GATEWAY_NOT_FOUND, HYPERVISOR_NOT_FOUND, MAX_TAGS_PER_RESOURCE, VIRTUAL_MACHINE_NOT_FOUND,
};

use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, never_created_arn, persisted_client_tags, Reply,
    REGION,
};
use crate::gateway_server_association::{arn_of, import, message_of};

/// What one answer said, reduced to the parts a client branches on.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: u16,
    /// The awsJson error discriminator, namespace stripped.
    shape: Option<String>,
    /// The `ErrorCode` member of the modelled error shape, absent where it states none.
    error_code: Option<String>,
    message: String,
}

fn answer_of(reply: &Reply) -> Answer {
    Answer {
        status: reply.status,
        shape: reply.code.clone(),
        error_code: reply.str_at("/ErrorCode").map(str::to_string),
        message: message_of(reply),
    }
}

/// One `TagResource` call carrying `pairs` in the order given.
async fn tag(router: &MultiServiceRouter, arn: &str, pairs: &[(String, String)]) -> Reply {
    let tags: Vec<Value> = pairs
        .iter()
        .map(|(key, value)| json!({ "Key": key, "Value": value }))
        .collect();
    call(
        router,
        "TagResource",
        &json!({ "ResourceARN": arn, "Tags": tags }),
    )
    .await
}

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// Pairs no member of this service may persist, with the half that breaks a rule.
///
/// The reserved-prefix rows span the case of the prefix (the reservation is
/// case-insensitive) and what follows it; the space rows span the Unicode space family —
/// the separators the modelled `TagKey` pattern spells out as `\p{Z}`, where a KEY is
/// deliberately allowed them (`smithy_query(query="shape",shape="TagKey")`) — the tab and
/// line forms, and where in the value the space sits.
fn refused_pairs() -> Vec<(String, String, String, String)> {
    // (case, key, value, the half the refusal must report)
    [
        ("the reserved prefix in lower case", "aws:created-by", "me", "aws:created-by"),
        ("the reserved prefix in upper case", "AWS:Owner", "me", "AWS:Owner"),
        ("the reserved prefix in mixed case", "Aws:Owner", "me", "Aws:Owner"),
        ("nothing but the reserved prefix", "aws:", "me", "aws:"),
        ("an ASCII space between words", "Owner", "two words", "two words"),
        ("a leading ASCII space", "Owner", " leading", " leading"),
        ("a trailing ASCII space", "Owner", "trailing ", "trailing "),
        ("a horizontal tab", "Owner", "two\tfields", "two\tfields"),
        ("a no-break space (U+00A0)", "Owner", "non\u{00a0}breaking", "non\u{00a0}breaking"),
        ("an ideographic space (U+3000)", "Owner", "wide\u{3000}space", "wide\u{3000}space"),
        ("both rules broken at once", "aws:owner", "two words", "aws:owner"),
    ]
    .into_iter()
    .map(|(case, key, value, offending)| {
        (
            case.to_string(),
            key.to_string(),
            value.to_string(),
            offending.to_string(),
        )
    })
    .collect()
}

/// A gateway ARN of a region this endpoint was not addressed as — a resource this region
/// holds no record of, whatever its own stores say.
fn another_region_arn() -> String {
    format!(
        "arn:aws:backup-gateway:eu-west-1:{}:gateway/bgw-0000000000000666",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// ARNs this service holds no record under, each well formed enough to reach the handler
/// rather than the boundary validator: every resource type this service's ARNs can name,
/// a type it does not name at all, and another region's.
fn arns_naming_nothing() -> Vec<(String, String)> {
    vec![
        (
            "a gateway that was never created".to_string(),
            never_created_arn(),
        ),
        (
            "another gateway id".to_string(),
            arn_of("gateway", "bgw-4444444444nonexistent"),
        ),
        (
            "a hypervisor that was never imported".to_string(),
            arn_of("hypervisor", "hv-4444444444444444"),
        ),
        (
            "a virtual machine no sync ever reported".to_string(),
            arn_of("vm", "vm-4444444444444444"),
        ),
        (
            "a resource type this service does not name".to_string(),
            arn_of("appliance", "ap-4444444444444444"),
        ),
        ("a gateway of another region".to_string(), another_region_arn()),
    ]
}

/// Assert one pair refusal: 400, the modelled `ValidationException`, no `ErrorCode`, and a
/// message naming the half of the pair at `position` that failed and the value it refused —
/// at the validation path the boundary validator would name the member by
/// (`refusal_names_the_member_one_way`).
fn assert_pair_refused(case: &str, position: usize, offending: &str, reply: &Reply) {
    let answer = answer_of(reply);
    assert_eq!(
        (answer.status, answer.shape.as_deref(), answer.error_code.as_deref()),
        (400, Some("ValidationException"), None),
        "case {case:?}: the pair must be refused with 400 ValidationException, got \
         {answer:?}"
    );
    assert!(
        answer.message.starts_with("1 validation error detected: "),
        "case {case:?}: one violated rule must state one validation error, got {answer:?}"
    );
    let at = format!("at 'tags.{position}.member.");
    assert!(
        answer.message.contains(&at),
        "case {case:?}: the refusal must state which half of which pair failed ({at:?}), \
         got {answer:?}"
    );
    assert!(
        answer.message.contains(offending),
        "case {case:?}: the refusal must report the {offending:?} it refused, got {answer:?}"
    );
}

/// The rule: one offending pair, one answer — the pair is refused for being that pair,
/// whether the ARN beside it names a live resource or no resource at all.
#[tokio::test]
async fn a_refused_tag_pair_is_answered_the_same_way_whatever_the_arn_names() {
    let engine = build_engine();
    let router = build_router(&engine);
    let gateway = create(&router, "gw-tag-order").await;
    let hypervisor = import(&router, "hv-tag-order", "198.51.100.120").await;

    // A tag on each live resource, so "nothing was written" below means the tag set it
    // already had is the tag set it still has.
    let seeded: BTreeMap<String, String> = [("env".to_string(), "test".to_string())].into();
    for arn in [&gateway, &hypervisor] {
        let accepted = tag(&router, arn, &[pair("env", "test")]).await;
        assert_eq!(
            accepted.status, 200,
            "the seed tag on {arn} must be accepted, got {} {:?}",
            accepted.status, accepted.body
        );
    }

    for (case, key, value, offending) in refused_pairs() {
        // The offending pair alone, and the same pair as the middle one of three: the rule
        // is about the pair, not about where in the list it sits.
        let alone = vec![pair(&key, &value)];
        let among_others = vec![
            pair("Tier", "Gold"),
            pair(&key, &value),
            pair("Owner", "Platform"),
        ];
        for (where_it_sits, position, pairs) in
            [("alone", 1, alone), ("second of three", 2, among_others)]
        {
            let case = format!("{case} ({where_it_sits})");

            // The answer a resource that EXISTS gives is the one answer the condition has.
            let reference = tag(&router, &gateway, &pairs).await;
            assert_pair_refused(&case, position, &offending, &reference);
            let expected = answer_of(&reference);

            let on_hypervisor = tag(&router, &hypervisor, &pairs).await;
            assert_eq!(
                answer_of(&on_hypervisor),
                expected,
                "case {case:?}: a live hypervisor must answer the pair exactly as a live \
                 gateway does, got {:?}",
                on_hypervisor.body
            );

            for (arn_case, arn) in arns_naming_nothing() {
                let refused = tag(&router, &arn, &pairs).await;
                assert_pair_refused(&format!("{case} on {arn_case}"), position, &offending, &refused);
                assert_eq!(
                    answer_of(&refused),
                    expected,
                    "case {case:?} on {arn_case:?}: the pair must be answered exactly as it \
                     is for a resource that exists, got {:?}",
                    refused.body
                );
            }
        }

        for (resource, arn) in [("gateway", &gateway), ("hypervisor", &hypervisor)] {
            let held = persisted_client_tags(&engine, arn);
            assert_eq!(
                held, seeded,
                "case {case:?}: a refused pair must leave the {resource} carrying \
                 {seeded:?}, the store holds {held:?}"
            );
        }
    }
}

/// The other side of the ordering: a pair this service MAY persist leaves nothing for the
/// request half to refuse, so an ARN naming no record takes the missing-resource refusal
/// for the resource type it names — the fix for the pair rules did not swallow the lookup.
#[tokio::test]
async fn a_pair_this_service_may_persist_still_takes_the_missing_resource_refusal() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, an ARN this store holds no record under, the status and the `ErrorCode` the
    // refusal states). Every resource type this service names is answered 400 with the
    // code naming it; a type it does not name keeps the status its modelled shape declares.
    let rows: Vec<(&str, String, u16, Option<&str>)> = vec![
        (
            "a gateway that was never created",
            never_created_arn(),
            400,
            Some(GATEWAY_NOT_FOUND),
        ),
        (
            "a hypervisor that was never imported",
            arn_of("hypervisor", "hv-5555555555555555"),
            400,
            Some(HYPERVISOR_NOT_FOUND),
        ),
        (
            "a virtual machine no sync ever reported",
            arn_of("vm", "vm-5555555555555555"),
            400,
            Some(VIRTUAL_MACHINE_NOT_FOUND),
        ),
        (
            "a resource type this service does not name",
            arn_of("appliance", "ap-5555555555555555"),
            404,
            None,
        ),
        (
            "a gateway of another region",
            another_region_arn(),
            400,
            Some(GATEWAY_NOT_FOUND),
        ),
    ];

    for (case, arn, status, error_code) in rows {
        let refused = tag(&router, &arn, &[pair("Department", "Finance")]).await;
        let answer = answer_of(&refused);
        assert_eq!(
            (
                answer.status,
                answer.shape.as_deref(),
                answer.error_code.as_deref()
            ),
            (status, Some("ResourceNotFoundException"), error_code),
            "case {case:?}: an acceptable pair sent to an ARN naming nothing must be \
             refused as the missing resource, got {answer:?}"
        );
    }

    // And the same pair is accepted on a resource that exists, so the refusals above are
    // about the ARN and not about the pair.
    let gateway = create(&router, "gw-tag-order-control").await;
    let accepted = tag(&router, &gateway, &[pair("Department", "Finance")]).await;
    assert_eq!(
        accepted.status, 200,
        "an acceptable pair must be accepted on a resource that exists, got {} {:?}",
        accepted.status, accepted.body
    );
    let held = persisted_client_tags(&engine, &gateway);
    assert_eq!(
        held.get("Department").map(String::as_str),
        Some("Finance"),
        "the accepted pair must read back out of the store, which holds {held:?}"
    );
    assert_eq!(
        accepted.str_at("/ResourceARN"),
        Some(gateway.as_str()),
        "the accepted call must echo the ARN the store holds the resource under, got {:?}",
        accepted.body
    );
}

/// Where the line falls: the per-resource tag CEILING is not a fact of the request — it
/// counts the tags the resource already carries — so it stays on the store's side of the
/// lookup. The same oversized request is refused for the ceiling on a resource that
/// exists, and as a missing resource on an ARN that names none.
#[tokio::test]
async fn the_per_resource_tag_ceiling_is_answered_after_the_arn_is_resolved() {
    let engine = build_engine();
    let router = build_router(&engine);
    let gateway = create(&router, "gw-tag-ceiling").await;

    // One pair past the ceiling, every one of them acceptable on its own, so the only rule
    // left to break is the one about the resource.
    let oversized: Vec<(String, String)> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|index| pair(&format!("k{index}"), &format!("v{index}")))
        .collect();

    let refused = tag(&router, &gateway, &oversized).await;
    let answer = answer_of(&refused);
    assert_eq!(
        (
            answer.status,
            answer.shape.as_deref(),
            answer.error_code.as_deref()
        ),
        (400, Some("ValidationException"), None),
        "case the ceiling on a live gateway: the request must be refused with 400 \
         ValidationException, got {answer:?}"
    );
    let at_the_member = format!(
        "at 'tags' failed to satisfy constraint: a resource must carry at most \
         {MAX_TAGS_PER_RESOURCE} tags"
    );
    assert!(
        answer.message.contains(&at_the_member),
        "case the ceiling on a live gateway: the refusal must state the ceiling at the tag \
         member itself ({at_the_member:?}), got {answer:?}"
    );
    let held = persisted_client_tags(&engine, &gateway);
    assert!(
        held.is_empty(),
        "case the ceiling on a live gateway: a refused request must write no tag, the \
         store holds {held:?}"
    );

    for (case, arn) in arns_naming_nothing() {
        let refused = tag(&router, &arn, &oversized).await;
        let answer = answer_of(&refused);
        assert_eq!(
            answer.shape.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: a ceiling the store alone can judge must not be answered for an \
             ARN naming no resource, got {answer:?}"
        );
    }

    // A request exactly AT the ceiling is accepted on the resource that carries no client
    // tag yet — the published bound itself, counted over the tags the resource holds,
    // which for a created gateway includes the AWS-assigned pairs it was stamped with.
    let listed = call(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceArn": gateway }),
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "ListTagsForResource must answer 200 for {gateway}, got {} {:?}",
        listed.status, listed.body
    );
    let assigned = listed.reported_tags().len();
    let at_the_bound: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE - assigned)
        .map(|index| pair(&format!("k{index}"), &format!("v{index}")))
        .collect();
    let accepted = tag(&router, &gateway, &at_the_bound).await;
    assert_eq!(
        accepted.status, 200,
        "case the ceiling exactly: {} tags beside the {assigned} this service assigned \
         must be accepted, got {} {:?}",
        at_the_bound.len(),
        accepted.status,
        accepted.body
    );
    let held = persisted_client_tags(&engine, &gateway);
    assert_eq!(
        held.len(),
        at_the_bound.len(),
        "case the ceiling exactly: every accepted tag must read back out of the store, \
         which holds {held:?}"
    );

    // The region the harness signs for, named so a reader can tell the cross-region row of
    // `arns_naming_nothing` from a row of this one.
    assert!(
        gateway.contains(REGION),
        "the gateway this suite tags must be a record of {REGION}, got {gateway}"
    );
}
