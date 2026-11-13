// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the VALUES ACM's ACME surface puts on the wire, and for the
//! refusals it answers when a request addresses an endpoint it may not.
//!
//! RULE 1: the directory URL a managed ACME endpoint publishes carries the endpoint's own
//! id as the FIRST path segment and the ACME resource beneath it, and nothing else — no
//! segment of the service's own naming before the id, and no query or fragment — and every
//! URL the directory it serves advertises is built the same way.
//!
//! RULE 2: a projected enum member carries the value the service puts on the WIRE: a
//! DNS-prevalidated domain validation reports `PrevalidationType` `DNS`, on every
//! operation that projects it, however the prevalidation was configured.
//!
//! RULE 3: an ARN input member naming the endpoint a request acts UNDER is held to the
//! caller's own account: an ARN whose account segment is another account's is refused as
//! invalid input (`ValidationException`, HTTP 400) before any lookup, while an ARN of the
//! caller's own account that no record answers to is the missing-resource verdict
//! (`ResourceNotFoundException`).
//!
//! RULE 4: a domain validation whose prevalidation ACM is still working through is not a
//! resource an update may act on: the call is refused (`ValidationException`, HTTP 400)
//! and writes nothing, so racing updates cannot leave a torn or non-deterministic
//! configuration behind.
//!
//! RULE 5: `DeleteAcmeEndpoint` removes the endpoint and everything that exists only on
//! it, so nothing under the endpoint's ARN answers afterwards and its published URL serves
//! nothing — whatever status the endpoint carried, and however often the call is repeated.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every read goes through a DIFFERENT
//! operation than the write, so a response that is not computed from the persisted store
//! cannot pass. The wire harness, the endpoint/validation helpers and the clock helpers
//! are shared with [`crate::certificate_lifecycle`], [`crate::acme_endpoint_lifecycle`],
//! [`crate::acme_domain_validation_lifecycle`] and [`crate::acme_directory_endpoint`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_directory_endpoint::{
    activate_endpoints_now, body_json, create_endpoint as create_endpoint_in, endpoint_id,
    endpoint_url, fetch, split_url, DIRECTORY_MEMBERS, DNS_SUFFIX, ENROLLMENT_LABEL, REGIONS,
};
use crate::acme_domain_validation_lifecycle::{
    create_validation, describe_validation, endpoint_body, hold_prevalidations,
    settle_prevalidations_now, DOMAIN_SCOPE_OPTIONS, HOSTED_ZONE_ID,
};
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::certificate_lifecycle::{
    call, error_code, error_message, router_with_engine, ACCOUNT, REGION,
};

/// The `PrevalidationType` ACM puts on the wire for a DNS-prevalidated domain validation.
///
/// Written out here rather than read from the service, so this suite pins the published
/// VALUE: it is what a client branches on to find the DNS challenge record it has to
/// publish.
const DNS_PREVALIDATION_TYPE: &str = "DNS";

/// The ACME resource a published `EndpointUrl` ends at (RFC 8555 §7.1.1).
const DIRECTORY_RESOURCE: &str = "directory";

/// A `PrevalidationOptions` union value carrying `options` as its `DnsPrevalidation`
/// member — the one member the union declares.
fn dns_prevalidation(options: Value) -> Value {
    json!({ "DnsPrevalidation": options })
}

/// The `DnsPrevalidationDetails` a described or listed validation reports.
fn details(validation: &Value) -> Value {
    validation["PrevalidationDetails"]["DnsPrevalidation"].clone()
}

/// RULE 1: every endpoint's published directory URL has the path `/<endpoint id>/directory`
/// and nothing more, the directory served there advertises its sibling resources the same
/// way, and the URL is the one the emulator really serves.
///
/// The rows differ along what the rule generalises over: the Region (five Region-name
/// shapes) and the endpoint within it (two per Region, created from identical requests, so
/// one host serves many and the id is what distinguishes their paths).
#[tokio::test]
async fn the_published_directory_url_is_the_endpoint_id_and_the_resource() {
    let (router, engine) = router_with_engine();

    for region in REGIONS {
        for attempt in 0..2 {
            let case = format!("{region} endpoint #{attempt}");
            let arn = create_endpoint_in(&router, &case, region, "client-supplied-marker").await;
            activate_endpoints_now(&engine);
            let url = endpoint_url(&router, &case, region, &arn).await;
            let id = endpoint_id(&arn);
            let (host, path) = split_url(&case, &url);

            assert_eq!(
                host,
                format!("{ENROLLMENT_LABEL}.{region}.{DNS_SUFFIX}"),
                "case {case:?}: expected the Region's enrollment host, got {host:?} from {url:?}"
            );
            assert_eq!(
                path,
                format!("/{id}/{DIRECTORY_RESOURCE}"),
                "case {case:?}: expected the path to be the endpoint id and the directory \
                 resource, got {path:?} from {url:?}"
            );
            assert!(
                !url.contains('?') && !url.contains('#'),
                "case {case:?}: a published directory URL carries no query or fragment, got {url:?}"
            );

            // The URL is one the service SERVES, and the resource map it answers names its
            // sibling resources with paths of the same form.
            let response = fetch(&router, &case, "GET", &url).await;
            let status = response.status().as_u16();
            let document = body_json(&case, &response);
            assert_eq!(
                status, 200,
                "case {case:?}: the published URL must be served, got {status} {document}"
            );
            for (member, resource) in DIRECTORY_MEMBERS {
                let advertised = document[*member].as_str().unwrap_or_default().to_string();
                let (member_host, member_path) = split_url(&case, &advertised);
                assert_eq!(
                    member_host, host,
                    "case {case:?}: expected {member} on {host:?}, got {advertised:?}"
                );
                assert_eq!(
                    member_path,
                    format!("/{id}/{resource}"),
                    "case {case:?}: expected the {member} path to be the endpoint id and \
                     {resource}, got {member_path:?} from {advertised:?}"
                );
            }
        }
    }
}

/// RULE 2: `PrevalidationType` is the wire value `DNS` on every projection of a
/// DNS-prevalidated validation — the describe, the list, and the describe after an update
/// replaced the configuration.
///
/// The rows differ along what the rule generalises over: which members the prevalidation
/// configuration names (neither, the hosted zone alone, the scope alone, both) and the
/// scope values inside it, none of which the projected TYPE depends on.
#[tokio::test]
async fn prevalidation_type_is_the_wire_value_on_every_projection() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "prevalidation host", &endpoint_body()).await;

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "no prevalidation configuration at all",
            "plain.example.com",
            json!({}),
        ),
        (
            "the hosted zone alone",
            "zone.example.com",
            json!({ "HostedZoneId": HOSTED_ZONE_ID }),
        ),
        (
            "the domain scope alone",
            "scope.example.com",
            json!({
                "DomainScope": {
                    "ExactDomain": DOMAIN_SCOPE_OPTIONS[0],
                    "Subdomains": DOMAIN_SCOPE_OPTIONS[1],
                }
            }),
        ),
        (
            "the hosted zone and every scope member",
            "both.example.com",
            json!({
                "HostedZoneId": HOSTED_ZONE_ID,
                "DomainScope": {
                    "ExactDomain": DOMAIN_SCOPE_OPTIONS[1],
                    "Subdomains": DOMAIN_SCOPE_OPTIONS[0],
                    "Wildcards": DOMAIN_SCOPE_OPTIONS[0],
                }
            }),
        ),
    ];

    for (case, domain, options) in rows {
        let arn = create_validation(
            &router,
            case,
            &json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": domain,
                "PrevalidationOptions": dns_prevalidation(options.clone()),
            }),
        )
        .await;

        let described = describe_validation(&router, case, &arn).await;
        assert_eq!(
            described["PrevalidationType"].as_str(),
            Some(DNS_PREVALIDATION_TYPE),
            "case {case:?}: expected PrevalidationType {DNS_PREVALIDATION_TYPE}, got {described}"
        );

        let (status, listed) = call(
            &router,
            "ListAcmeDomainValidations",
            &json!({ "AcmeEndpointArn": endpoint_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListAcmeDomainValidations must succeed, got {status} {listed}"
        );
        let summary = listed["AcmeDomainValidations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no AcmeDomainValidations in {listed}"))
            .iter()
            .find(|summary| summary["AcmeDomainValidationArn"].as_str() == Some(arn.as_str()))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: {arn} is not listed in {listed}"));
        assert_eq!(
            summary["PrevalidationType"].as_str(),
            Some(DNS_PREVALIDATION_TYPE),
            "case {case:?}: expected the listed PrevalidationType {DNS_PREVALIDATION_TYPE}, got \
             {summary}"
        );

        // And after an update replaced the configuration the type is still the wire value:
        // the validation has to be settled first, which is the state an update acts on.
        settle_prevalidations_now(&engine);
        let (status, answer) = call(
            &router,
            "UpdateAcmeDomainValidation",
            &json!({
                "AcmeDomainValidationArn": arn,
                "PrevalidationOptions": dns_prevalidation(json!({
                    "HostedZoneId": HOSTED_ZONE_ID,
                    "DomainScope": { "Wildcards": DOMAIN_SCOPE_OPTIONS[1] },
                })),
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateAcmeDomainValidation must succeed once settled, got {status} \
             {answer}"
        );
        let updated = describe_validation(&router, case, &arn).await;
        assert_eq!(
            updated["PrevalidationType"].as_str(),
            Some(DNS_PREVALIDATION_TYPE),
            "case {case:?}: expected PrevalidationType {DNS_PREVALIDATION_TYPE} after the update, \
             got {updated}"
        );
    }
}

/// RULE 3: an endpoint ARN of ANOTHER account is refused as invalid input by every
/// operation that takes the endpoint a request acts under, while an ARN of the caller's own
/// account that no record answers to is reported as missing — and neither refusal echoes
/// the ARN back or changes what a later caller sees.
///
/// The rows differ along what the rule generalises over: the account segment (another
/// 12-digit account, one in another Region, one in another partition, and an account of all
/// nines) and the operation the ARN reaches (a create under the endpoint, two reads of what
/// hangs off it, and the endpoint's own delete).
#[tokio::test]
async fn an_endpoint_arn_of_another_account_is_refused_as_invalid_input() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "owned host", &endpoint_body()).await;
    let owned = create_validation(
        &router,
        "owned validation",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "DomainName": "owned.example.com",
            "PrevalidationOptions": dns_prevalidation(json!({})),
        }),
    )
    .await;
    hold_prevalidations(&engine);

    let foreign: &[(&str, String)] = &[
        (
            "another account in this Region",
            format!("arn:aws:acm:{REGION}:111122223333:acme-endpoint/2b260b3a-a1db-47b2-98dd-b80925898547"),
        ),
        (
            "another account in another Region",
            "arn:aws:acm:eu-central-1:210987654321:acme-endpoint/33333333-3333-4333-8333-333333333333".to_string(),
        ),
        (
            "another account in another partition",
            "arn:aws-us-gov:acm:us-gov-west-1:444455556666:acme-endpoint/44444444".to_string(),
        ),
        (
            "an account of all nines",
            format!("arn:aws:acm:{REGION}:999999999999:acme-endpoint/a"),
        ),
    ];

    for (case, arn) in foreign {
        for (operation, body) in requests_under_endpoint(arn) {
            let case = format!("{case} on {operation}");
            let (status, refusal) = call(&router, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {arn} must be refused, got {status} {refusal}"
            );
            assert_eq!(
                error_code(&refusal),
                "ValidationException",
                "case {case:?}: expected ValidationException for {arn}, got {refusal}"
            );
            assert!(
                !error_message(&refusal).contains(arn.as_str()),
                "case {case:?}: a cross-account refusal must withhold the submitted ARN, got \
                 {refusal}"
            );
            assert_eq!(
                error_message(&refusal),
                crate::acme_authorization::cross_account_arn_message("acme-endpoint"),
                "case {case:?}: expected the cross-account verdict for the endpoint's own \
                 resource type, got {refusal}"
            );
        }
    }

    // The contrast: an ARN of the CALLER's account that no record answers to is a missing
    // resource, not a bad request. (`DeleteAcmeEndpoint` is not in this table: it models no
    // not-found error and is `@idempotent`, so an endpoint that is already gone is the
    // state it asks for — pinned by `deleting_an_endpoint_removes_everything_under_it`.)
    let unknown = format!("arn:aws:acm:{REGION}:{ACCOUNT}:acme-endpoint/00000000-0000-4000-8000-000000000000");
    for (operation, body) in requests_under_endpoint(&unknown) {
        if operation == "DeleteAcmeEndpoint" {
            continue;
        }
        let case = format!("an endpoint this account never created, on {operation}");
        let (status, refusal) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: {unknown} must be refused, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for {unknown}, got {refusal}"
        );
    }

    // None of those refusals wrote anything: the endpoint still holds exactly the one
    // validation that was created under it.
    let (status, listed) = call(
        &router,
        "ListAcmeDomainValidations",
        &json!({ "AcmeEndpointArn": endpoint_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the owned endpoint to still list its validations, got {status} {listed}"
    );
    let arns: Vec<&str> = listed["AcmeDomainValidations"]
        .as_array()
        .unwrap_or_else(|| panic!("no AcmeDomainValidations in {listed}"))
        .iter()
        .filter_map(|summary| summary["AcmeDomainValidationArn"].as_str())
        .collect();
    assert_eq!(
        arns,
        vec![owned.as_str()],
        "expected the refusals to have created nothing, got {listed}"
    );
}

/// One request per operation that takes `acme_endpoint_arn` as the endpoint it acts under:
/// the operation name and the body that addresses that endpoint.
fn requests_under_endpoint(acme_endpoint_arn: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreateAcmeDomainValidation",
            json!({
                "AcmeEndpointArn": acme_endpoint_arn,
                "DomainName": "refused.example.com",
                "PrevalidationOptions": { "DnsPrevalidation": {} },
            }),
        ),
        (
            "ListAcmeDomainValidations",
            json!({ "AcmeEndpointArn": acme_endpoint_arn }),
        ),
        (
            "ListAcmeAccounts",
            json!({ "AcmeEndpointArn": acme_endpoint_arn }),
        ),
        (
            "DeleteAcmeEndpoint",
            json!({ "AcmeEndpointArn": acme_endpoint_arn }),
        ),
    ]
}

/// RULE 4: while a validation's prevalidation is still in flight an update is refused and
/// writes nothing — including two updates racing with mutually exclusive configurations,
/// after which the stored configuration is still the one the create left — and the same
/// update is accepted once the prevalidation falls due.
///
/// The rows differ along what the rule generalises over: which member the racing updates
/// disagree on (the two `DomainScopeOption` values of one scope member, two different
/// hosted zones) — the refusal being about the resource's state, not about what the request
/// asked for.
#[tokio::test]
async fn updating_a_validation_in_flight_is_refused_and_writes_nothing() {
    let (router, engine) = router_with_engine();
    let endpoint_arn = create_endpoint(&router, "racing host", &endpoint_body()).await;

    /// A second `HostedZoneId` satisfying the member's modelled pattern (`^Z[A-Z0-9]+$`).
    const OTHER_HOSTED_ZONE_ID: &str = "Z1D633PJN98FT9";

    let rows: Vec<(&str, &str, Value, Value)> = vec![
        (
            "two scope values of one member",
            "scope.race.example.com",
            dns_prevalidation(json!({ "DomainScope": { "Subdomains": DOMAIN_SCOPE_OPTIONS[0] } })),
            dns_prevalidation(json!({ "DomainScope": { "Subdomains": DOMAIN_SCOPE_OPTIONS[1] } })),
        ),
        (
            "two hosted zones",
            "zone.race.example.com",
            dns_prevalidation(json!({ "HostedZoneId": HOSTED_ZONE_ID })),
            dns_prevalidation(json!({ "HostedZoneId": OTHER_HOSTED_ZONE_ID })),
        ),
    ];

    for (case, domain, first, second) in rows {
        let arn = create_validation(
            &router,
            case,
            &json!({
                "AcmeEndpointArn": endpoint_arn,
                "DomainName": domain,
                "PrevalidationOptions": dns_prevalidation(json!({})),
            }),
        )
        .await;
        // Pin the transient status: the prevalidation this create owes the validation is
        // pushed out of the test's way, so the state under test cannot race the wall clock.
        hold_prevalidations(&engine);
        let before = describe_validation(&router, case, &arn).await;

        let (left, right) = tokio::join!(
            update(&router, &arn, &first),
            update(&router, &arn, &second)
        );
        for (which, (status, refusal)) in [("first", left), ("second", right)] {
            assert_eq!(
                status, 400,
                "case {case:?}: the {which} racing update must be refused, got {status} {refusal}"
            );
            assert_eq!(
                error_code(&refusal),
                "ValidationException",
                "case {case:?}: expected ValidationException from the {which} update, got {refusal}"
            );
        }

        // Neither write landed, in whole or in part: the record is byte-for-byte the one
        // the create left.
        let after = describe_validation(&router, case, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: two refused updates must leave the record alone, got {after}"
        );

        // Once the prevalidation falls due the very same update is accepted, so the
        // refusal was about the state and not about the request.
        settle_prevalidations_now(&engine);
        let (status, answer) = update(&router, &arn, &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the same update must succeed once settled, got {status} {answer}"
        );
        hold_prevalidations(&engine);
        let accepted = describe_validation(&router, case, &arn).await;
        assert_eq!(
            details(&accepted)["DomainScope"],
            first["DnsPrevalidation"]["DomainScope"],
            "case {case:?}: expected the accepted update's DomainScope to be stored, got {accepted}"
        );
        assert_eq!(
            details(&accepted)["HostedZoneId"],
            first["DnsPrevalidation"]["HostedZoneId"],
            "case {case:?}: expected the accepted update's HostedZoneId to be stored, got \
             {accepted}"
        );
    }
}

/// `UpdateAcmeDomainValidation` for `arn`, naming `options`, as `(status, body)`.
async fn update(router: &MultiServiceRouter, arn: &str, options: &Value) -> (u16, Value) {
    call(
        router,
        "UpdateAcmeDomainValidation",
        &json!({ "AcmeDomainValidationArn": arn, "PrevalidationOptions": options }),
    )
    .await
}

/// RULE 5: deleting an endpoint removes it and everything that exists only on it, and the
/// delete is the same whatever status the endpoint carried and however often it is sent.
///
/// The rows differ along what the rule generalises over: the status the endpoint carries
/// when the delete arrives (`ACTIVE`, and one still `CREATING` — an endpoint the caller is
/// discarding before it ever served has nothing to wait for, so the status does not gate
/// the delete the way it gates an update) and whether it holds domain validations at all.
#[tokio::test]
async fn deleting_an_endpoint_removes_everything_under_it() {
    for (case, activate, domains) in [
        (
            "an ACTIVE endpoint holding two validations",
            true,
            &["one.example.com", "two.example.com"][..],
        ),
        ("an endpoint still CREATING, holding none", false, &[][..]),
    ] {
        let (router, engine) = router_with_engine();
        let endpoint_arn = create_endpoint(&router, case, &endpoint_body()).await;
        let mut validations = Vec::new();
        for domain in domains {
            validations.push(
                create_validation(
                    &router,
                    case,
                    &json!({
                        "AcmeEndpointArn": endpoint_arn,
                        "DomainName": domain,
                        "PrevalidationOptions": dns_prevalidation(json!({})),
                    }),
                )
                .await,
            );
        }
        if activate {
            activate_endpoints_now(&engine);
        }
        let url = endpoint_url(&router, case, REGION, &endpoint_arn).await;

        let (status, answer) = call(
            &router,
            "DeleteAcmeEndpoint",
            &json!({ "AcmeEndpointArn": endpoint_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteAcmeEndpoint must succeed, got {status} {answer}"
        );

        // Nothing under the endpoint's ARN answers any more.
        let (status, refusal) = call(
            &router,
            "DescribeAcmeEndpoint",
            &json!({ "AcmeEndpointArn": endpoint_arn }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the deleted endpoint must not be describable, got {status} {refusal}"
        );
        assert_eq!(
            error_code(&refusal),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException for the deleted endpoint, got \
             {refusal}"
        );
        for arn in &validations {
            let (status, refusal) = call(
                &router,
                "DescribeAcmeDomainValidation",
                &json!({ "AcmeDomainValidationArn": arn }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: validation {arn} must go with its endpoint, got {status} {refusal}"
            );
            assert_eq!(
                error_code(&refusal),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for {arn}, got {refusal}"
            );
        }

        // The endpoint is gone from the listing, and its published URL serves nothing.
        let (status, listed) = call(&router, "ListAcmeEndpoints", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListAcmeEndpoints must succeed, got {status} {listed}"
        );
        assert!(
            !listed.to_string().contains(endpoint_arn.as_str()),
            "case {case:?}: expected {endpoint_arn} to be gone from the listing, got {listed}"
        );
        let response = fetch(&router, case, "GET", &url).await;
        let status = response.status().as_u16();
        let document = body_json(case, &response);
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted endpoint's URL must serve nothing, got {status} {document}"
        );

        // And the delete is idempotent: repeating it asks for a state the store is already
        // in, so it succeeds and still leaves nothing behind.
        let (status, answer) = call(
            &router,
            "DeleteAcmeEndpoint",
            &json!({ "AcmeEndpointArn": endpoint_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a repeated DeleteAcmeEndpoint must succeed, got {status} {answer}"
        );
    }
}
