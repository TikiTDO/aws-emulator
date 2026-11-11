// SPDX-License-Identifier: CC0-1.0

//! `AddTags` over the wire: what a tag write stores is what a later `DescribeTags`
//! reports, and a write naming a resource the store does not hold is refused with the
//! `NotFound` error modelled for the kind the ARN declares.
//!
//! Every request goes through the router exactly as a client's would — awsQuery form body
//! in, `<AddTagsResponse>` XML out — so these tests exercise the wire protocol as well as
//! the handler, and every assertion about a write is made on what a later READ reports.
//!
//! The rule under test is not one call: a tag write resolves each `ResourceArn` by the
//! resource kind the ARN itself declares, stores the request's tags on the resource the
//! store holds for it (a key already present taking the request's value), and refuses a
//! call naming a resource the store does not hold — whatever the reason it is absent —
//! with that kind's `NotFound` error, having written nothing. So the suites below span a
//! table of tag sets, every taggable resource kind, every refusal and the published
//! per-resource tag bound; the recorded call (one deleted load balancer, tagged with one
//! pair) is one row of it.
//!
//! The resource creators, the awsQuery helpers and the tag-set table are the ones the
//! sibling suites already carry (`provisioning_loop`, `listener_rules`,
//! `resource_tags`), so a tag write is driven against resources created exactly as every
//! other suite creates them. A trust store is SEEDED through the shared engine, as
//! `resource_tags` seeds one: a seeded record needs no CA certificates bundle staged in
//! Amazon S3, which is what the create reads one from.

use crate::listener_rules::{add_rule, create_listener, create_load_balancer, forward_to, host_header};
use crate::provisioning_loop::{
    account, action, call, create_http_target_group, emulator, p, seed_trust_store, Reply,
    TrustStoreSeed, REGION,
};
use crate::resource_tags::{absent_arn, describe_tags, reported, sorted, tag_fields, tag_sets};
use crate::target_group_attributes_and_modification::unescaped;
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

/// One row of the merge table: the case, the tags written first, the tags written next,
/// and what the resource must then carry.
type MergeRow = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<(&'static str, &'static str)>,
    Vec<(&'static str, &'static str)>,
);

/// The request fields of a rule that forwards one host to `tg_arn` — a condition and an
/// action, both of which `CreateRule` requires.
fn rule_fields(tg_arn: &str, host: &str) -> Vec<(String, String)> {
    let mut fields = host_header(host);
    fields.extend(forward_to(tg_arn));
    fields
}

/// One row of the batch table: the case, the ARNs the call names, and the tags it
/// writes to each of them.
type BatchRow = (&'static str, Vec<String>, Vec<(&'static str, &'static str)>);

/// `AddTags` over `arns`, carrying `tags`.
pub(crate) async fn add_tags(
    router: &MultiServiceRouter,
    arns: &[String],
    tags: &[(String, String)],
) -> Reply {
    let mut form = action("AddTags");
    for (index, arn) in arns.iter().enumerate() {
        form.push(p(&format!("ResourceArns.member.{}", index + 1), arn));
    }
    form.extend(tag_fields(tags));
    call(router, &form).await
}

/// `AddTags` over one ARN, carrying `tags`.
pub(crate) async fn add_tags_to(router: &MultiServiceRouter, arn: &String, tags: &[(String, String)]) -> Reply {
    add_tags(router, std::slice::from_ref(arn), tags).await
}

/// Owned tag pairs from literals.
pub(crate) fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// The tags `DescribeTags` reports for `arn`, sorted by key — what a write is asserted
/// through.
pub(crate) async fn stored(router: &MultiServiceRouter, arn: &String) -> Vec<(String, String)> {
    let reply = describe_tags(router, std::slice::from_ref(arn)).await;
    assert_eq!(
        reply.status, 200,
        "case read back {arn}: expected 200, got {} body {}",
        reply.status, reply.body
    );
    reported(&reply)
        .into_iter()
        .next()
        .map(|(_, tags)| tags)
        .unwrap_or_default()
}

/// The `<Message>` of an error envelope.
pub(crate) fn message(reply: &Reply) -> String {
    reply.text("Message").unwrap_or_default()
}

/// A tag set of `count` distinct pairs.
pub(crate) fn distinct_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| (format!("key-{index}"), format!("value-{index}")))
        .collect()
}

// ---------------------------------------------------------------------------
// What a write stores is what a later read reports, for every taggable kind.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_tags_a_write_stores_are_what_a_later_read_reports() {
    let (router, engine) = emulator();

    // The load balancer and target group the per-row listeners and rules hang off; their
    // own tags are never written, so they also witness that a write to one resource
    // leaves the others alone.
    let host_lb = create_load_balancer(&router, "tagw-host-lb").await;
    let host_tg = create_http_target_group(&router, "tagw-host-tg").await;

    for (row, (case, set)) in tag_sets().into_iter().enumerate() {
        if set.is_empty() {
            // `TagList` is modelled with a minimum length of one, so the empty row of the
            // shared table is not an input a tag write can carry; the refusal of a call
            // carrying no tag is asserted in its own test below.
            continue;
        }

        let lb = create_load_balancer(&router, &format!("tagw-lb-{row}")).await;
        let tg = create_http_target_group(&router, &format!("tagw-tg-{row}")).await;
        let listener = create_listener(&router, &host_lb, &host_tg, 9000 + row as i32).await;
        let rule = add_rule(
            &router,
            &case,
            &listener,
            10,
            &rule_fields(&host_tg, &format!("row-{row}.example.com")),
        )
        .await;
        let ts_name = format!("tagw-ts-{row}");
        let trust_store = seed_trust_store(
            &engine,
            row,
            &TrustStoreSeed {
                name: &ts_name,
                certificates: 1,
                revoked: 0,
                settled: true,
                tags: &[],
            },
        );

        for (kind, arn) in [
            ("load balancer", &lb),
            ("target group", &tg),
            ("listener", &listener),
            ("rule", &rule),
            ("trust store", &trust_store),
        ] {
            let case = format!("{case} on a {kind}");
            let reply = add_tags_to(&router, arn, &set).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                reply.status, reply.body
            );
            assert!(
                reply.body.contains("AddTagsResponse"),
                "case {case:?}: expected an AddTagsResponse envelope, got {}",
                reply.body
            );
            assert_eq!(
                stored(&router, arn).await,
                sorted(&set),
                "case {case:?}: expected the written tags on {arn}"
            );

            // Writing the same tags again is the same resource state: a repeated write
            // neither duplicates a key nor drops one.
            let again = add_tags_to(&router, arn, &set).await;
            assert_eq!(
                again.status, 200,
                "case {case:?} (written twice): expected 200, got {} body {}",
                again.status, again.body
            );
            assert_eq!(
                stored(&router, arn).await,
                sorted(&set),
                "case {case:?} (written twice): expected the same tags on {arn}"
            );
        }

        // Nothing was written to the resources the calls did not name.
        for (kind, arn) in [("host load balancer", &host_lb), ("host target group", &host_tg)] {
            assert_eq!(
                stored(&router, arn).await,
                Vec::new(),
                "case {case:?}: expected the {kind} to carry no tag, because no call named it"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A write merges into what the resource already carries.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_write_merges_into_the_tags_the_resource_already_carries() {
    let (router, _engine) = emulator();

    // "If a resource already has a tag with the same key, AddTags updates its value" —
    // the model's own documentation of this operation. The rows differ in whether the
    // second write adds a key, replaces one, repeats one, empties one, or does several
    // of those at once.
    let rows: Vec<MergeRow> = vec![
        (
            "a key the resource does not carry is added",
            vec![("env", "prod")],
            vec![("tier", "web")],
            vec![("env", "prod"), ("tier", "web")],
        ),
        (
            "a key the resource carries takes the new value",
            vec![("env", "prod")],
            vec![("env", "staging")],
            vec![("env", "staging")],
        ),
        (
            "writing a pair the resource already carries changes nothing",
            vec![("env", "prod"), ("tier", "web")],
            vec![("env", "prod")],
            vec![("env", "prod"), ("tier", "web")],
        ),
        (
            "a value can be replaced by the empty value",
            vec![("marker", "set")],
            vec![("marker", "")],
            vec![("marker", "")],
        ),
        (
            "one call both adds and replaces",
            vec![("env", "prod"), ("tier", "web")],
            vec![("env", "dev"), ("owner", "platform team")],
            vec![("env", "dev"), ("owner", "platform team"), ("tier", "web")],
        ),
    ];

    for (row, (case, first, second, expected)) in rows.into_iter().enumerate() {
        // Two kinds per row, so the merge is the rule of a tag write rather than
        // anything about one resource kind.
        let lb = create_load_balancer(&router, &format!("tagm-lb-{row}")).await;
        let tg = create_http_target_group(&router, &format!("tagm-tg-{row}")).await;

        for (kind, arn) in [("load balancer", &lb), ("target group", &tg)] {
            let case = format!("{case} on a {kind}");
            for (step, write) in [("first", &first), ("second", &second)] {
                let reply = add_tags_to(&router, arn, &owned(write)).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?} ({step} write): expected 200, got {} body {}",
                    reply.status, reply.body
                );
            }
            assert_eq!(
                stored(&router, arn).await,
                sorted(&owned(&expected)),
                "case {case:?}: expected the merged tags on {arn}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A resource the store does not hold: the NotFound error of the ARN's kind.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_write_to_a_resource_the_store_does_not_hold_is_refused_with_the_error_of_its_kind() {
    let (router, _engine) = emulator();

    let known = create_http_target_group(&router, "tagn-known-tg").await;
    let known_tags = owned(&[("env", "prod")]);
    let reply = add_tags_to(&router, &known, &known_tags).await;
    assert_eq!(
        reply.status, 200,
        "case tag the known target group: expected 200, got {} body {}",
        reply.status, reply.body
    );

    // A load balancer that was created and then DELETED: the recorded row of this table,
    // and the one no "it never existed" ARN can stand in for.
    let deleted = create_load_balancer(&router, "tagn-deleted-lb").await;
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", &deleted));
    let gone = call(&router, &form).await;
    assert_eq!(
        gone.status, 200,
        "case delete the load balancer: expected 200, got {} body {}",
        gone.status, gone.body
    );

    // One row per taggable kind, in the ARN forms AWS publishes for this service's
    // resources, each with the awsQuery error code the model gives that kind's NotFound
    // shape (`smithy_query(query="operation",operation="AddTags")`).
    let cases = [
        ("a deleted application load balancer", deleted.clone(), "LoadBalancerNotFound"),
        (
            "an application load balancer that never existed",
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

    let writing = owned(&[("after", "delete")]);
    for (case, arn, expected_code) in &cases {
        let refused = add_tags_to(&router, arn, &writing).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {} {}",
            refused.status,
            refused.body
        );
        let said = unescaped(&message(&refused));
        // The wording belongs to the KIND the ARN declares: the catalog of the templates
        // lives in `all/tag_not_found_wording_by_kind.rs`, which is where the rule itself
        // is pinned.
        let expected = crate::tag_not_found_wording_by_kind::tag_not_found_message(
            expected_code,
            arn,
        );
        assert_eq!(
            said, expected,
            "case {case:?}: expected the service's wording {expected:?}, got {said:?}"
        );

        // A live resource named beside the absent one is not tagged either: the write is
        // all-or-nothing across the resources one call names.
        let mixed = add_tags(&router, &[known.clone(), arn.clone()], &writing).await;
        assert_eq!(
            (mixed.status, mixed.error_code().as_str()),
            (400, *expected_code),
            "case {case:?} (beside a live ARN): expected 400 {expected_code}, got {} {}",
            mixed.status,
            mixed.body
        );
        assert_eq!(
            stored(&router, &known).await,
            sorted(&known_tags),
            "case {case:?} (beside a live ARN): expected the live resource's tags to be untouched"
        );
    }
}

// ---------------------------------------------------------------------------
// A string that is not an Elastic Load Balancing resource ARN.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_write_to_a_string_that_is_not_an_elastic_load_balancing_arn_is_refused() {
    let (router, _engine) = emulator();
    let known = create_http_target_group(&router, "tagv-known-tg").await;

    let cases = [
        ("not an ARN at all", "not-an-arn".to_string(), false),
        ("an empty string", String::new(), false),
        (
            "an ARN of another service",
            format!("arn:aws:s3:::my-bucket-{}", account()),
            false,
        ),
        (
            "an Elastic Load Balancing ARN naming no known resource kind",
            absent_arn("widget/absent/0123456789abcdef"),
            false,
        ),
        (
            "an ARN missing its resource part",
            format!("arn:aws:elasticloadbalancing:{REGION}:{}", account()),
            false,
        ),
        (
            "a live ARN beside a malformed one",
            "also-not-an-arn".to_string(),
            true,
        ),
    ];

    let writing = owned(&[("env", "prod")]);
    for (case, value, beside_known) in &cases {
        let arns = if *beside_known {
            vec![known.clone(), value.clone()]
        } else {
            vec![value.clone()]
        };
        let refused = add_tags(&router, &arns, &writing).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationError"),
            "case {case:?}: expected 400 ValidationError, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            stored(&router, &known).await,
            Vec::new(),
            "case {case:?}: expected a refused write to store nothing"
        );
    }

    // A call carrying no tag at all asks for no write. `Tags` is modelled required with a
    // minimum length of one and awsQuery cannot carry an empty list, so the refusal comes
    // from the boundary validator before the handler runs; only the status and the fact
    // that nothing was stored are asserted, because the code of a boundary refusal is the
    // framework's rather than this operation's.
    let empty = add_tags_to(&router, &known, &[]).await;
    assert_eq!(
        empty.status, 400,
        "case {:?}: expected 400, got {} body {}",
        "no tag at all", empty.status, empty.body
    );
    assert_eq!(
        stored(&router, &known).await,
        Vec::new(),
        "case {:?}: expected nothing to be stored",
        "no tag at all"
    );
}

// ---------------------------------------------------------------------------
// One key named twice in one call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_tag_key_named_twice_in_one_call_is_refused() {
    let (router, _engine) = emulator();
    let tg = create_http_target_group(&router, "tagd-tg").await;

    let stored_first = owned(&[("env", "prod")]);
    let accepted = add_tags_to(&router, &tg, &stored_first).await;
    assert_eq!(
        accepted.status, 200,
        "case tag the target group: expected 200, got {} body {}",
        accepted.status, accepted.body
    );

    // Rows differ in where the repeat sits and whether the two values agree: either way
    // the request does not say what the key takes — and the refusal names the key that was
    // repeated, whichever position it sat in.
    let cases = [
        (
            "the same key twice with different values",
            vec![("env", "dev"), ("env", "test")],
            "env",
        ),
        (
            "the same key twice with the same value",
            vec![("env", "dev"), ("env", "dev")],
            "env",
        ),
        (
            "a repeat after an unrelated key",
            vec![("tier", "web"), ("env", "dev"), ("env", "test")],
            "env",
        ),
        (
            "a repeat of a key whose name is the longer of two",
            vec![("env", "dev"), ("environment", "dev"), ("environment", "test")],
            "environment",
        ),
    ];

    for (case, pairs, repeated) in cases {
        let refused = add_tags_to(&router, &tg, &owned(&pairs)).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "DuplicateTagKeys"),
            "case {case:?}: expected 400 DuplicateTagKeys, got {} {}",
            refused.status,
            refused.body
        );
        let expected = format!(
            "Tag key '{repeated}' has two values, but tag keys must have exactly one value"
        );
        let stated = unescaped(&message(&refused));
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
        assert_eq!(
            stored(&router, &tg).await,
            sorted(&stored_first),
            "case {case:?}: expected a refused write to leave the stored tags alone"
        );
    }
}

// ---------------------------------------------------------------------------
// How many tags one resource carries.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_write_past_the_per_resource_tag_limit_is_refused() {
    let (router, _engine) = emulator();

    // Elastic Load Balancing publishes a maximum number of tags per resource, which the
    // service crate states once as `state::TAGS_PER_RESOURCE`; the bound is tested below
    // it, at it and one past it.
    for count in [TAGS_PER_RESOURCE - 1, TAGS_PER_RESOURCE] {
        let tg = create_http_target_group(&router, &format!("tagl-tg-{count}")).await;
        let set = distinct_tags(count);
        let reply = add_tags_to(&router, &tg, &set).await;
        assert_eq!(
            reply.status, 200,
            "case {count} tags in one call: expected 200, got {} body {}",
            reply.status, reply.body
        );
        assert_eq!(
            stored(&router, &tg).await,
            sorted(&set),
            "case {count} tags in one call: expected every written tag to be stored"
        );
    }

    let past = TAGS_PER_RESOURCE + 1;
    let tg = create_http_target_group(&router, &format!("tagl-tg-{past}")).await;
    let refused = add_tags_to(&router, &tg, &distinct_tags(past)).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "TooManyTags"),
        "case {past} tags in one call: expected 400 TooManyTags, got {} {}",
        refused.status,
        refused.body
    );
    assert_eq!(
        stored(&router, &tg).await,
        Vec::new(),
        "case {past} tags in one call: expected a refused write to store nothing"
    );

    // A resource already at the limit: a key it carries can still be updated, because
    // that adds no tag, while one more key cannot be added.
    let full = create_http_target_group(&router, "tagl-full-tg").await;
    let mut set = distinct_tags(TAGS_PER_RESOURCE);
    let filled = add_tags_to(&router, &full, &set).await;
    assert_eq!(
        filled.status, 200,
        "case fill to the limit: expected 200, got {} body {}",
        filled.status, filled.body
    );

    let update = vec![(set[0].0.clone(), "updated".to_string())];
    let updated = add_tags_to(&router, &full, &update).await;
    assert_eq!(
        updated.status, 200,
        "case update a key at the limit: expected 200, got {} body {}",
        updated.status, updated.body
    );
    set[0].1 = "updated".to_string();
    assert_eq!(
        stored(&router, &full).await,
        sorted(&set),
        "case update a key at the limit: expected the key to take the new value"
    );

    let one_more = owned(&[("one-more", "key")]);
    let over = add_tags_to(&router, &full, &one_more).await;
    assert_eq!(
        (over.status, over.error_code().as_str()),
        (400, "TooManyTags"),
        "case one key past the limit: expected 400 TooManyTags, got {} {}",
        over.status,
        over.body
    );
    assert_eq!(
        stored(&router, &full).await,
        sorted(&set),
        "case one key past the limit: expected the stored tags to be untouched"
    );
}

// ---------------------------------------------------------------------------
// How many resources one call names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_write_names_at_most_twenty_resources() {
    let (router, _engine) = emulator();

    // "You can specify up to 20 resources in a single call" — the model's documentation
    // of the sibling tag read's `ResourceArns`, which is the same input shape this write
    // takes. The bound is tested below it, at it and one past it; every ARN below names a
    // resource that exists, so only the count can refuse.
    let mut arns = Vec::new();
    for index in 0..21usize {
        arns.push(create_http_target_group(&router, &format!("tagc-tg-{index}")).await);
    }

    for count in [1usize, 19, 20] {
        let write = owned(&[("batch", "size")]);
        let reply = add_tags(&router, &arns[..count], &write).await;
        assert_eq!(
            reply.status, 200,
            "case {count} resources: expected 200, got {} body {}",
            reply.status, reply.body
        );
        for arn in &arns[..count] {
            assert_eq!(
                stored(&router, arn).await,
                sorted(&write),
                "case {count} resources: expected the written tag on {arn}"
            );
        }
    }

    let past = add_tags(&router, &arns[..21], &owned(&[("past", "the bound")])).await;
    assert_eq!(
        (past.status, past.error_code().as_str()),
        (400, "ValidationError"),
        "case {:?}: expected 400 ValidationError, got {} {}",
        "one past the bound",
        past.status,
        past.body
    );
    assert_eq!(
        stored(&router, &arns[20]).await,
        Vec::new(),
        "case {:?}: expected a refused write to store nothing",
        "one past the bound"
    );
}

// ---------------------------------------------------------------------------
// One call tags every distinct resource it names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_call_tags_every_distinct_resource_it_names() {
    let (router, engine) = emulator();

    let lb = create_load_balancer(&router, "tagb-lb").await;
    let tg = create_http_target_group(&router, "tagb-tg").await;
    let listener = create_listener(&router, &lb, &tg, 80).await;
    let rule = add_rule(
        &router,
        "the batch rule",
        &listener,
        10,
        &rule_fields(&tg, "batch.example.com"),
    )
    .await;
    let trust_store = seed_trust_store(
        &engine,
        0,
        &TrustStoreSeed {
            name: "tagb-ts",
            certificates: 1,
            revoked: 0,
            settled: true,
            tags: &[],
        },
    );

    // Rows differ in how many resources one call names, in which kinds, in what order,
    // and in whether an ARN is repeated.
    let cases: Vec<BatchRow> = vec![
        ("one resource", vec![tg.clone()], vec![("batch", "one")]),
        (
            "every kind at once",
            vec![
                lb.clone(),
                tg.clone(),
                listener.clone(),
                rule.clone(),
                trust_store.clone(),
            ],
            vec![("batch", "all"), ("round", "two")],
        ),
        (
            "an ARN named twice is one resource",
            vec![tg.clone(), lb.clone(), tg.clone()],
            vec![("batch", "repeat")],
        ),
    ];

    let mut expected: Vec<(String, Vec<(String, String)>)> = vec![
        (lb.clone(), Vec::new()),
        (tg.clone(), Vec::new()),
        (listener.clone(), Vec::new()),
        (rule.clone(), Vec::new()),
        (trust_store.clone(), Vec::new()),
    ];

    for (case, arns, pairs) in cases {
        let write = owned(&pairs);
        let reply = add_tags(&router, &arns, &write).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );

        // The same merge every row of the table above asserts, tracked here across the
        // resources this row named.
        for (arn, tags) in expected.iter_mut() {
            if arns.contains(arn) {
                for (key, value) in &write {
                    tags.retain(|(stored_key, _)| stored_key != key);
                    tags.push((key.clone(), value.clone()));
                }
            }
        }

        for (arn, tags) in &expected {
            assert_eq!(
                stored(&router, arn).await,
                sorted(tags),
                "case {case:?}: expected the tags accumulated on {arn}"
            );
        }
    }
}
