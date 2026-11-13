// SPDX-License-Identifier: CC0-1.0

//! The SHARED Amazon Connect instance gate, driven through this API version's own
//! operations.
//!
//! RULE under test: whether an operation that requires an Amazon Connect instance may
//! proceed is decided by ONE read of Amazon Connect's own instance registry — the account
//! holds an instance under the id the request names, or it does not — and that one answer
//! is what every operation of this service that requires a usable instance acts on. The
//! registry resolves by instance id alone, so the answer is the registry's present
//! contents and not the id's shape, its history or the region the request was signed for.
//!
//! What each operation DOES with that answer stays its own: `CreateCampaign` answers the
//! absent instance with the modelled `ValidationException` (HTTP 400) and the fixed
//! sentence of that class, while `StartInstanceOnboardingJob` — whose own resource is the
//! instance's onboarding record — answers the SAME outcome with the modelled
//! `ResourceNotFoundException` (HTTP 404) naming the instance the request asked about. One
//! read, two refusals, so the pair cannot disagree about whether the account holds an
//! instance while still answering in the words recorded for each operation.
//!
//! Asserted over a TABLE whose rows differ along what the gate generalises over — the form
//! of the `InstanceId` (len 0..256, pattern `^[a-zA-Z0-9_\-.]*$`, from
//! `smithy_query(query="shape", shape="InstanceId")`), the region the request is signed for
//! and whether the account ever held the id — rather than along one request.

use serde_json::{json, Value};

use super::common::{call, job_status, router, Answer, Harness};

/// The sentence an operation that only REQUIRES a usable Connect instance answers an id
/// the account's registry does not hold with. Written out here rather than read back out
/// of the implementation, so this suite pins the wording itself; it carries nothing from
/// the request, so there is no place to fill.
const NOT_USABLE_WORDING: &str =
    "Connect Instance does not exist or is not enabled to use Campaigns.";

/// The sentence `StartInstanceOnboardingJob`, whose own resource is the instance's
/// onboarding record, answers the same registry outcome with: fixed text around the id the
/// request carried, where `{id}` goes. Written out for the same reason.
const ONBOARDING_NOT_FOUND_WORDING: &str = "ConnectInstanceId not found {id}.";

/// One row of the table: the instance id the account is given and the region the request
/// that follows is signed for.
#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    /// The region the campaign request is signed for.
    region: &'static str,
    /// The region the instance is registered in, which the gate does not read: the
    /// registry resolves an instance by its id alone.
    registered_in: &'static str,
    instance: String,
}

/// The table every suite below is driven over: the uuid form a Connect instance id takes,
/// the empty id the `InstanceId` length minimum admits, every punctuation character its
/// pattern allows, the longest id its bound allows, and an instance the account holds in a
/// region other than the one the request is signed for.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the uuid form a Connect instance id takes",
            region: "us-east-1",
            registered_in: "us-east-1",
            instance: "1b4d2c7e-0a55-4f81-9d2c-7e0a554f8100".to_string(),
        },
        Case {
            name: "the empty id the length minimum admits",
            region: "us-east-1",
            registered_in: "us-east-1",
            instance: String::new(),
        },
        Case {
            name: "every punctuation character the pattern admits",
            region: "eu-west-2",
            registered_in: "eu-west-2",
            instance: "a.b_c-D9".to_string(),
        },
        Case {
            name: "256 characters (the length maximum)",
            region: "ap-southeast-1",
            registered_in: "ap-southeast-1",
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

/// A minimal `CreateCampaign` body for `instance`: every member the model marks required,
/// and nothing else.
fn create_body(name: &str, instance: &str) -> Value {
    json!({
        "name": name,
        "connectInstanceId": instance,
        "dialerConfig": {"progressiveDialerConfig": {"bandwidthAllocation": 0.5}},
        "outboundCallConfig": {"connectContactFlowId": "flow-1"},
    })
}

/// `CreateCampaign`: `PUT /campaigns`.
async fn create(harness: &Harness, case: &Case, name: &str) -> Answer {
    call(
        harness,
        case.region,
        "PUT",
        "/campaigns",
        &create_body(name, &case.instance),
    )
    .await
}

/// `StartInstanceOnboardingJob`: `PUT /connect-instance/{id}/onboarding`.
async fn start_onboarding(harness: &Harness, case: &Case) -> Answer {
    call(
        harness,
        case.region,
        "PUT",
        &format!("/connect-instance/{}/onboarding", case.instance),
        &json!({"encryptionConfig": {"enabled": false}}),
    )
    .await
}

/// An instance the account holds passes the gate: the create is served and the campaign
/// reads back through `DescribeCampaign` for the instance it was created for.
#[tokio::test]
async fn an_instance_the_registry_holds_passes_the_gate() {
    for case in cases() {
        let harness = router();
        harness.register_connect_instance(case.registered_in, &case.instance);

        let created = create(&harness, &case, "gated-campaign").await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected HTTP 200 for an instance the account holds, got {} {}",
            case.name, created.status, created.body
        );
        let id = created.body["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: no minted id in {}", case.name, created.body))
            .to_string();

        let described = call(
            &harness,
            case.region,
            "GET",
            &format!("/campaigns/{id}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            (
                described.status,
                described
                    .body
                    .pointer("/campaign/connectInstanceId")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
            ),
            (200, case.instance.as_str()),
            "case {:?}: expected the campaign to read back for the instance it was created \
             for, got {} {}",
            case.name,
            described.status,
            described.body
        );
    }
}

/// An instance the account does NOT hold is refused — whether it never held the id or
/// held it and gave it up — and nothing is minted for it.
#[tokio::test]
async fn an_instance_the_registry_does_not_hold_is_refused_by_create_campaign() {
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

            let refused = create(&harness, &case, "ungated-campaign").await;
            assert_eq!(
                refused.status, 400,
                "case {:?} (ever held: {ever_held}): expected HTTP 400 for an instance the \
                 account does not hold, got {} {}",
                case.name, refused.status, refused.body
            );
            assert!(
                refused.error_code.ends_with("ValidationException"),
                "case {:?} (ever held: {ever_held}): expected ValidationException, got {:?}",
                case.name,
                refused.error_code
            );
            assert_eq!(
                refused.message(),
                NOT_USABLE_WORDING,
                "case {:?} (ever held: {ever_held}): expected the fixed sentence of this \
                 refusal class, got {}",
                case.name,
                refused.body
            );
            assert!(
                refused.body.get("id").is_none() && refused.body.get("arn").is_none(),
                "case {:?} (ever held: {ever_held}): a refused create mints nothing, got {}",
                case.name,
                refused.body
            );
        }
    }
}

/// The SAME read serves `StartInstanceOnboardingJob`, which maps the one outcome to its
/// own modelled refusal: HTTP 404 naming the instance for an id the account does not hold,
/// and a started job for one it does.
#[tokio::test]
async fn the_same_read_gates_instance_onboarding_in_this_service_s_own_words() {
    for case in cases() {
        let harness = router();

        let refused = start_onboarding(&harness, &case).await;
        assert_eq!(
            refused.status, 404,
            "case {:?}: expected HTTP 404 from StartInstanceOnboardingJob for an instance \
             the account does not hold, got {} {}",
            case.name, refused.status, refused.body
        );
        assert!(
            refused.error_code.ends_with("ResourceNotFoundException"),
            "case {:?}: expected ResourceNotFoundException, got {:?}",
            case.name,
            refused.error_code
        );
        assert_eq!(
            refused.message(),
            ONBOARDING_NOT_FOUND_WORDING.replace("{id}", &case.instance),
            "case {:?}: expected this operation's own wording for an instance it cannot \
             find, got {}",
            case.name,
            refused.body
        );

        // The one registration is all it takes for the same request to be served: the
        // gate is the registry's contents and nothing else about the call changed.
        harness.register_connect_instance(case.registered_in, &case.instance);
        let started = start_onboarding(&harness, &case).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected HTTP 200 from StartInstanceOnboardingJob once the account \
             holds the instance, got {} {}",
            case.name, started.status, started.body
        );
        assert_eq!(
            started
                .body
                .pointer("/connectInstanceOnboardingJobStatus/connectInstanceId")
                .and_then(Value::as_str),
            Some(case.instance.as_str()),
            "case {:?}: the started job names the instance it onboards, got {}",
            case.name,
            started.body
        );
        assert!(
            !job_status(&started).is_empty(),
            "case {:?}: the started job reports a status, got {}",
            case.name,
            started.body
        );
    }
}
