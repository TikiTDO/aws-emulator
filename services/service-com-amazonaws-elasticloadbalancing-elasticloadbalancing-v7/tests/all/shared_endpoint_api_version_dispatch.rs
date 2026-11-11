// SPDX-License-Identifier: CC0-1.0

//! What a RESOLVED action/version pair reaches on the shared Elastic Load Balancing
//! endpoint: the ELBv2 model's own validation, its own handlers' state, and its scaffolds.
//!
//! The routing rule itself — which of the two models an awsQuery envelope's `Version`
//! names, and the `400 InvalidAction` for a pair neither resolves — is pinned by
//! `core/router/tests/all/elb_classic_and_v2_signing_name_routing.rs`, which mounts both
//! services. This suite pins the consequence that made the
//! defect critical: before it, EVERY `2015-12-01` envelope was answered `InvalidAction`,
//! which tells an SDK the action does not exist. So for a pair that resolves, what comes
//! back must be the ELBv2 model's OWN answer, whatever that answer is:
//!
//! * an operation it implements serves its document, in its own `@xmlNamespace` and in no
//!   other, and persists what a later read reports;
//! * a value outside a member's modelled bound is refused as a validation failure, not as
//!   an unknown action;
//! * an operation it declares but has not implemented answers its scaffold's `501
//!   NotImplemented` — reached, and still not `InvalidAction`.
//!
//! Every request goes through this crate's service alone, signed for the shared
//! `elasticloadbalancing` name at `Version=2015-12-01`: the production path for an AWS SDK
//! ELBv2 client.

use crate::wire::{call_at_version, emulator, find, kid_text, param, parse, Element, API_VERSION};

/// The API version the ELBv2 model declares (`smithy_query(query="service_info")` on its
/// model: `version: 2015-12-01`).
const V2_VERSION: &str = "2015-12-01";

/// The `@xmlNamespace` of the ELBv2 model's document, from the same query.
const V2_XMLNS: &str = "http://elasticloadbalancing.amazonaws.com/doc/2015-12-01/";

/// The `@xmlNamespace` of THIS model's document — what an answer served out of the ELBv2
/// model must never carry.
const CLASSIC_XMLNS: &str = "http://elasticloadbalancing.amazonaws.com/doc/2012-06-01/";

/// The region every request below is signed for.
const REGION: &str = "us-west-2";

/// The VPC every target group below belongs to.
const VPC: &str = "vpc-0abc1234";

/// An ELBv2 write and the read that reports it, both through the shared endpoint: a
/// target registered with a target group is reported by that group's health read.
///
/// The rows differ in the KIND of target the registration names — an instance id, an
/// instance id with a port override, and an IP target — so the pair resolving is checked
/// against several shapes of input rather than one, and each case asserts what the later
/// read reports rather than what the write answered.
#[tokio::test]
async fn targets_registered_through_the_shared_endpoint_are_reported_by_the_health_read() {
    let router = emulator();

    // (case, target group name, target type, the target's Id, its Port override)
    let cases: [(&str, &str, &str, &str, Option<&str>); 3] = [
        (
            "an instance target on the group's own port",
            "shared-health-instance",
            "instance",
            "i-0123456789abcdef0",
            None,
        ),
        (
            "an instance target with a port override",
            "shared-health-override",
            "instance",
            "i-00fedcba987654321",
            Some("8081"),
        ),
        (
            "an ip target",
            "shared-health-ip",
            "ip",
            "10.0.1.17",
            Some("443"),
        ),
    ];

    for (case, name, target_type, id, port) in cases {
        let created = v2_call(
            &router,
            "CreateTargetGroup",
            &[
                param("Name", name),
                param("Protocol", "HTTP"),
                param("Port", "80"),
                param("VpcId", VPC),
                param("TargetType", target_type),
            ],
        )
        .await;
        assert_eq!(
            created.0, 200,
            "case {case:?}: CreateTargetGroup must be served, got {} with {}",
            created.0, created.1
        );
        served_by_the_elbv2_model(case, &created.1);
        let arn = first_text(&created.1, "TargetGroupArn")
            .unwrap_or_else(|| panic!("case {case:?}: no TargetGroupArn in {}", created.1));

        let mut registration = vec![
            param("TargetGroupArn", &arn),
            param("Targets.member.1.Id", id),
        ];
        if let Some(port) = port {
            registration.push(param("Targets.member.1.Port", port));
        }
        let registered = v2_call(&router, "RegisterTargets", &registration).await;
        assert_eq!(
            registered.0, 200,
            "case {case:?}: RegisterTargets must be served, got {} with {}",
            registered.0, registered.1
        );
        served_by_the_elbv2_model(case, &registered.1);

        // What the health read reports is the registration the write persisted.
        let health = v2_call(
            &router,
            "DescribeTargetHealth",
            &[param("TargetGroupArn", &arn)],
        )
        .await;
        assert_eq!(
            health.0, 200,
            "case {case:?}: DescribeTargetHealth must be served, got {} with {}",
            health.0, health.1
        );
        served_by_the_elbv2_model(case, &health.1);
        let document = parse(&health.1);
        let described = find(&document, "Target")
            .unwrap_or_else(|| panic!("case {case:?}: no <Target> in {}", health.1));
        assert_eq!(
            kid_text(described, "Id").as_deref(),
            Some(id),
            "case {case:?}: expected the registered target {id}, got {}",
            health.1
        );
        assert_eq!(
            kid_text(described, "Port").as_deref(),
            Some(port.unwrap_or("80")),
            "case {case:?}: expected the target's port (the override, or the group's own \
             port when it states none), got {}",
            health.1
        );
        // A health read answers the modelled `TargetHealth` for it, not an empty list.
        assert!(
            find(&document, "TargetHealth").is_some(),
            "case {case:?}: expected a <TargetHealth> for the registered target, got {}",
            health.1
        );
    }

    // A health read for a target group nothing holds is the operation's OWN modelled
    // refusal — `TargetGroupNotFound` (`smithy_query(query="operation",
    // operation="DescribeTargetHealth")`: awsQuery code `TargetGroupNotFound`, HTTP 400) —
    // and not a routing refusal.
    let missing = format!(
        "arn:aws:elasticloadbalancing:{REGION}:000000000000:targetgroup/not-here/0123456789abcdef"
    );
    let answer = v2_call(
        &router,
        "DescribeTargetHealth",
        &[param("TargetGroupArn", &missing)],
    )
    .await;
    assert_eq!(
        (answer.0, error_code(&answer.1).as_str()),
        (400, "TargetGroupNotFound"),
        "a health read for an absent target group must be the modelled refusal, got {} \
         with {}",
        answer.0,
        answer.1
    );
}

/// A value outside a member's modelled bound, asked for at the ELBv2 version, is refused
/// as a VALIDATION failure — never `InvalidAction`, which would say the action does not
/// exist — and a value on either published bound is served.
///
/// The members and their bounds are `CreateTargetGroup`'s own, from
/// `smithy_query(query="operation",operation="CreateTargetGroup")`: `Port` `@range 1..65535`,
/// `HealthCheckIntervalSeconds` `5..300`, `HealthCheckTimeoutSeconds` `2..120`,
/// `HealthyThresholdCount` and `UnhealthyThresholdCount` `2..10`. Each is exercised on BOTH
/// published bounds and one step past each of them, so the row the issue recorded
/// (`UnhealthyThresholdCount=11`) is one cell of the table rather than the whole test.
#[tokio::test]
async fn a_value_outside_a_modelled_bound_is_a_validation_failure_not_an_unknown_action() {
    let router = emulator();

    // (member, min, max) — the modelled `@range` of each bounded `CreateTargetGroup`
    // member. `Port` is the group's own port; the rest are its health check's.
    let bounded: [(&str, i64, i64); 5] = [
        ("Port", 1, 65535),
        ("HealthCheckIntervalSeconds", 5, 300),
        ("HealthCheckTimeoutSeconds", 2, 120),
        ("HealthyThresholdCount", 2, 10),
        ("UnhealthyThresholdCount", 2, 10),
    ];

    for (index, (member, min, max)) in bounded.into_iter().enumerate() {
        for (position, value, accepted) in [
            ("the published minimum", min, true),
            ("the published maximum", max, true),
            ("one below the minimum", min - 1, false),
            ("one above the maximum", max + 1, false),
        ] {
            let case = format!("{member} at {position} ({value})");
            // A distinct name per cell: a repeat of one name with other settings is the
            // handler's own duplicate refusal, which is not what this table is about.
            let name = format!("bound-tg-{index}-{value}");
            let mut params = vec![
                param("Name", &name),
                param("Protocol", "HTTP"),
                param("VpcId", VPC),
            ];
            // `Port` is itself one of the bounded members, so it is only defaulted for
            // the rows that are not about it.
            if member != "Port" {
                params.push(param("Port", "80"));
            }
            params.push(param(member, &value.to_string()));

            let (status, body) = v2_call(&router, "CreateTargetGroup", &params).await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a value ON the modelled bound must be served, got \
                     {status} with {body}"
                );
                served_by_the_elbv2_model(&case, &body);
                // What was accepted is what the stored record reports.
                assert!(
                    body.contains(&format!("<{member}>{value}</{member}>")),
                    "case {case:?}: expected <{member}>{value}</{member}> in the stored \
                     target group, got {body}"
                );
            } else {
                let code = error_code(&body);
                assert_eq!(
                    (status, code.as_str()),
                    (400, "ValidationError"),
                    "case {case:?}: a value outside the modelled bound must be refused as \
                     a validation failure, got {status} with {body}"
                );
                // And nothing was stored for it.
                let described = v2_call(
                    &router,
                    "DescribeTargetGroups",
                    &[param("Names.member.1", &name)],
                )
                .await;
                assert_ne!(
                    described.0, 200,
                    "case {case:?}: a refused create must store nothing, yet {name} reads \
                     back as {}",
                    described.1
                );
            }
        }
    }
}

/// An ELBv2 operation the model declares but this emulator has not implemented answers the
/// scaffold's `501 NotImplemented` — which is what proves the WHOLE 2015-12-01 action set
/// resolves, not only the operations with handlers.
///
/// `501 NotImplemented` says "this emulator has not written it yet"; `400 InvalidAction`
/// says "this API has no such action", which is false for every row below. Each is an
/// operation whose handler still carries the scaffold header in
/// `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/`
/// and has its own open stub issue; when one is implemented its row moves to a served
/// answer, and the assertion names both outcomes so it fails only on a refusal that claims
/// the action does not exist.
#[tokio::test]
async fn a_declared_but_unimplemented_elbv2_action_is_reached_not_refused_as_unknown() {
    let router = emulator();

    // Operations spanning the model's shapes of answer: a list read with no required
    // input, a read of a named resource, and a write addressing a named resource.
    let scaffolded: [(&str, &[(&str, &str)]); 4] = [
        ("DescribeSSLPolicies", &[]),
        ("DescribeAccountLimits", &[]),
        (
            // `DescribeTrustStores` was this row until it was implemented, and
            // `DescribeTrustStoreAssociations` stood in for it until its own stub issue was
            // closed too; `DescribeTrustStoreRevocations`, which reads a named trust
            // store's revocation entries, is the stub that stands in for the same shape of
            // answer.
            "DescribeTrustStoreRevocations",
            &[(
                "TrustStoreArn",
                "arn:aws:elasticloadbalancing:us-west-2:000000000000:truststore/t/0",
            )],
        ),
        (
            // `AddTags` was this row until it was implemented, `RemoveTags` stood in for
            // it until it was implemented too, `ModifyRule` stood in for them until its
            // own stub issue was closed, and `DeleteTrustStore` stood in after that until
            // the trust-store control plane was implemented; `AddTrustStoreRevocations`, a
            // write addressing one named resource, is the stub that stands in for the same
            // shape of answer.
            "AddTrustStoreRevocations",
            &[(
                "TrustStoreArn",
                "arn:aws:elasticloadbalancing:us-west-2:000000000000:truststore/t/0",
            )],
        ),
    ];

    for (action, fields) in scaffolded {
        let params: Vec<(String, String)> = fields
            .iter()
            .map(|(name, value)| param(name, value))
            .collect();
        let (status, body) = v2_call(&router, action, &params).await;
        let code = error_code(&body);
        assert_ne!(
            code, "InvalidAction",
            "case {action:?}: the ELBv2 model declares this action, so it must be reached \
             rather than refused as unknown, got {status} with {body}"
        );
        assert!(
            (status == 501 && code == "NotImplemented") || status == 200,
            "case {action:?}: expected the scaffold's 501 NotImplemented (or a served \
             answer once it is implemented), got {status} with {body}"
        );
    }
}

/// A `Version` that names no Elastic Load Balancing model is refused rather than guessed —
/// whatever shape the client got wrong, and whatever the action.
///
/// `CreateLoadBalancer` is an action BOTH models declare, so nothing but the version can
/// decide; the rows span an empty field, the other API's version mis-typed or with
/// trailing text, a non-date, a future date and a value carrying XML metacharacters. Each
/// answer must be a well-formed `<ErrorResponse>` naming the pair, and nothing may be
/// created under either model.
#[tokio::test]
async fn a_version_naming_no_model_is_refused_rather_than_guessed() {
    let router = emulator();
    let cases = [
        ("an empty Version field", ""),
        ("the ELBv2 version with trailing text", "2015-12-01x"),
        ("the ELBv2 version mis-typed", "2015-21-01"),
        ("the Classic version with a trailing space", "2012-06-01 "),
        ("a version from the future", "2099-01-01"),
        ("a Version that is not a date", "latest"),
        // The refusal echoes the value the caller sent, so the envelope has to carry it
        // escaped and stay parseable rather than letting the request shape the document.
        ("a Version carrying XML metacharacters", "2015-12-01<&\">"),
    ];

    for (case, version) in cases {
        let answer = call_at_version(
            &router,
            REGION,
            "CreateLoadBalancer",
            Some(version),
            &[
                param("Name", "shared-door-guess"),
                param("Subnets.member.1", "subnet-0abc1234"),
            ],
        )
        .await;
        let (status, body) = (answer.status, answer.body.clone());
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidAction"),
            "case {case:?}: a version no model declares resolves nothing, got {status} \
             with {body}"
        );
        // Well-formed XML the caller can read: `parse` panics on anything else.
        let document = parse(&body);
        assert_eq!(
            document.root_element().name,
            "ErrorResponse",
            "case {case:?}: expected an <ErrorResponse> envelope, got {body}"
        );
        assert!(
            body.contains("CreateLoadBalancer"),
            "case {case:?}: the refusal must name the action it could not resolve, got \
             {body}"
        );
        if !version.trim().is_empty() {
            let escaped = aws_smithy_schema_server::xml_escape(version);
            assert!(
                body.contains(&escaped),
                "case {case:?}: the refusal must name the version it could not resolve, \
                 as {escaped:?}, got {body}"
            );
        }

        // Nothing was created under EITHER model.
        for (model, model_version) in [("ELBv2", V2_VERSION), ("Classic", API_VERSION)] {
            let described =
                call_at_version(&router, REGION, "DescribeLoadBalancers", Some(model_version), &[])
                    .await;
            assert!(
                !described.body.contains("shared-door-guess"),
                "case {case:?}: a refused pair must create nothing, yet the {model} model \
                 reports it: {}",
                described.body
            );
        }
    }
}

/// POST one ELBv2 action at `Version=2015-12-01` on the SHARED signing name, and answer
/// its status and body.
async fn v2_call(
    router: &smithy_server_runtime::MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let answer = call_at_version(router, REGION, action, Some(V2_VERSION), params).await;
    (answer.status, answer.body)
}

/// Assert an answer is the ELBv2 model's own document: its `@xmlNamespace` and NOT this
/// model's, so a row fails if the wrong half of the shared endpoint served it.
fn served_by_the_elbv2_model(case: &str, body: &str) {
    assert!(
        body.contains(V2_XMLNS) && !body.contains(CLASSIC_XMLNS),
        "case {case:?}: expected the ELBv2 model's own document ({V2_XMLNS}) and no other \
         model's, got {body}"
    );
}

/// The `<Code>` of an awsQuery error envelope, or the empty string for a success.
fn error_code(body: &str) -> String {
    find(&parse(body), "Code")
        .and_then(Element::text)
        .unwrap_or_default()
        .to_string()
}

/// The text of the first element named `tag`.
fn first_text(body: &str, tag: &str) -> Option<String> {
    find(&parse(body), tag)
        .and_then(Element::text)
        .map(str::to_string)
}
