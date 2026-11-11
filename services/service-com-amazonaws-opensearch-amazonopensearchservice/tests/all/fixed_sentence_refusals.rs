// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusals this service states as FIXED sentences — the ones
//! whose whole message is the service's own text, with no value of the request in it.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness and the per-operation request helpers come from the sibling suites.
//!
//! The rules, each asserted over a SET of inputs rather than one call:
//!
//! * a batch package operation that names the same package more than once is refused as a
//!   WHOLE with the service's fixed duplicate-batch sentence, which names neither the
//!   member that repeated it nor the id — asserted over where the repeat sits in the
//!   batch and how long the batch is, through both the associate and the dissociate form;
//! * an `ActionID` that names no scheduled change of the domain is refused with the
//!   service's fixed sentence for an action id it cannot resolve, which names neither the
//!   id nor the domain — asserted over a table of unresolvable ids;
//! * a grant or a withdrawal of VPC endpoint access to a domain the service cannot serve
//!   one for is refused with the deliberately NON-COMMITTAL sentence, which does not
//!   disclose whether a domain of that name exists — asserted over a table of names, on
//!   both operations, with the sentence checked to carry no part of the name;
//! * an upgrade the published relation does not offer states the target as its BARE
//!   release, with the engine prefix the `VersionString` carries stripped, while the
//!   version the domain runs keeps the prefix its record declares it with — asserted over
//!   targets of both engine lines.

use serde_json::json;

use super::control_plane_refusal_wording::message;
use super::domain_lifecycle::{
    create_baseline, error_code, Emulator, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::package_associations::available_packages;
use super::package_dissociation::{associate, dissociate_many};
use super::scheduled_action_updates::{body as reschedule_body, update as reschedule};
use super::scheduled_actions::{base_instant, seed};
use super::upgrade_history::{settled_domain, upgrade};
use super::vpc_endpoint_access::{authorize, created_domain};
use super::vpc_endpoint_access_revocation::revoke;

/// The sentences under test, spelled out here rather than read from the implementation:
/// the WORDING is the requirement, so an expectation that imported the constant the
/// handler states would pass whatever that constant said.
///
/// A batch that names one package more than once.
const DUPLICATE_PACKAGES_IN_REQUEST: &str =
    "Operation not allowed. Duplicate packages found in the request.";
/// An `ActionID` that names no scheduled change of the domain.
const SCHEDULED_ACTION_NOT_FOUND: &str =
    "ReleaseVersion does not applies for this domain or release does not exist";
/// `ScheduleAt` naming a specific time with no instant to schedule for.
const DESIRED_START_TIME_REQUIRED: &str =
    "Please provide the desired start time to schedule for a specific time.";
/// A domain the service cannot serve an interface VPC endpoint for — the sentence that
/// states every reason and discloses which one applies for none of them.
const DOMAIN_UNAVAILABLE_FOR_VPC_ENDPOINT: &str =
    "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you \
     don't have permission to create a VPC endpoint for the domain.";

#[tokio::test]
async fn a_batch_that_repeats_a_package_is_refused_with_the_fixed_duplicate_sentence() {
    let emulator = Emulator::new();
    let ids = available_packages(&emulator).await;

    // One row per shape of repeat: the same package twice on its own, a repeat with
    // another package between them, a repeat at the end of a longer batch, and the same
    // package named three times — so the sentence is asserted over WHERE the repeat sits
    // and how long the batch is.
    let rows: Vec<(&str, Vec<usize>)> = vec![
        ("the same package twice", vec![0, 0]),
        ("a repeat with another package between", vec![1, 2, 1]),
        ("a repeat at the end of a longer batch", vec![0, 1, 2, 3, 3]),
        ("the same package three times", vec![2, 2, 2]),
    ];

    for (index, (case, packages)) in rows.iter().enumerate() {
        let domain = format!("dup-batch-{index}");
        create_baseline(&emulator, &domain).await;
        let named: Vec<String> = packages.iter().map(|at| ids[*at].clone()).collect();

        // (the wire form, the answer it gave). Both batch operations judge one rule about
        // the request, so both state the one sentence.
        for (form, answer) in [
            (
                "AssociatePackages",
                associate(&emulator, &domain, &named).await,
            ),
            (
                "DissociatePackages",
                dissociate_many(
                    &emulator,
                    json!({ "DomainName": domain, "PackageList": named }),
                )
                .await,
            ),
        ] {
            let case = format!("{form} / {case}");
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (400, "ValidationException"),
                "case {case}: expected the modelled validation refusal, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                message(&answer),
                DUPLICATE_PACKAGES_IN_REQUEST,
                "case {case}: expected {DUPLICATE_PACKAGES_IN_REQUEST:?}, got {:?}",
                answer.body
            );
            // The sentence carries nothing of the request: no id and no domain name.
            for value in [ids[*packages.first().expect("a row names a package")].as_str(), domain.as_str()] {
                assert!(
                    !message(&answer).contains(value),
                    "case {case}: expected the fixed sentence to carry no {value:?}, got {:?}",
                    answer.body
                );
            }
        }
    }
}

#[tokio::test]
async fn an_action_id_that_names_no_change_is_refused_with_the_fixed_sentence() {
    let emulator = Emulator::new();
    let domain = "sched-wording";
    let sibling = "sched-wording-other";
    create_baseline(&emulator, domain).await;
    create_baseline(&emulator, sibling).await;
    seed(&emulator, domain, base_instant());

    // One row per shape of unresolvable id: one invented, one shaped like a GUID, one
    // shaped like a release version (what a `SERVICE_SOFTWARE_UPDATE` change is addressed
    // by), and one that names a change of ANOTHER domain.
    for (label, target, action_id) in [
        ("an invented id", domain, "no-such-action"),
        (
            "a GUID-shaped id",
            domain,
            "00000000-0000-0000-0000-000000000000",
        ),
        ("a release-shaped id", domain, "R20990101-P1"),
        (
            "an id from another domain's listing",
            sibling,
            "ssu-pending",
        ),
    ] {
        let case = format!("{label} ({action_id:?})");
        let answer = reschedule(
            &emulator,
            target,
            reschedule_body(action_id, "SERVICE_SOFTWARE_UPDATE", "NOW", None),
        )
        .await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
            "case {case}: expected the modelled not-found refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            message(&answer),
            SCHEDULED_ACTION_NOT_FOUND,
            "case {case}: expected {SCHEDULED_ACTION_NOT_FOUND:?}, got {:?}",
            answer.body
        );
        for value in [action_id, target] {
            assert!(
                !message(&answer).contains(value),
                "case {case}: expected the fixed sentence to carry no {value:?}, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_schedule_for_a_specific_time_without_one_states_the_fixed_sentence() {
    let emulator = Emulator::new();
    let domain = "sched-wording-timestamp";
    create_baseline(&emulator, domain).await;
    seed(&emulator, domain, base_instant());

    // `TIMESTAMP` "must also provide a value for DesiredStartTime", so a request naming
    // that value without the instant is refused — whatever else it names. The rows differ
    // in the `ActionID` and the `ActionType` they carry: an action the domain HOLDS and
    // one it does not, so the rule is asserted as a property of the REQUEST, decided
    // before the change it names is looked up.
    for (case, action_id, action_type) in [
        ("an action the domain holds", "ssu-pending", "SERVICE_SOFTWARE_UPDATE"),
        ("an action the domain does not hold", "no-such-action", "SERVICE_SOFTWARE_UPDATE"),
        ("an Auto-Tune action", "heap-tuning", "JVM_HEAP_SIZE_TUNING"),
    ] {
        let answer = reschedule(
            &emulator,
            domain,
            reschedule_body(action_id, action_type, "TIMESTAMP", None),
        )
        .await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case}: expected the modelled validation refusal, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            message(&answer),
            DESIRED_START_TIME_REQUIRED,
            "case {case}: expected {DESIRED_START_TIME_REQUIRED:?}, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn vpc_endpoint_access_on_a_domain_it_cannot_serve_states_the_non_committal_sentence() {
    let emulator = Emulator::new();

    // One row per way a well-formed `DomainName` resolves to no domain the service can
    // serve an endpoint for: a name nothing was created under, the shortest and the
    // longest name the member's `@length(min: 3, max: 28)` admits, and a name whose
    // domain was deleted.
    let deleted = "vpc-access-deleted";
    created_domain(&emulator, deleted).await;
    let dropped = emulator.delete(deleted).await;
    assert_eq!(
        dropped.status, 200,
        "case DeleteDomain({deleted:?}): expected 200, got {} with {:?}",
        dropped.status, dropped.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    let names: Vec<String> = vec![
        "vpc-access-absent".to_string(),
        "abc".to_string(),
        format!("a{}", "b".repeat(27)),
        deleted.to_string(),
    ];
    // (the operation, the principal the request names). The grant and the withdrawal
    // address the same set, so both state the one sentence.
    for name in &names {
        for (operation, answer) in [
            (
                "AuthorizeVpcEndpointAccess",
                authorize(&emulator, name, json!({ "Account": "123456789012" })).await,
            ),
            (
                "RevokeVpcEndpointAccess",
                revoke(&emulator, name, json!({ "Account": "123456789012" })).await,
            ),
        ] {
            let case = format!("{operation} for the unservable domain {name:?}");
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
                "case {case}: expected the modelled not-found refusal, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                message(&answer),
                DOMAIN_UNAVAILABLE_FOR_VPC_ENDPOINT,
                "case {case}: expected {DOMAIN_UNAVAILABLE_FOR_VPC_ENDPOINT:?}, got {:?}",
                answer.body
            );
            // The refusal discloses nothing about the domain: the name the request
            // carried appears nowhere in it.
            assert!(
                !message(&answer).contains(name.as_str()),
                "case {case}: expected the refusal to disclose no domain name, got {:?}",
                answer.body
            );
        }
    }

    // The boundary of the same rule: a domain the service DOES hold is served, so the
    // sentence above is about the domain and not about the operation.
    let held = "vpc-access-held";
    created_domain(&emulator, held).await;
    let granted = authorize(&emulator, held, json!({ "Account": "123456789012" })).await;
    assert_eq!(
        granted.status, 200,
        "case AuthorizeVpcEndpointAccess({held:?}): expected the grant to be accepted, got {} \
         with {:?}",
        granted.status, granted.body
    );
    let withdrawn = revoke(&emulator, held, json!({ "Account": "123456789012" })).await;
    assert_eq!(
        withdrawn.status, 200,
        "case RevokeVpcEndpointAccess({held:?}): expected the withdrawal to be accepted, got {} \
         with {:?}",
        withdrawn.status, withdrawn.body
    );
}

#[tokio::test]
async fn an_unsupported_upgrade_states_the_target_as_its_bare_release() {
    let emulator = Emulator::new();
    let source = "OpenSearch_2.11";
    let name = "upgrade-bare-target";
    settled_domain(&emulator, name, source).await;

    // One row per engine line a target can name, and per shape of its release numbers: a
    // one-digit Elasticsearch major with a two-digit minor, a one-digit OpenSearch
    // release, and a two-digit OpenSearch major — every one of them a version this domain
    // cannot be upgraded to, so each is answered by the same refusal.
    // (the requested TargetVersion, the release the sentence states)
    for (target, release) in [
        ("Elasticsearch_7.10", "7.10"),
        ("Elasticsearch_5.1", "5.1"),
        ("OpenSearch_1.3", "1.3"),
        ("OpenSearch_10.0", "10.0"),
    ] {
        for check_only in [true, false] {
            let case = format!("{source} -> {target} (PerformCheckOnly {check_only})");
            let answer = upgrade(&emulator, name, target, check_only).await;
            let expected =
                format!("Failed to submit upgrade. Upgrade from {source} to {release} not supported.");
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (400, "BaseException"),
                "case {case}: expected the generic envelope, got {} with {:?}",
                answer.status,
                answer.body
            );
            assert_eq!(
                message(&answer),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                answer.body
            );
            // The target is stated WITHOUT its engine prefix, while the source keeps the
            // one its record declares.
            assert!(
                !message(&answer).contains(target),
                "case {case}: expected the target's engine prefix to be stripped, got {:?}",
                answer.body
            );
            assert!(
                message(&answer).contains(source),
                "case {case}: expected the domain's own version as it is declared, got {:?}",
                answer.body
            );
        }
    }
}
