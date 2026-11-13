// SPDX-License-Identifier: CC0-1.0

//! The SHARED Amazon Connect instance gate, driven through this API version's
//! `StartInstanceOnboardingJob`.
//!
//! RULE under test: whether an operation that requires an Amazon Connect instance may
//! proceed is decided by ONE read of Amazon Connect's own instance registry — the account
//! holds an instance under the id the request names, or it does not — and the registry
//! resolves by instance id alone, so the answer is its present contents and not the id's
//! shape, its history or the region the request was signed for.
//!
//! What this API version DOES with that answer is its own: the registry outcome is
//! answered with the modelled `ResourceNotFoundException` (HTTP 404) carrying this
//! service's one fixed sentence for the class, where the first ConnectCampaigns API
//! version answers the same outcome with its modelled `ValidationException` (HTTP 400) —
//! the same sentence, the refusal each version's own model and recorded wire name. Pinning
//! that sentence here and in that version's own suite is what keeps the two from drifting
//! apart on the one text they share.
//!
//! Asserted over a TABLE whose rows differ along what the gate generalises over — the form
//! of the `connectInstanceId` (an `InstanceId`, len 1..256, pattern
//! `^[-_.a-zA-Z0-9]+$`, from `smithy_query(query="shape", shape="InstanceId")`), the
//! region the request is signed for, and whether the account ever held the id — rather than
//! along one request.

use serde_json::{json, Value};

use super::common::{call, error_code, router, Harness};

/// The sentence an operation that only REQUIRES a usable Connect instance answers an id
/// the account's registry does not hold with. Written out here rather than read back out
/// of the implementation, so this suite pins the wording itself; it carries nothing from
/// the request, so there is no place to fill.
const NOT_USABLE_WORDING: &str =
    "Connect Instance does not exist or is not enabled to use Campaigns.";

/// One row of the table: the instance id the account is given, the region the request is
/// signed for, and the region the instance is registered in — which the gate does not
/// read, because the registry resolves an instance by its id alone.
#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    region: &'static str,
    registered_in: &'static str,
    instance: String,
}

/// The table every suite below is driven over: the uuid form a Connect instance id takes,
/// the one-character id the `InstanceId` length minimum admits, every punctuation
/// character its pattern allows, the longest id its bound allows, and an instance the
/// account holds in a region other than the one the request is signed for.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the uuid form a Connect instance id takes",
            region: "us-east-1",
            registered_in: "us-east-1",
            instance: "7c0a554f-8100-4d2c-9e0a-1b4d2c7e0a55".to_string(),
        },
        Case {
            name: "one character (the length minimum)",
            region: "us-east-1",
            registered_in: "us-east-1",
            instance: "i".to_string(),
        },
        Case {
            name: "every punctuation character the pattern admits",
            region: "eu-west-1",
            registered_in: "eu-west-1",
            instance: "a.b_c-D9".to_string(),
        },
        Case {
            name: "256 characters (the length maximum)",
            region: "ap-southeast-2",
            registered_in: "ap-southeast-2",
            instance: "i".repeat(256),
        },
        Case {
            name: "an instance held in another region than the request is signed for",
            region: "us-west-2",
            registered_in: "eu-central-1",
            instance: "cross-region-instance".to_string(),
        },
    ]
}

/// `StartInstanceOnboardingJob`: `PUT /v2/connect-instance/{id}/onboarding`.
async fn start_onboarding(harness: &Harness, case: &Case) -> (u16, Value) {
    call(
        harness,
        case.region,
        "PUT",
        &format!("/v2/connect-instance/{}/onboarding", case.instance),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await
}

/// `GetInstanceOnboardingJobStatus`: `GET /v2/connect-instance/{id}/onboarding` — the read
/// that proves whether the start call persisted a job at all.
async fn onboarding_status(harness: &Harness, case: &Case) -> (u16, Value) {
    call(
        harness,
        case.region,
        "GET",
        &format!("/v2/connect-instance/{}/onboarding", case.instance),
        &Value::Null,
    )
    .await
}

/// An instance the account holds passes the gate: the job is started and reads back
/// through `GetInstanceOnboardingJobStatus` for the instance it was started for.
#[tokio::test]
async fn an_instance_the_registry_holds_passes_the_gate() {
    for case in cases() {
        let harness = router();
        harness.register_connect_instance(case.registered_in, &case.instance);

        let (status, started) = start_onboarding(&harness, &case).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 for an instance the account holds, got {status} \
             {started}",
            case.name
        );
        assert_eq!(
            started
                .pointer("/connectInstanceOnboardingJobStatus/connectInstanceId")
                .and_then(Value::as_str),
            Some(case.instance.as_str()),
            "case {:?}: the started job names the instance it onboards, got {started}",
            case.name
        );

        let (status, read_back) = onboarding_status(&harness, &case).await;
        assert_eq!(
            (
                status,
                read_back
                    .pointer("/connectInstanceOnboardingJobStatus/connectInstanceId")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
            ),
            (200, case.instance.as_str()),
            "case {:?}: expected the started job to read back for its instance, got \
             {status} {read_back}",
            case.name
        );
    }
}

/// An instance the account does NOT hold is refused — whether it never held the id or
/// held it and gave it up — with this API version's own modelled error and the shared
/// sentence, and no onboarding job is persisted for it.
#[tokio::test]
async fn an_instance_the_registry_does_not_hold_is_refused() {
    for case in cases() {
        for ever_held in [false, true] {
            let harness = router();
            if ever_held {
                harness.register_connect_instance(case.registered_in, &case.instance);
                assert!(
                    harness.deregister_connect_instance(&case.instance),
                    "case {:?}: expected the registered instance to be removable",
                    case.name
                );
            }

            let (status, refused) = start_onboarding(&harness, &case).await;
            assert_eq!(
                status, 404,
                "case {:?} (ever held: {ever_held}): expected HTTP 404 for an instance the \
                 account does not hold, got {status} {refused}",
                case.name
            );
            assert!(
                error_code(&refused).ends_with("ResourceNotFoundException"),
                "case {:?} (ever held: {ever_held}): expected ResourceNotFoundException, got \
                 {:?}",
                case.name,
                error_code(&refused)
            );
            assert_eq!(
                refused.get("message").and_then(Value::as_str),
                Some(NOT_USABLE_WORDING),
                "case {:?} (ever held: {ever_held}): expected the fixed sentence of this \
                 refusal class, got {refused}",
                case.name
            );

            // Nothing was persisted: the record reader finds no job for the instance.
            let (status, read_back) = onboarding_status(&harness, &case).await;
            assert_eq!(
                status, 404,
                "case {:?} (ever held: {ever_held}): a refused start persists no job, got \
                 {status} {read_back}",
                case.name
            );
        }
    }
}
