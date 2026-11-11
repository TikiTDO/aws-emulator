// SPDX-License-Identifier: CC0-1.0

//! `DescribeTags` over the wire: what the store holds on a resource is what the read
//! reports, for every taggable kind of Elastic Load Balancing resource.
//!
//! Every request goes through the router exactly as a client's would — awsQuery form
//! body in, `<DescribeTagsResponse>` XML out — so these tests exercise the wire protocol
//! as well as the handler.
//!
//! The rule under test is not one call: `DescribeTags` answers one `TagDescription` per
//! distinct ARN the request names, carrying the tags persisted on that resource; an ARN
//! whose kind the service knows but whose resource the store does not hold is the
//! `NotFound` error modelled for that kind, and a string that is not an Elastic Load
//! Balancing resource ARN is refused as a malformed request. So the suites below span a
//! table of tag sets (count, empty value, character classes, the published length
//! bounds), every resource kind, and every refusal — the recorded call (one target group,
//! read by ARN) is one row of it.
//!
//! A trust store is SEEDED through the shared engine, using the harness in
//! `provisioning_loop` that the `trust_stores` suite also uses: a seeded record needs no
//! CA certificates bundle staged in Amazon S3 (which is what the create reads one from),
//! and reads and writes go through `service_..._v10::state::service_state`'s store exactly
//! as a handler's do, so a seeded record is the same record the handler serves.

use crate::provisioning_loop::{
    account, action, call, emulator, first_text, members, p, seed_trust_store, Reply,
    TrustStoreSeed, REGION,
};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// One tag set to persist and read back.
pub(crate) type TagSet = (String, Vec<(String, String)>);

/// One reported `TagDescription`: the resource ARN, and its tags sorted by key.
pub(crate) type Described = (String, Vec<(String, String)>);

/// One row of the multi-ARN table: the case name, the ARNs the call names and the
/// descriptions it must then report, in order.
type MultiArnCase = (&'static str, Vec<String>, Vec<Described>);

/// The tag sets every resource kind below is tagged with, one per row.
///
/// The rule generalizes over the tag set itself, so the rows differ in how many tags
/// they carry, whether a value is empty, which character classes the key and value use,
/// and how long they are. The length and character bounds are the model's own for the
/// `Tag` shape (`smithy_query(query="shape",shape="Tag")`: `Key` length 1..128, `Value`
/// length 0..256, both matching `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`).
pub(crate) fn tag_sets() -> Vec<TagSet> {
    let owned = |case: &str, tags: &[(&str, &str)]| {
        (
            case.to_string(),
            tags.iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<Vec<_>>(),
        )
    };
    let mut sets = vec![
        owned("no tags", &[]),
        owned("one tag", &[("env", "prod")]),
        owned(
            "three tags",
            &[
                ("tier", "web"),
                ("env", "prod"),
                ("owner", "platform team"),
            ],
        ),
        owned("a tag with an empty value", &[("marker", "")]),
        owned(
            "every allowed punctuation class",
            &[("ns:key_1.a/b=c+d-e@f", "v:1.a/b=c+d-e@f")],
        ),
        owned(
            "unicode letters and separators",
            &[("propriétaire", "équipe réseau")],
        ),
    ];
    // The published maximum lengths, which a tag is allowed to reach exactly.
    sets.push((
        "key and value at the published maximum lengths".to_string(),
        vec![("k".repeat(128), "v".repeat(256))],
    ));
    sets
}

/// The form fields that ask a create to persist `tags`.
pub(crate) fn tag_fields(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let member = index + 1;
        fields.push(p(&format!("Tags.member.{member}.Key"), key));
        fields.push(p(&format!("Tags.member.{member}.Value"), value));
    }
    fields
}

/// Create a load balancer carrying `tags` and answer its ARN.
async fn create_load_balancer(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let mut form = action("CreateLoadBalancer");
    form.push(p("Name", name));
    form.push(p("Subnets.member.1", "subnet-11111111"));
    form.push(p("Subnets.member.2", "subnet-22222222"));
    form.extend(tag_fields(tags));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create load balancer {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("LoadBalancerArn")
        .unwrap_or_else(|| panic!("case create load balancer {name}: no ARN in {}", reply.body))
}

/// Create a target group carrying `tags` and answer its ARN.
async fn create_target_group(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let mut form = action("CreateTargetGroup");
    form.push(p("Name", name));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("VpcId", "vpc-12345678"));
    form.extend(tag_fields(tags));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create target group {name}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("case create target group {name}: no ARN in {}", reply.body))
}

/// Create a listener on `lb_arn` forwarding to `tg_arn`, carrying `tags`.
async fn create_listener(
    router: &MultiServiceRouter,
    lb_arn: &str,
    tg_arn: &str,
    port: i32,
    tags: &[(String, String)],
) -> String {
    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", &port.to_string()));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", tg_arn));
    form.extend(tag_fields(tags));
    let reply = call(router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case create listener on port {port}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reply
        .text("ListenerArn")
        .unwrap_or_else(|| panic!("case create listener: no ARN in {}", reply.body))
}

/// Seed a trust store named `name` at creation position `index`, carrying `tags`, and
/// answer its ARN — through the shared seeder, which needs no bundle staged for it.
fn seed_tagged_trust_store(
    engine: &Arc<engine::InMemoryEngine>,
    index: usize,
    name: &str,
    tags: &[(String, String)],
) -> String {
    seed_trust_store(
        engine,
        index,
        &TrustStoreSeed {
            name,
            certificates: 1,
            revoked: 0,
            settled: true,
            tags,
        },
    )
}

/// `DescribeTags` over `arns`.
pub(crate) async fn describe_tags(router: &MultiServiceRouter, arns: &[String]) -> Reply {
    let mut form = action("DescribeTags");
    for (index, arn) in arns.iter().enumerate() {
        form.push(p(&format!("ResourceArns.member.{}", index + 1), arn));
    }
    call(router, &form).await
}

/// What the reply reported: one entry per `TagDescription`, its ARN and its tags sorted
/// by key, in the order the reply listed the descriptions.
pub(crate) fn reported(reply: &Reply) -> Vec<Described> {
    members(&reply.body, "TagDescriptions")
        .iter()
        .map(|description| {
            let arn = first_text(description, "ResourceArn").unwrap_or_default();
            let mut tags: Vec<(String, String)> = members(description, "Tags")
                .iter()
                .map(|tag| {
                    (
                        first_text(tag, "Key").unwrap_or_default(),
                        // An empty value serializes as an empty element.
                        first_text(tag, "Value").unwrap_or_default(),
                    )
                })
                .collect();
            tags.sort();
            (arn, tags)
        })
        .collect()
}

/// `tags` sorted by key, which is the order a reported description is compared in.
pub(crate) fn sorted(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut sorted = tags.to_vec();
    sorted.sort();
    sorted
}

/// An ARN of this service that no resource in the store carries.
pub(crate) fn absent_arn(resource: &str) -> String {
    format!(
        "arn:aws:elasticloadbalancing:{REGION}:{}:{resource}",
        account()
    )
}

// ---------------------------------------------------------------------------
// The tags a resource was created with are the tags the read reports, for every
// taggable kind.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_tags_stored_on_a_resource_are_what_describe_tags_reports() {
    let (router, engine) = emulator();

    // One load balancer and one target group the tagged listeners hang off; their own
    // tags are asserted through their own rows below.
    let host_lb = create_load_balancer(&router, "tag-host-lb", &[]).await;
    let host_tg = create_target_group(&router, "tag-host-tg", &[]).await;

    for (row, (case, tags)) in tag_sets().into_iter().enumerate() {
        let lb = create_load_balancer(&router, &format!("tag-lb-{row}"), &tags).await;
        let tg = create_target_group(&router, &format!("tag-tg-{row}"), &tags).await;
        let listener = create_listener(
            &router,
            &host_lb,
            &host_tg,
            8000 + row as i32,
            &tags,
        )
        .await;
        let trust_store = seed_tagged_trust_store(&engine, row, &format!("tag-ts-{row}"), &tags);

        for (kind, arn) in [
            ("load balancer", &lb),
            ("target group", &tg),
            ("listener", &listener),
            ("trust store", &trust_store),
        ] {
            let case = format!("{case} on a {kind}");
            let reply = describe_tags(&router, std::slice::from_ref(arn)).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                reply.status, reply.body
            );
            assert_eq!(
                reported(&reply),
                vec![(arn.clone(), sorted(&tags))],
                "case {case:?}: expected the stored tags of {arn}, got {}",
                reply.body
            );

            // A read changes nothing a later caller sees: the same call answers the
            // same tags.
            let again = describe_tags(&router, std::slice::from_ref(arn)).await;
            assert_eq!(
                reported(&again),
                vec![(arn.clone(), sorted(&tags))],
                "case {case:?} (second read): expected the same tags, got {}",
                again.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// One description per distinct ARN, in the order the request named them.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_description_is_reported_per_distinct_requested_arn_in_request_order() {
    let (router, engine) = emulator();

    let lb_tags = vec![("kind".to_string(), "load-balancer".to_string())];
    let tg_tags = vec![
        ("kind".to_string(), "target-group".to_string()),
        ("env".to_string(), "staging".to_string()),
    ];
    let listener_tags = vec![("kind".to_string(), "listener".to_string())];
    let ts_tags = vec![("kind".to_string(), "trust-store".to_string())];

    let lb = create_load_balancer(&router, "multi-lb", &lb_tags).await;
    let tg = create_target_group(&router, "multi-tg", &tg_tags).await;
    let listener = create_listener(&router, &lb, &tg, 80, &listener_tags).await;
    let trust_store = seed_tagged_trust_store(&engine, 0, "multi-ts", &ts_tags);

    let expected = |arn: &String, tags: &Vec<(String, String)>| (arn.clone(), sorted(tags));

    // Rows differ in how many resources one call names, in which kinds, in what order,
    // and in whether an ARN is repeated.
    let cases: Vec<MultiArnCase> = vec![
        (
            "one resource",
            vec![tg.clone()],
            vec![expected(&tg, &tg_tags)],
        ),
        (
            "every kind, in request order",
            vec![
                lb.clone(),
                tg.clone(),
                listener.clone(),
                trust_store.clone(),
            ],
            vec![
                expected(&lb, &lb_tags),
                expected(&tg, &tg_tags),
                expected(&listener, &listener_tags),
                expected(&trust_store, &ts_tags),
            ],
        ),
        (
            "the reverse order",
            vec![
                trust_store.clone(),
                listener.clone(),
                tg.clone(),
                lb.clone(),
            ],
            vec![
                expected(&trust_store, &ts_tags),
                expected(&listener, &listener_tags),
                expected(&tg, &tg_tags),
                expected(&lb, &lb_tags),
            ],
        ),
        (
            "an ARN named twice is one resource",
            vec![tg.clone(), lb.clone(), tg.clone()],
            vec![expected(&tg, &tg_tags), expected(&lb, &lb_tags)],
        ),
    ];

    for (case, arns, want) in cases {
        let reply = describe_tags(&router, &arns).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reported(&reply),
            want,
            "case {case:?}: expected one description per distinct ARN in request order, got {}",
            reply.body
        );
    }
}

// ---------------------------------------------------------------------------
// An ARN no resource carries: the NotFound error of the kind the ARN names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_arn_no_resource_carries_is_refused_with_the_error_of_its_kind() {
    let (router, _engine) = emulator();

    let known_tags = vec![("env".to_string(), "prod".to_string())];
    let known = create_target_group(&router, "known-tg", &known_tags).await;

    // The ARN forms AWS publishes for this service's resources, one per kind, with the
    // awsQuery error code the model gives that kind's NotFound shape
    // (`smithy_query(query="operation",operation="DescribeTags")`).
    let cases = [
        (
            "an application load balancer",
            absent_arn("loadbalancer/app/absent/0123456789abcdef"),
            "LoadBalancerNotFound",
        ),
        (
            "a network load balancer",
            absent_arn("loadbalancer/net/absent/0123456789abcdef"),
            "LoadBalancerNotFound",
        ),
        (
            "a gateway load balancer",
            absent_arn("loadbalancer/gwy/absent/0123456789abcdef"),
            "LoadBalancerNotFound",
        ),
        (
            "a target group",
            absent_arn("targetgroup/absent/0123456789abcdef"),
            "TargetGroupNotFound",
        ),
        (
            "a listener",
            absent_arn("listener/app/absent/0123456789abcdef/fedcba9876543210"),
            "ListenerNotFound",
        ),
        (
            "a listener rule",
            absent_arn(
                "listener-rule/app/absent/0123456789abcdef/fedcba9876543210/00112233445566aa",
            ),
            "RuleNotFound",
        ),
        (
            "a trust store",
            absent_arn("truststore/absent/0123456789abcdef"),
            "TrustStoreNotFound",
        ),
    ];

    for (case, arn, expected_code) in &cases {
        let refused = describe_tags(&router, std::slice::from_ref(arn)).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {} {}",
            refused.status,
            refused.body
        );

        // A known ARN beside the unknown one is refused the same way: the call reports
        // nothing when any resource it names is missing.
        let mixed = describe_tags(&router, &[known.clone(), arn.clone()]).await;
        assert_eq!(
            (mixed.status, mixed.error_code().as_str()),
            (400, *expected_code),
            "case {case:?} (beside a known ARN): expected 400 {expected_code}, got {} {}",
            mixed.status,
            mixed.body
        );
    }

    // Every refusal left the known resource exactly as it was.
    let still = describe_tags(&router, std::slice::from_ref(&known)).await;
    assert_eq!(
        reported(&still),
        vec![(known.clone(), sorted(&known_tags))],
        "case {:?}: expected the known target group's tags, got {}",
        "the known resource survives every refusal",
        still.body
    );
}

// ---------------------------------------------------------------------------
// A string that is not an Elastic Load Balancing resource ARN.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_string_that_is_not_an_elastic_load_balancing_arn_is_refused() {
    let (router, _engine) = emulator();
    let known = create_target_group(&router, "malformed-known-tg", &[]).await;

    let cases = [
        ("not an ARN at all", "not-an-arn".to_string()),
        ("an empty string", String::new()),
        (
            "an ARN of another service",
            format!("arn:aws:s3:::my-bucket-{}", account()),
        ),
        (
            "an Elastic Load Balancing ARN naming no known resource kind",
            absent_arn("widget/absent/0123456789abcdef"),
        ),
        (
            "an ARN missing its resource part",
            format!("arn:aws:elasticloadbalancing:{REGION}:{}", account()),
        ),
        (
            "a known ARN beside a malformed one",
            "also-not-an-arn".to_string(),
        ),
    ];

    for (case, value) in &cases {
        let arns = if case.starts_with("a known ARN") {
            vec![known.clone(), value.clone()]
        } else {
            vec![value.clone()]
        };
        let refused = describe_tags(&router, &arns).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationError"),
            "case {case:?}: expected 400 ValidationError, got {} {}",
            refused.status,
            refused.body
        );
    }
}

// ---------------------------------------------------------------------------
// How many resources one call may name.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_call_names_between_one_and_twenty_resources() {
    let (router, _engine) = emulator();

    // "You can specify up to 20 resources in a single call" — the model's documentation
    // of `DescribeTagsInput$ResourceArns`. The bound is tested at it, below it and past
    // it; every ARN below names a resource that exists, so only the count can refuse.
    let mut arns = Vec::new();
    for index in 0..21usize {
        let tags = vec![("index".to_string(), index.to_string())];
        arns.push(create_target_group(&router, &format!("bound-tg-{index}"), &tags).await);
    }

    for count in [1usize, 19, 20] {
        let page = describe_tags(&router, &arns[..count]).await;
        assert_eq!(
            page.status, 200,
            "case {count} resources: expected 200, got {} body {}",
            page.status, page.body
        );
        let got = reported(&page);
        assert_eq!(
            got.len(),
            count,
            "case {count} resources: expected {count} descriptions, got {got:?}"
        );
        assert_eq!(
            got.iter().map(|(arn, _)| arn.clone()).collect::<Vec<_>>(),
            arns[..count].to_vec(),
            "case {count} resources: expected the requested ARNs in order, got {got:?}"
        );
        for (index, (_, tags)) in got.iter().enumerate() {
            assert_eq!(
                tags,
                &vec![("index".to_string(), index.to_string())],
                "case {count} resources: expected the tags stored on resource {index}, got {tags:?}"
            );
        }
    }

    let past = describe_tags(&router, &arns[..21]).await;
    assert_eq!(
        (past.status, past.error_code().as_str()),
        (400, "ValidationError"),
        "case {:?}: expected 400 ValidationError, got {} {}",
        "one past the bound",
        past.status,
        past.body
    );

    // Naming no resource at all is refused rather than reported as nothing. `ResourceArns`
    // is modelled required and awsQuery cannot carry an empty list, so this refusal comes
    // from the boundary before the handler runs — answered under this service's own
    // input-refusal code, as `src/service.rs` states it; the handler refuses an empty list
    // for the callers that can express one. Only the status and the fact that nothing was
    // reported are asserted here, because the wording of a boundary refusal belongs to
    // `all/aws_error_wording.rs` and not to this operation.
    let empty = describe_tags(&router, &[]).await;
    assert_eq!(
        empty.status, 400,
        "case {:?}: expected 400, got {} body {}",
        "no resource at all", empty.status, empty.body
    );
    assert_eq!(
        reported(&empty),
        Vec::new(),
        "case {:?}: expected no description, got {}",
        "no resource at all",
        empty.body
    );
}
