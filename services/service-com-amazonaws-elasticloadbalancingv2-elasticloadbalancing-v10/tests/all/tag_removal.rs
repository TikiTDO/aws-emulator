// SPDX-License-Identifier: CC0-1.0

//! `RemoveTags` over the wire: the keys a removal withdraws are the keys a later
//! `DescribeTags` no longer reports, and a removal naming a resource the store does not
//! hold is refused with the `NotFound` error modelled for the kind the ARN declares.
//!
//! Every request goes through the router exactly as a client's would — awsQuery form body
//! in, `<RemoveTagsResponse>` XML out — so these tests exercise the wire protocol as well
//! as the handler, and every assertion about a removal is made on what a later READ
//! reports.
//!
//! The rule under test is not one call: a tag removal resolves each `ResourceArn` by the
//! resource kind the ARN itself declares, withdraws exactly the named keys from the
//! resource the store holds for it and leaves every other key on it, and refuses a call
//! naming a resource the store does not hold — whatever the reason it is absent — with
//! that kind's `NotFound` error, having withdrawn nothing. So the suites below span a
//! table of stored sets and key selections, every taggable resource kind, every refusal
//! and the published bounds; the recorded call (one target group, one key withdrawn, then
//! a bogus target-group ARN) is one row of it.
//!
//! The resource creators, the awsQuery helpers and the tag-set table are the ones the
//! sibling suites already carry (`provisioning_loop`, `listener_rules`, `resource_tags`,
//! `tag_writes`), so a removal is driven against resources created and tagged exactly as
//! every other suite creates and tags them. A trust store is SEEDED through the shared
//! engine, as those suites seed one: a seeded record needs no CA certificates bundle
//! staged in Amazon S3, which is what the create reads one from.

use crate::listener_rules::{
    add_rule, create_listener, create_load_balancer, forward_to, host_header,
};
use crate::provisioning_loop::{
    account, action, call, create_http_target_group, emulator, members, p, seed_trust_store, Reply,
    TrustStoreSeed, REGION,
};
use crate::resource_tags::{absent_arn, describe_tags, reported, sorted, tag_sets};
use crate::target_group_attributes_and_modification::unescaped;
use crate::tag_writes::{add_tags, add_tags_to, distinct_tags, message, owned, stored};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::TAGS_PER_RESOURCE;
use smithy_server_runtime::MultiServiceRouter;

/// One row of the removal table: the case, the tags stored first, the keys the removal
/// names, and the tags the resource must then carry.
type RemovalRow = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    Vec<&'static str>,
    Vec<(&'static str, &'static str)>,
);

/// The request fields of a rule that forwards one host to `tg_arn` — a condition and an
/// action, both of which `CreateRule` requires.
fn rule_fields(tg_arn: &str, host: &str) -> Vec<(String, String)> {
    let mut fields = host_header(host);
    fields.extend(forward_to(tg_arn));
    fields
}

/// `RemoveTags` over `arns`, withdrawing `keys`.
pub(crate) async fn remove_tags(
    router: &MultiServiceRouter,
    arns: &[String],
    keys: &[String],
) -> Reply {
    let mut form = action("RemoveTags");
    for (index, arn) in arns.iter().enumerate() {
        form.push(p(&format!("ResourceArns.member.{}", index + 1), arn));
    }
    for (index, key) in keys.iter().enumerate() {
        form.push(p(&format!("TagKeys.member.{}", index + 1), key));
    }
    call(router, &form).await
}

/// `RemoveTags` over one ARN, withdrawing `keys`.
pub(crate) async fn remove_tags_from(
    router: &MultiServiceRouter,
    arn: &String,
    keys: &[String],
) -> Reply {
    remove_tags(router, std::slice::from_ref(arn), keys).await
}

/// Owned keys from literals.
fn owned_keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| key.to_string()).collect()
}

// ---------------------------------------------------------------------------
// What a removal withdraws is what a later read no longer reports, and what it
// leaves is what the read still reports — for every taggable kind.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_removal_withdraws_the_named_keys_and_leaves_every_other_key() {
    let (router, engine) = emulator();

    // The load balancer and target group the per-row listeners and rules hang off; their
    // own tags are never touched, so they also witness that a removal on one resource
    // leaves the others alone.
    let host_lb = create_load_balancer(&router, "tagr-host-lb").await;
    let host_tg = create_http_target_group(&router, "tagr-host-tg").await;
    let host_tags = owned(&[("host", "untouched")]);
    assert_eq!(
        add_tags_to(&router, &host_tg, &host_tags).await.status,
        200,
        "case tagging the host target group: expected 200"
    );

    // The rows differ along what the rule generalizes over: how many keys are stored, how
    // many of them the call names, whether a named key is stored at all, whether the call
    // names a key twice, and which character classes and lengths the keys use. The
    // character classes and the maximum key length are the model's own for the `TagKey`
    // shape (`smithy_query(query="shape",shape="TagKey")`: length 1..128, matching
    // `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`).
    let max_key: &'static str = Box::leak("k".repeat(128).into_boxed_str());
    let rows: Vec<RemovalRow> = vec![
        (
            "one of three keys",
            vec![("env", "prod"), ("owner", "platform"), ("tier", "web")],
            vec!["env"],
            vec![("owner", "platform"), ("tier", "web")],
        ),
        (
            "two of three keys",
            vec![("env", "prod"), ("owner", "platform"), ("tier", "web")],
            vec!["env", "tier"],
            vec![("owner", "platform")],
        ),
        (
            "every stored key",
            vec![("env", "prod"), ("owner", "platform")],
            vec!["env", "owner"],
            vec![],
        ),
        (
            "the only stored key",
            vec![("env", "prod")],
            vec!["env"],
            vec![],
        ),
        (
            "a key the resource does not carry",
            vec![("env", "prod")],
            vec!["absent"],
            vec![("env", "prod")],
        ),
        (
            "one stored key beside one it does not carry",
            vec![("env", "prod"), ("owner", "platform")],
            vec!["env", "absent"],
            vec![("owner", "platform")],
        ),
        (
            "the same key named twice in one call",
            vec![("env", "prod"), ("owner", "platform")],
            vec!["env", "env"],
            vec![("owner", "platform")],
        ),
        (
            "a key whose stored value is empty",
            vec![("marker", ""), ("env", "prod")],
            vec!["marker"],
            vec![("env", "prod")],
        ),
        (
            "a key using every allowed punctuation class",
            vec![("ns:key_1.a/b=c+d-e@f", "v:1"), ("env", "prod")],
            vec!["ns:key_1.a/b=c+d-e@f"],
            vec![("env", "prod")],
        ),
        (
            "a key of unicode letters and separators",
            vec![("propriétaire", "équipe réseau"), ("env", "prod")],
            vec!["propriétaire"],
            vec![("env", "prod")],
        ),
        (
            "a key at the published maximum length",
            vec![(max_key, "long"), ("env", "prod")],
            vec![max_key],
            vec![("env", "prod")],
        ),
    ];

    for (row, (case, stored_first, removing, expected)) in rows.into_iter().enumerate() {
        // Several kinds per row, so what is asserted is the rule of a tag removal rather
        // than anything about one resource kind.
        let lb = create_load_balancer(&router, &format!("tagr-lb-{row}")).await;
        let tg = create_http_target_group(&router, &format!("tagr-tg-{row}")).await;
        let listener = create_listener(&router, &host_lb, &host_tg, 9100 + row as i32).await;
        let rule = add_rule(
            &router,
            case,
            &listener,
            10,
            &rule_fields(&host_tg, &format!("removal-{row}.example.com")),
        )
        .await;
        let ts_name = format!("tagr-ts-{row}");
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
            ("listener rule", &rule),
            ("trust store", &trust_store),
        ] {
            let case = format!("{case} on a {kind}");
            let write = owned(&stored_first);
            assert_eq!(
                add_tags_to(&router, arn, &write).await.status,
                200,
                "case {case:?}: expected the setup write to be accepted"
            );

            let reply = remove_tags_from(&router, arn, &owned_keys(&removing)).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                reply.status, reply.body
            );

            let want = sorted(&owned(&expected));
            let got = stored(&router, arn).await;
            assert_eq!(
                got, want,
                "case {case:?}: expected the resource to carry {want:?}, got {got:?}"
            );

            // Removing the same keys again asks for the same end state, which already
            // holds: the repeat is accepted and changes nothing.
            let again = remove_tags_from(&router, arn, &owned_keys(&removing)).await;
            assert_eq!(
                again.status, 200,
                "case {case:?} (repeated): expected 200, got {} body {}",
                again.status, again.body
            );
            let after = stored(&router, arn).await;
            assert_eq!(
                after, want,
                "case {case:?} (repeated): expected the resource to still carry {want:?}, got {after:?}"
            );
        }
    }

    // Every removal above left the resource none of them named exactly as it was.
    let host = stored(&router, &host_tg).await;
    assert_eq!(
        host,
        sorted(&host_tags),
        "case {:?}: expected the untouched target group to keep its tags, got {host:?}",
        "a resource no removal named"
    );
}

// ---------------------------------------------------------------------------
// A resource whose last key was withdrawn and one that was never tagged report
// the same thing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_fully_cleared_resource_reports_exactly_what_a_never_tagged_one_reports() {
    let (router, _engine) = emulator();

    // The rows differ in the set that was stored before it was cleared, so the
    // convergence is a property of clearing rather than of one tag set. The sets are the
    // table the sibling tag suites share, whose empty row is the never-tagged case
    // asserted through `never` below.
    for (row, (case, set)) in tag_sets().into_iter().enumerate() {
        if set.is_empty() {
            continue;
        }
        let never = create_load_balancer(&router, &format!("tagc-never-{row}")).await;
        let cleared = create_load_balancer(&router, &format!("tagc-cleared-{row}")).await;

        assert_eq!(
            add_tags_to(&router, &cleared, &set).await.status,
            200,
            "case {case:?}: expected the setup write to be accepted"
        );
        let keys: Vec<String> = set.iter().map(|(key, _)| key.clone()).collect();
        let reply = remove_tags_from(&router, &cleared, &keys).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );

        // One read over both resources: the two descriptions must differ only in the ARN
        // they carry, so a cleared resource is indistinguishable from a never-tagged one.
        let read = describe_tags(&router, &[never.clone(), cleared.clone()]).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from the read, got {} body {}",
            read.status, read.body
        );
        assert_eq!(
            reported(&read),
            vec![(never.clone(), Vec::new()), (cleared.clone(), Vec::new())],
            "case {case:?}: expected both resources to report no tag, got {}",
            read.body
        );
        let descriptions = members(&read.body, "TagDescriptions");
        assert_eq!(
            descriptions.len(),
            2,
            "case {case:?}: expected two descriptions, got {}",
            read.body
        );
        let without_arn = |description: &String, arn: &String| description.replace(arn.as_str(), "");
        assert_eq!(
            without_arn(&descriptions[1], &cleared),
            without_arn(&descriptions[0], &never),
            "case {case:?}: expected the cleared resource's description to match the never-tagged one, got {}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// A resource the store does not hold: the NotFound error of the kind the ARN names,
// and nothing withdrawn.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_removal_naming_a_resource_the_store_does_not_hold_is_refused_with_the_error_of_its_kind()
{
    let (router, _engine) = emulator();

    let known = create_http_target_group(&router, "tagx-known-tg").await;
    let known_tags = owned(&[("env", "prod"), ("owner", "platform")]);
    assert_eq!(
        add_tags_to(&router, &known, &known_tags).await.status,
        200,
        "case tagging the known target group: expected 200"
    );

    // A load balancer that was created and then DELETED: the row no "it never existed"
    // ARN can stand in for.
    let deleted = create_load_balancer(&router, "tagx-deleted-lb").await;
    assert_eq!(
        add_tags_to(&router, &deleted, &owned(&[("env", "prod")]))
            .await
            .status,
        200,
        "case tagging the load balancer before deleting it: expected 200"
    );
    let mut form = action("DeleteLoadBalancer");
    form.push(p("LoadBalancerArn", &deleted));
    let gone = call(&router, &form).await;
    assert_eq!(
        gone.status, 200,
        "case deleting the load balancer: expected 200, got {} body {}",
        gone.status, gone.body
    );

    // The ARN forms AWS publishes for this service's resources, one per kind, with the
    // awsQuery error code the model gives that kind's NotFound shape
    // (`smithy_query(query="operation",operation="RemoveTags")`).
    let cases = [
        ("a deleted load balancer", deleted.clone(), "LoadBalancerNotFound"),
        (
            "an application load balancer that never existed",
            absent_arn("loadbalancer/app/absent/0123456789abcdef"),
            "LoadBalancerNotFound",
        ),
        (
            "a network load balancer that never existed",
            absent_arn("loadbalancer/net/absent/0123456789abcdef"),
            "LoadBalancerNotFound",
        ),
        (
            "a gateway load balancer that never existed",
            absent_arn("loadbalancer/gwy/absent/0123456789abcdef"),
            "LoadBalancerNotFound",
        ),
        (
            "a target group that never existed",
            absent_arn("targetgroup/absent/0123456789abcdef"),
            "TargetGroupNotFound",
        ),
        (
            "a listener that never existed",
            absent_arn("listener/app/absent/0123456789abcdef/fedcba9876543210"),
            "ListenerNotFound",
        ),
        (
            "a listener rule that never existed",
            absent_arn(
                "listener-rule/app/absent/0123456789abcdef/fedcba9876543210/00112233445566aa",
            ),
            "RuleNotFound",
        ),
        (
            "a trust store that never existed",
            absent_arn("truststore/absent/0123456789abcdef"),
            "TrustStoreNotFound",
        ),
    ];

    let keys = owned_keys(&["env"]);
    for (case, arn, expected_code) in &cases {
        let refused = remove_tags_from(&router, arn, &keys).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, *expected_code),
            "case {case:?}: expected 400 {expected_code}, got {} {}",
            refused.status,
            refused.body
        );
        // The framework's XML codec escapes the quotes a message carries, so the VALUE
        // is what is compared — through the crate's one reader for that encoding.
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

        // A live resource named beside the absent one keeps every key: the removal is
        // all-or-nothing across the resources one call names.
        let mixed = remove_tags(&router, &[known.clone(), arn.clone()], &keys).await;
        assert_eq!(
            (mixed.status, mixed.error_code().as_str()),
            (400, *expected_code),
            "case {case:?} (beside a known ARN): expected 400 {expected_code}, got {} {}",
            mixed.status,
            mixed.body
        );
        let still = stored(&router, &known).await;
        assert_eq!(
            still,
            sorted(&known_tags),
            "case {case:?}: expected a refused removal to withdraw nothing, got {still:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A string that is not an Elastic Load Balancing resource ARN.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_removal_naming_a_string_that_is_not_an_elastic_load_balancing_arn_is_refused() {
    let (router, _engine) = emulator();

    let known = create_http_target_group(&router, "tagy-known-tg").await;
    let known_tags = owned(&[("env", "prod")]);
    assert_eq!(
        add_tags_to(&router, &known, &known_tags).await.status,
        200,
        "case tagging the known target group: expected 200"
    );

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
        ("a known ARN beside a malformed one", "also-not-an-arn".to_string(), true),
    ];

    let keys = owned_keys(&["env"]);
    for (case, value, beside_known) in &cases {
        let arns = if *beside_known {
            vec![known.clone(), value.clone()]
        } else {
            vec![value.clone()]
        };
        let refused = remove_tags(&router, &arns, &keys).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationError"),
            "case {case:?}: expected 400 ValidationError, got {} {}",
            refused.status,
            refused.body
        );
        let still = stored(&router, &known).await;
        assert_eq!(
            still,
            sorted(&known_tags),
            "case {case:?}: expected a refused removal to withdraw nothing, got {still:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The bounds on one call: how many resources it may name, and how many keys.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_call_removes_tags_from_between_one_and_twenty_resources() {
    let (router, _engine) = emulator();

    // "You can specify up to 20 resources in a single call" — the model's documentation of
    // the tag read's `DescribeTagsInput$ResourceArns`, which is the same `ResourceArns`
    // shape this call takes. The bound is tested below it, at it and one past it; every
    // ARN below names a resource that exists, so only the count can refuse.
    let batch = owned(&[("batch", "size"), ("keep", "me")]);
    let mut arns = Vec::new();
    for index in 0..21usize {
        let arn = create_http_target_group(&router, &format!("tagz-tg-{index}")).await;
        assert_eq!(
            add_tags_to(&router, &arn, &batch).await.status,
            200,
            "case tagging resource {index}: expected 200"
        );
        arns.push(arn);
    }

    let keys = owned_keys(&["batch"]);
    for count in [1usize, 19, 20] {
        let reply = remove_tags(&router, &arns[..count], &keys).await;
        assert_eq!(
            reply.status, 200,
            "case {count} resources: expected 200, got {} body {}",
            reply.status, reply.body
        );
        for arn in &arns[..count] {
            let got = stored(&router, arn).await;
            assert_eq!(
                got,
                vec![("keep".to_string(), "me".to_string())],
                "case {count} resources: expected only the unnamed key to survive on {arn}, got {got:?}"
            );
        }
    }

    // One past the bound: refused, and every resource it named keeps the key it still
    // carries.
    let restore = owned(&[("batch", "size")]);
    assert_eq!(
        add_tags(&router, &arns[..20], &restore).await.status,
        200,
        "case restoring the removed key: expected 200"
    );
    let past = remove_tags(&router, &arns[..21], &keys).await;
    assert_eq!(
        (past.status, past.error_code().as_str()),
        (400, "ValidationError"),
        "case {:?}: expected 400 ValidationError, got {} {}",
        "one past the resource bound",
        past.status,
        past.body
    );
    for arn in &arns[..21] {
        let got = stored(&router, arn).await;
        assert_eq!(
            got,
            sorted(&batch),
            "case {:?}: expected nothing withdrawn from {arn}, got {got:?}",
            "one past the resource bound"
        );
    }

    // Naming no resource at all, and naming no key at all, are refused rather than
    // accepted as a removal of nothing. Both lists are modelled required and awsQuery
    // cannot carry an empty one, so these refusals come from the boundary validator
    // before the handler runs; only the status is asserted, because the code of a
    // boundary refusal is the framework's, not this operation's.
    let no_resource = remove_tags(&router, &[], &keys).await;
    assert_eq!(
        no_resource.status, 400,
        "case {:?}: expected 400, got {} body {}",
        "no resource at all", no_resource.status, no_resource.body
    );
    let no_key = remove_tags_from(&router, &arns[0], &[]).await;
    assert_eq!(
        no_key.status, 400,
        "case {:?}: expected 400, got {} body {}",
        "no tag key at all", no_key.status, no_key.body
    );
    let got = stored(&router, &arns[0]).await;
    assert_eq!(
        got,
        sorted(&batch),
        "case {:?}: expected nothing withdrawn, got {got:?}",
        "no tag key at all"
    );
}

#[tokio::test]
async fn one_call_names_at_most_as_many_keys_as_a_resource_can_carry() {
    let (router, _engine) = emulator();

    // Elastic Load Balancing publishes a maximum number of tags per resource, which the
    // service crate states once as `state::TAGS_PER_RESOURCE`: a call naming more keys
    // than that names keys no resource of this service can carry. The bound is tested
    // below it, at it and one past it.
    let tg = create_http_target_group(&router, "tagk-tg").await;
    let set = distinct_tags(TAGS_PER_RESOURCE);
    assert_eq!(
        add_tags_to(&router, &tg, &set).await.status,
        200,
        "case filling the resource: expected 200"
    );

    let all_keys: Vec<String> = set.iter().map(|(key, _)| key.clone()).collect();
    for count in [1usize, TAGS_PER_RESOURCE - 1, TAGS_PER_RESOURCE] {
        // Each accepted removal is asserted on a freshly filled resource, so the count is
        // the only thing that differs between the rows.
        let arn = create_http_target_group(&router, &format!("tagk-tg-{count}")).await;
        assert_eq!(
            add_tags_to(&router, &arn, &set).await.status,
            200,
            "case {count} keys: expected the setup write to be accepted"
        );
        let reply = remove_tags_from(&router, &arn, &all_keys[..count]).await;
        assert_eq!(
            reply.status, 200,
            "case {count} keys: expected 200, got {} body {}",
            reply.status, reply.body
        );
        let got = stored(&router, &arn).await;
        assert_eq!(
            got,
            sorted(&set[count..]),
            "case {count} keys: expected the unnamed keys to survive, got {got:?}"
        );
    }

    let mut past = all_keys.clone();
    past.push("one-more-key".to_string());
    let refused = remove_tags_from(&router, &tg, &past).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "TooManyTags"),
        "case {} keys: expected 400 TooManyTags, got {} {}",
        past.len(),
        refused.status,
        refused.body
    );
    let got = stored(&router, &tg).await;
    assert_eq!(
        got,
        sorted(&set),
        "case {} keys: expected nothing withdrawn, got {got:?}",
        past.len()
    );
}
