// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the `NotFound` a tag operation answers for an ARN this store holds no
//! resource for, by the KIND the ARN declares.
//!
//! The rule under test is not one call: Elastic Load Balancing words this refusal per
//! resource KIND rather than per operation, so each kind keeps its own template and the
//! three tag operations (`AddTags`, `RemoveTags` and `DescribeTags`, which all take
//! `ResourceArns`) answer the same words for the same kind. The target group kind QUOTES
//! the entry of `ResourceArns` that did not resolve, under its plural noun, exactly as
//! every operation addressing one target group through its own `TargetGroupArn` does; the
//! load balancer kind names NO value, and the listener, rule and trust store kinds keep the
//! collection wording their own list reads answer.
//!
//! So the tables below span what the rule generalizes over: all three tag operations, every
//! taggable kind, several ARN forms per kind (differing in partition, Region, account, name
//! and resource id), and selections of one ARN, of several at once and of an absent ARN
//! beside a live resource. The quoted value is asserted to be the request's own ARN
//! verbatim, so no row can pass on a value the emulator made up; and every refusal is
//! asserted to leave the live resource's tags exactly as they were, so the wording is all
//! that these suites change.
//!
//! Every request goes through the router exactly as a client's would — awsQuery form body
//! in, an `<ErrorResponse>` envelope out — and the helpers are the ones the sibling tag
//! suites already carry (`provisioning_loop`, `resource_tags`, `tag_writes`,
//! `tag_removal`), so the calls under test are built exactly as those suites build them.

use crate::provisioning_loop::{
    account, create_http_target_group, emulator, seed_trust_store, Reply, TrustStoreSeed, REGION,
};
use crate::resource_tags::{absent_arn, describe_tags, sorted};
use crate::tag_removal::remove_tags;
use crate::tag_writes::{add_tags, message, owned, stored};
use crate::target_group_attributes_and_modification::{create_target_group, unescaped};
use smithy_server_runtime::MultiServiceRouter;

/// The three operations whose resource selector is the `ResourceArns` LIST
/// (`smithy_query(query="operation",operation="AddTags,RemoveTags,DescribeTags")`: each
/// takes `ResourceArns` of type list, and each models the five per-kind `NotFound`
/// errors).
const TAG_OPERATIONS: [&str; 3] = ["AddTags", "RemoveTags", "DescribeTags"];

/// The not-found wording a tag operation answers for an unresolved `arn`, by the awsQuery
/// error code of the kind's `NotFound` shape.
///
/// The catalog is the service's own, one entry per kind: `LoadBalancerNotFound` is the
/// recorded answer to a tag write naming a load balancer that was deleted (no value in it),
/// `TargetGroupNotFound` is the recorded answer to a tag removal naming a target group ARN
/// with nothing behind it (the identifier quoted under the plural noun), and the listener,
/// rule and trust store entries are the wording this service's sibling LIST reads answer.
pub(crate) fn tag_not_found_message(code: &str, arn: &str) -> String {
    match code {
        "LoadBalancerNotFound" => "One or more load balancers not found".to_string(),
        "TargetGroupNotFound" => format!("Target groups '{arn}' not found"),
        "ListenerNotFound" => "One or more listeners not found".to_string(),
        "RuleNotFound" => "One or more rules not found".to_string(),
        "TrustStoreNotFound" => "One or more trust stores not found".to_string(),
        other => panic!("case {other:?}: no tag not-found wording is defined for this error code"),
    }
}

/// Whether the kind refused with `code` quotes the unresolved identifier back — read off
/// the catalog itself, so there is no second list of kinds to keep in step with it.
pub(crate) fn names_the_identifier(code: &str) -> bool {
    const PROBE: &str = "arn:aws:elasticloadbalancing:eu-west-1:000000000001:probe/the-value";
    tag_not_found_message(code, PROBE).contains(PROBE)
}

/// One tag operation over `arns`. The tags and keys are the ones the resolution pass never
/// reaches on a refusal, so every operation refuses on the same selection.
async fn tag_call(router: &MultiServiceRouter, operation: &str, arns: &[String]) -> Reply {
    match operation {
        "AddTags" => add_tags(router, arns, &owned(&[("after", "delete")])).await,
        "RemoveTags" => remove_tags(router, arns, &["after".to_string()]).await,
        "DescribeTags" => describe_tags(router, arns).await,
        other => panic!("case {other:?}: this suite drives only the tag operations"),
    }
}

/// An ARN of this service naming `resource` in `partition`, `region` and `account`.
///
/// Those three fields are deliberately NOT checked when an ARN is resolved — an ARN of
/// another Region or account is well formed and simply names no resource this store holds
/// — so they are what a row varies to prove the quoted value comes from the request.
fn arn_in(partition: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:{partition}:elasticloadbalancing:{region}:{account}:{resource}")
}

/// One row of the table: the kind as this suite names it, the awsQuery error code of the
/// kind's `NotFound` shape, and ARNs of that kind that no resource in the store carries.
type AbsentRow = (&'static str, &'static str, Vec<String>);

/// Target-group ARNs no resource of the store carries, differing along everything the
/// quoted value can differ along: the name, the resource id, the Region, the account and
/// the partition. `prefix` is a live target group's ARN up to its name, so the first row is
/// the recorded shape — a name and id never created, spliced onto the live group's own
/// partition/Region/account prefix.
fn absent_target_groups(prefix: &str) -> Vec<String> {
    vec![
        format!("{prefix}/never-created-tg/0123456789abcdef"),
        absent_arn("targetgroup/another-absent-name/fedcba9876543210"),
        arn_in(
            "aws",
            "eu-west-1",
            account(),
            "targetgroup/absent-in-eu/00112233445566aa",
        ),
        arn_in(
            "aws",
            REGION,
            "111122223333",
            "targetgroup/absent-elsewhere/aabbccddeeff0011",
        ),
        arn_in(
            "aws-us-gov",
            "us-gov-west-1",
            "444455556666",
            "targetgroup/absent-in-gov/9988776655443322",
        ),
    ]
}

// ---------------------------------------------------------------------------
// Every tag operation, every kind, every selection: the wording of the KIND.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_tag_operation_refuses_an_absent_arn_with_the_wording_of_its_kind() {
    let (router, engine) = emulator();

    // A live resource, so a selection can mix one that resolves with one that does not,
    // and so the suite can witness that a refused write stored nothing. Seeded through the
    // shared engine for the trust store kind, as the sibling tag suites seed one: a seeded
    // record needs no CA certificates bundle staged in Amazon S3.
    let live = create_http_target_group(&router, "tagkind-live-tg").await;
    let live_tags = owned(&[("env", "prod")]);
    assert_eq!(
        add_tags(&router, std::slice::from_ref(&live), &live_tags)
            .await
            .status,
        200,
        "case tagging the live target group: expected 200"
    );
    let live_trust_store = seed_trust_store(
        &engine,
        0,
        &TrustStoreSeed {
            name: "tagkind-live-ts",
            certificates: 1,
            revoked: 0,
            settled: true,
            tags: &[],
        },
    );

    let prefix = live
        .rsplitn(3, '/')
        .nth(2)
        .expect("case setup: a target group ARN carries a name and an id")
        .to_string();

    let rows: Vec<AbsentRow> = vec![
        (
            "load balancers",
            "LoadBalancerNotFound",
            vec![
                absent_arn("loadbalancer/app/absent-app-lb/0123456789abcdef"),
                absent_arn("loadbalancer/net/absent-net-lb/fedcba9876543210"),
                arn_in(
                    "aws",
                    "ap-south-1",
                    "111122223333",
                    "loadbalancer/gwy/absent-gwy-lb/00112233445566aa",
                ),
            ],
        ),
        ("target groups", "TargetGroupNotFound", absent_target_groups(&prefix)),
        (
            "listeners",
            "ListenerNotFound",
            vec![
                absent_arn("listener/app/absent-listener/0123456789abcdef/fedcba9876543210"),
                arn_in(
                    "aws",
                    "eu-central-1",
                    "111122223333",
                    "listener/net/absent-net-listener/00112233445566aa/aabbccddeeff0011",
                ),
            ],
        ),
        (
            "rules",
            "RuleNotFound",
            vec![
                absent_arn(
                    "listener-rule/app/absent-rule/0123456789abcdef/fedcba9876543210/00112233445566aa",
                ),
                arn_in(
                    "aws-cn",
                    "cn-north-1",
                    "111122223333",
                    "listener-rule/net/absent-cn-rule/aabbccddeeff0011/9988776655443322/5544332211009988",
                ),
            ],
        ),
        (
            "trust stores",
            "TrustStoreNotFound",
            vec![
                absent_arn("truststore/absent-trust-store/0123456789abcdef"),
                arn_in(
                    "aws",
                    "sa-east-1",
                    "111122223333",
                    "truststore/absent-sa-trust-store/fedcba9876543210",
                ),
            ],
        ),
    ];

    for (kind, code, arns) in &rows {
        // The selections differ along what the rule generalizes over: which ARN of the kind
        // is named, how many are named at once, and whether one that resolves sits beside
        // them. The entry that does not resolve is the one the refusal is about — the first
        // such entry in request order, which is also the entry that picks the error kind.
        let mut selections: Vec<(String, Vec<String>, String)> = arns
            .iter()
            .map(|arn| (format!("one {kind} ARN ({arn})"), vec![arn.clone()], arn.clone()))
            .collect();
        selections.push((
            format!("every absent {kind} ARN at once"),
            arns.clone(),
            arns[0].clone(),
        ));
        selections.push((
            format!("an absent {kind} ARN behind a live target group"),
            vec![live.clone(), arns[0].clone()],
            arns[0].clone(),
        ));
        selections.push((
            format!("an absent {kind} ARN behind a live trust store"),
            vec![live_trust_store.clone(), arns[0].clone()],
            arns[0].clone(),
        ));

        for operation in TAG_OPERATIONS {
            for (selection, named, unresolved) in &selections {
                let case = format!("{operation} over {selection}");
                let refused = tag_call(&router, operation, named).await;
                assert_eq!(
                    (refused.status, refused.error_code().as_str()),
                    (400, *code),
                    "case {case:?}: expected 400 {code}, got {} body {}",
                    refused.status,
                    refused.body
                );
                // The framework's XML codec escapes the quotes a message carries, so the
                // VALUE is what is compared — through the crate's one reader for that
                // encoding.
                let said = unescaped(&message(&refused));
                let expected = tag_not_found_message(code, unresolved);
                assert_eq!(
                    said, expected,
                    "case {case:?}: expected the message {expected:?}, got {said:?}"
                );
                // A kind whose template carries an identifier carries the request's OWN
                // ARN, and only the one that did not resolve; a kind whose template carries
                // none names no ARN of the request at all.
                for arn in named {
                    let quoted = said.contains(arn.as_str());
                    let owed = names_the_identifier(code) && arn == unresolved;
                    assert_eq!(
                        quoted, owed,
                        "case {case:?}: expected the message to quote {arn} only when the \
                         kind's template names an unresolved identifier ({owed}), got {said:?}"
                    );
                }
            }
        }
    }

    // Every refusal above left the live resource exactly as it was: the wording is what
    // this rule is about, not what a tag call does.
    let still = stored(&router, &live).await;
    assert_eq!(
        still,
        sorted(&live_tags),
        "case the live target group survives every refusal: got {still:?}"
    );
}

// ---------------------------------------------------------------------------
// The parameterised kind: the quoted value is the request's own ARN, and the
// live resource keeps every key.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_absent_target_group_is_refused_with_the_arn_the_request_carried() {
    let (router, _engine) = emulator();

    // A live target group with two keys, one of which is then withdrawn: the recorded
    // shape of this flow, and what proves a refusal beside it withdraws nothing.
    let live = create_target_group(&router, "tagkind-tg", &[("TargetType", "lambda")]).await;
    let both = owned(&[("env", "test"), ("owner", "sim2real")]);
    assert_eq!(
        add_tags(&router, std::slice::from_ref(&live), &both)
            .await
            .status,
        200,
        "case tagging the live target group: expected 200"
    );
    assert_eq!(
        stored(&router, &live).await,
        sorted(&both),
        "case the write of two keys: expected both keys to be reported"
    );
    let removed = remove_tags(
        &router,
        std::slice::from_ref(&live),
        &["env".to_string()],
    )
    .await;
    assert_eq!(
        removed.status, 200,
        "case withdrawing one of two keys: expected 200, got {} body {}",
        removed.status, removed.body
    );
    let kept = owned(&[("owner", "sim2real")]);
    assert_eq!(
        stored(&router, &live).await,
        sorted(&kept),
        "case withdrawing one of two keys: expected the other key to survive"
    );

    let prefix = live
        .rsplitn(3, '/')
        .nth(2)
        .expect("case setup: a target group ARN carries a name and an id")
        .to_string();

    for absent in absent_target_groups(&prefix) {
        let expected = format!("Target groups '{absent}' not found");
        for operation in TAG_OPERATIONS {
            for (shape, named) in [
                ("on its own", vec![absent.clone()]),
                (
                    "behind the live target group",
                    vec![live.clone(), absent.clone()],
                ),
                (
                    "named twice",
                    vec![absent.clone(), absent.clone()],
                ),
            ] {
                let case = format!("{operation} over {absent} {shape}");
                let refused = tag_call(&router, operation, &named).await;
                assert_eq!(
                    (refused.status, refused.error_code().as_str()),
                    (400, "TargetGroupNotFound"),
                    "case {case:?}: expected 400 TargetGroupNotFound, got {} body {}",
                    refused.status,
                    refused.body
                );
                let said = unescaped(&message(&refused));
                assert_eq!(
                    said, expected,
                    "case {case:?}: expected the message {expected:?}, got {said:?}"
                );
            }
        }
    }

    // The refusals above withdrew nothing and stored nothing: the live group still carries
    // exactly the key the successful removal left on it.
    let still = stored(&router, &live).await;
    assert_eq!(
        still,
        sorted(&kept),
        "case the live target group survives every refusal: got {still:?}"
    );
}
