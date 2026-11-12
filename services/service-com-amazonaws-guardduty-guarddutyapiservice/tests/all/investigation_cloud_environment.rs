// SPDX-License-Identifier: CC0-1.0

//! The cloud environment one investigation reports, end to end over the wire (the
//! harness lives in the `detector_lifecycle` suite).
//!
//! The rule under test, asserted over a TABLE rather than over one recorded call:
//! `Investigation.Cloud` is a projection of the environment the investigation ITSELF
//! resolved and stored, and of nothing about the request a read arrives on. So:
//!
//! 1. While the analysis pass has not ended — the investigation is still `RUNNING` —
//!    the record has resolved no environment and the read reports NO `cloud` member.
//! 2. Once the pass has ended, the read reports the environment that investigation
//!    covered: the provider (`CloudProvider`'s only member,
//!    `smithy_query(shape="CloudProvider")`), the Region the investigation was
//!    initiated through and the account that initiated it — which is the account the
//!    same body reports as `triggeredBy`.
//! 3. It is STORED, not rebuilt per read: repeated reads of one ended investigation
//!    report the same environment, and two investigations living in one store under
//!    different Regions each report their OWN Region rather than one endpoint's.
//!
//! The rows differ along what the rule generalises over: the Region the client talks
//! to (several members of the published catalogue, including ones no recorded call
//! used), the form and size of the prompt, and how many investigations the detector
//! holds.

use std::time::Duration;

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, new_router, Router};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    INVESTIGATION_REGIONS, INVESTIGATION_RUNNING,
};

/// `CloudProvider`'s only member — "The cloud provider. Currently, only `AWS` is
/// supported" (`smithy_query(query="doc",shape="CloudDetails",member="Provider")`).
/// Written out here rather than read from the service, so the test pins the wire
/// value instead of echoing it.
const CLOUD_PROVIDER: &str = "AWS";

/// How long a poll may wait for the clock to end an analysis before the test gives
/// up. Generous on purpose: the test waits for the CONDITION (a terminal status), and
/// this is only the deadline past which the emulator is considered broken.
const SETTLE_DEADLINE: Duration = Duration::from_secs(30);

/// How long the poll pauses between two reads. Only the gap between attempts; the
/// loop below ends on the condition, never on this.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One row: what it exercises, the Region the client talks to, and the prompts it
/// investigates through that Region's endpoint.
struct Row {
    case: &'static str,
    region: &'static str,
    prompts: &'static [&'static str],
}

/// The rows span the rule's inputs: Regions taken from the published catalogue
/// (`state::INVESTIGATION_REGIONS`, asserted below to still carry each one), prompt
/// forms from the shortest the model admits to the longest documented shape, and
/// detectors holding one, two and three investigations.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one investigation in the first catalogue Region",
            region: "us-east-1",
            prompts: &["summarise the finding on the bastion host"],
        },
        Row {
            case: "the Region the recorded call used",
            region: "us-west-2",
            prompts: &["Analyze findings in my organization"],
        },
        Row {
            case: "two investigations through a European endpoint",
            region: "eu-north-1",
            prompts: &["I", "why was this role assumed from a new ASN"],
        },
        Row {
            case: "three investigations through an Asia Pacific endpoint",
            region: "ap-northeast-1",
            prompts: &[
                "Investigate finding 1ab2c3d4e5f6a7b8c9d0e1f2a3b4c5d6 in account 000000000000",
                "Анализировать подозрительную активность IAM — 調査",
                "first line\nsecond line\twith a tab",
            ],
        },
    ]
}

/// Enable GuardDuty through `region`'s endpoint and return the detector's id.
async fn detector_in(router: &Router, region: &str, case: &str) -> String {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        "/detector",
        Some(json!({ "enable": true })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateDetector in {region} to answer 200, got {status} {body}"
    );
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a detectorId, got {body}"))
        .to_string()
}

/// Initiate one investigation through `region` and return its id.
async fn initiate(
    router: &Router,
    region: &str,
    detector_id: &str,
    prompt: &str,
    case: &str,
) -> String {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/investigation"),
        Some(json!({ "triggerPrompt": prompt })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateInvestigation to answer 200, got {status} {body}"
    );
    body["investigationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an investigationId, got {body}"))
        .to_string()
}

/// The `Investigation` `GetInvestigation` reports through `region`'s endpoint.
async fn investigation(
    router: &Router,
    region: &str,
    detector_id: &str,
    investigation_id: &str,
    case: &str,
) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/investigation/{investigation_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetInvestigation to answer 200, got {status} {body}"
    );
    body["investigation"].clone()
}

/// Read until the analysis pass has ended, or until [`SETTLE_DEADLINE`] has passed —
/// the condition decides, never a fixed wait.
async fn ended(
    router: &Router,
    region: &str,
    detector_id: &str,
    investigation_id: &str,
    case: &str,
) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let got = investigation(router, region, detector_id, investigation_id, case).await;
        if got["status"] != json!(INVESTIGATION_RUNNING) {
            return got;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the analysis of {investigation_id} never ended, last read {got}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// RULE 1 and 2 on one reported investigation: the reported `cloud` follows the
/// reported status, and when it is in it carries the environment this investigation
/// covered.
fn assert_cloud_follows_the_record(case: &str, label: &str, region: &str, got: &Value) {
    let cloud = &got["cloud"];
    if got["status"] == json!(INVESTIGATION_RUNNING) {
        assert_eq!(
            *cloud,
            Value::Null,
            "case {case:?} ({label}): an investigation still {INVESTIGATION_RUNNING} has \
             resolved no environment, so it reports no cloud member, got {got}"
        );
        return;
    }

    let account = got["triggeredBy"].as_str().unwrap_or_else(|| {
        panic!("case {case:?} ({label}): expected the initiating account, got {got}")
    });
    assert_eq!(
        *cloud,
        json!({
            "provider": CLOUD_PROVIDER,
            "region": region,
            "account": account,
        }),
        "case {case:?} ({label}): expected the environment this investigation covered \
         (provider {CLOUD_PROVIDER}, Region {region}, account {account}), got {cloud}"
    );
}

#[tokio::test]
async fn an_investigation_reports_the_environment_it_resolved_and_nothing_before() {
    // The rows' Regions come from the published catalogue, so a row cannot name a
    // Region the feature is not served through.
    for Row { case, region, .. } in rows() {
        assert!(
            INVESTIGATION_REGIONS.contains(&region),
            "case {case:?}: {region} is not a Region the feature is served through: \
             {INVESTIGATION_REGIONS:?}"
        );
    }

    for Row {
        case,
        region,
        prompts,
    } in rows()
    {
        // A store per row, so one row's investigations cannot explain another's.
        let router = new_router();
        let detector_id = detector_in(&router, region, case).await;

        let mut ids = Vec::new();
        for prompt in prompts {
            let id = initiate(&router, region, &detector_id, prompt, case).await;
            // Read straight back: whatever phase this lands in, the reported cloud
            // must follow the reported status rather than the endpoint the read
            // arrived on.
            let first = investigation(&router, region, &detector_id, &id, case).await;
            assert_cloud_follows_the_record(case, "the first read", region, &first);
            ids.push(id);
        }

        for id in &ids {
            let settled = ended(&router, region, &detector_id, id, case).await;
            assert_cloud_follows_the_record(case, "past the pass", region, &settled);

            // RULE 3: stored, not rebuilt — two more reads report the same
            // environment, and reading it never changes it.
            for round in 0..2 {
                let again = investigation(&router, region, &detector_id, id, case).await;
                assert_eq!(
                    again["cloud"], settled["cloud"],
                    "case {case:?} (read {round} after the pass): an ended investigation's \
                     environment is stored, so every read reports the same one, got {:?} then \
                     {:?}",
                    settled["cloud"], again["cloud"]
                );
            }
        }
    }
}

#[tokio::test]
async fn each_investigation_reports_its_own_region_not_the_endpoints_of_the_others() {
    // Two Regions of the catalogue in ONE store: if the member were rebuilt from the
    // serving context — or from a single stored constant — the two investigations
    // could not report different Regions.
    let router = new_router();
    let case = "two Regions in one store";
    let regions = ["ca-central-1", "eu-west-3"];

    let mut reported = Vec::new();
    for region in regions {
        let detector_id = detector_in(&router, region, case).await;
        let id = initiate(
            &router,
            region,
            &detector_id,
            "investigate the credential exfiltration finding",
            case,
        )
        .await;
        let settled = ended(&router, region, &detector_id, &id, case).await;
        assert_cloud_follows_the_record(case, region, region, &settled);
        reported.push(
            settled["cloud"]["region"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
    }

    assert_eq!(
        reported,
        regions.map(str::to_string).to_vec(),
        "case {case:?}: each investigation reports the Region it was initiated through, got \
         {reported:?}"
    );
}
