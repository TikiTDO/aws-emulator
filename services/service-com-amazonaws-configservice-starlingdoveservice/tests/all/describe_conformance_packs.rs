// SPDX-License-Identifier: CC0-1.0

//! `DescribeConformancePacks`, end to end over the wire.
//!
//! The rule pinned here: `DescribeConformancePacks` reports, for every conformance pack
//! this account and Region holds, the CONFIGURATION that pack was last deployed with —
//! its name, ARN and id, the Amazon S3 location its template is stored at and the input
//! parameters it carries — taking every member out of state rather than from the request
//! that asked for it. A request naming no pack reports them all; a request naming packs
//! reports those, in the packs' name ordering; a name this account holds no pack for is
//! refused with `NoSuchConformancePackException`, which names every such name the
//! request carried; and the set is paged with `Limit` and the opaque `NextToken` the
//! previous page answered.
//!
//! Every case drives the emulator the way a client does — an `awsJson1_1` `POST /`
//! through the same `MultiServiceRouter` the production router builds — using the
//! `Emulator` harness `config_setup_and_rule_loop` defines and the template helpers
//! `conformance_packs` defines.

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::DESCRIBE_CONFORMANCE_PACKS_PAGE_SIZE;

use crate::config_setup_and_rule_loop::Emulator;
use crate::conformance_packs::{pack_from_body, past_the_deployment, rule_template};

/// How Config words `NoSuchConformancePackException` for the names a request carried
/// that resolve to no pack: the fixed sentence, then the names themselves — which THIS
/// operation, the one that takes a LIST of pack names, renders inside square brackets.
const NO_SUCH_PACK_SENTENCE: &str = "No such conformance pack exists. Conformance Pack(s): ";

/// The pack details of a `DescribeConformancePacks` response.
fn described(case: &str, body: &Value) -> Vec<Value> {
    body["ConformancePackDetails"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected ConformancePackDetails, got {body}"))
        .clone()
}

/// The names of the packs a response reports, in the order reported.
fn described_names(case: &str, body: &Value) -> Vec<String> {
    described(case, body)
        .iter()
        .map(|detail| {
            detail["ConformancePackName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a pack detail carries a name, got {detail}"))
                .to_string()
        })
        .collect()
}

/// The `DescribeConformancePacks` body naming `names` (every pack when empty).
fn describe_of(names: &[&str]) -> Value {
    if names.is_empty() {
        json!({})
    } else {
        json!({ "ConformancePackNames": names })
    }
}

/// Deploy `name` from a one-rule template, with `extras` merged into the
/// `PutConformancePack` request, and answer the ARN the deployment was given.
async fn deploy(case: &str, emulator: &Emulator, name: &str, extras: &Value) -> String {
    let mut request = pack_from_body(name, &rule_template("only-rule"));
    for (member, value) in extras
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the extras are a JSON object, got {extras}"))
    {
        request[member] = value.clone();
    }
    let created = emulator.ok("PutConformancePack", &request).await;
    created["ConformancePackArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConformancePackArn, got {created}"))
        .to_string()
}

#[tokio::test]
async fn every_pack_is_reported_with_the_configuration_it_was_deployed_with() {
    // Rows differ along the configuration the pack was deployed with: no delivery
    // location and no input parameters, a bucket on its own, a bucket with the key
    // prefix under it, one input parameter, and several of them.
    let cases: [(&str, &str, Value); 5] = [
        (
            "no delivery location and no input parameters",
            "bare-pack",
            json!({}),
        ),
        (
            "a delivery bucket on its own",
            "bucket-pack",
            json!({ "DeliveryS3Bucket": "config-pack-templates" }),
        ),
        (
            "a bucket and the key prefix under it",
            "prefix-pack",
            json!({
                "DeliveryS3Bucket": "config-pack-templates",
                "DeliveryS3KeyPrefix": "packs/first",
            }),
        ),
        (
            "one input parameter",
            "one-parameter-pack",
            json!({
                "ConformancePackInputParameters": [
                    { "ParameterName": "BucketName", "ParameterValue": "audit-logs" },
                ],
            }),
        ),
        (
            "several input parameters",
            "many-parameters-pack",
            json!({
                "ConformancePackInputParameters": [
                    { "ParameterName": "BucketName", "ParameterValue": "audit-logs" },
                    { "ParameterName": "RetentionDays", "ParameterValue": "90" },
                    { "ParameterName": "Owner", "ParameterValue": "" },
                ],
            }),
        ),
    ];

    for (case, name, extras) in cases {
        let emulator = Emulator::new();
        let arn = deploy(case, &emulator, name, &extras).await;

        let body = emulator
            .ok("DescribeConformancePacks", &describe_of(&[name]))
            .await;
        let reported = described(case, &body);
        let [detail] = reported.as_slice() else {
            panic!("case {case:?}: expected exactly one pack detail, got {body}");
        };

        assert_eq!(
            (
                detail["ConformancePackName"].as_str(),
                detail["ConformancePackArn"].as_str(),
            ),
            (Some(name), Some(arn.as_str())),
            "case {case:?}: the pack is reported under the name and ARN it was deployed with, \
             got {detail}"
        );
        assert_eq!(
            (
                detail["DeliveryS3Bucket"].as_str(),
                detail["DeliveryS3KeyPrefix"].as_str(),
            ),
            (
                extras["DeliveryS3Bucket"].as_str(),
                extras["DeliveryS3KeyPrefix"].as_str(),
            ),
            "case {case:?}: the delivery location is the one the deployment carried, got {detail}"
        );
        assert_eq!(
            detail["ConformancePackInputParameters"],
            extras
                .get("ConformancePackInputParameters")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: the input parameters are the ones the deployment carried, got {detail}"
        );
        // `CreatedBy` is "the Amazon Web Services service that created the conformance
        // pack", which a pack the account deployed itself has none of, and
        // `TemplateSSMDocumentDetails` belongs to a pack deployed from a Systems Manager
        // document, which this one was not.
        assert!(
            detail["CreatedBy"].is_null() && detail["TemplateSSMDocumentDetails"].is_null(),
            "case {case:?}: a pack the account deployed itself names no creating service and no \
             SSM document, got {detail}"
        );
        assert!(
            detail["LastUpdateRequestedTime"].is_number()
                || detail["LastUpdateRequestedTime"].is_string(),
            "case {case:?}: the detail carries the instant the deployment was requested, got \
             {detail}"
        );

        // The id is the one the status read reports for the same pack: one pack, one
        // identity, whichever reader is asked.
        let status = emulator
            .ok(
                "DescribeConformancePackStatus",
                &json!({ "ConformancePackNames": [name] }),
            )
            .await;
        let status_id = status["ConformancePackStatusDetails"][0]["ConformancePackId"].as_str();
        assert!(
            detail["ConformancePackId"].as_str().is_some_and(|id| !id.is_empty())
                && detail["ConformancePackId"].as_str() == status_id,
            "case {case:?}: the pack reports the id its status reports ({status_id:?}), got \
             {detail}"
        );

        // The read is a report: it must not change what the next reader sees.
        let again = emulator
            .ok("DescribeConformancePacks", &describe_of(&[name]))
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: a second read must report the same detail, got {again}"
        );
    }
}

#[tokio::test]
async fn a_redeploy_is_reported_with_what_it_was_last_deployed_with() {
    // The pack's stored configuration is REPLACED by the next deployment of the same
    // name, while its identity is not. Rows differ along which part the redeploy
    // changes: the delivery location, the input parameters, and both at once.
    let cases: [(&str, Value, Value); 3] = [
        (
            "the delivery location",
            json!({ "DeliveryS3Bucket": "first-bucket" }),
            json!({ "DeliveryS3Bucket": "second-bucket", "DeliveryS3KeyPrefix": "packs/next" }),
        ),
        (
            "the input parameters",
            json!({
                "ConformancePackInputParameters": [
                    { "ParameterName": "BucketName", "ParameterValue": "audit-logs" },
                ],
            }),
            json!({
                "ConformancePackInputParameters": [
                    { "ParameterName": "RetentionDays", "ParameterValue": "7" },
                ],
            }),
        ),
        (
            "both at once",
            json!({
                "DeliveryS3Bucket": "first-bucket",
                "ConformancePackInputParameters": [
                    { "ParameterName": "BucketName", "ParameterValue": "audit-logs" },
                ],
            }),
            json!({}),
        ),
    ];

    for (case, first, second) in cases {
        let emulator = Emulator::new();
        let name = "redeployed-pack";
        let arn = deploy(case, &emulator, name, &first).await;
        let before = emulator
            .ok("DescribeConformancePacks", &describe_of(&[name]))
            .await;
        let first_id = before["ConformancePackDetails"][0]["ConformancePackId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the pack carries an id, got {before}"))
            .to_string();

        // A pack is redeployed once its previous deployment has settled: while that one
        // is in flight the pack is in use (`conformance_packs` pins that refusal).
        tokio::time::sleep(past_the_deployment()).await;
        let redeployed_arn = deploy(case, &emulator, name, &second).await;
        let after = emulator
            .ok("DescribeConformancePacks", &describe_of(&[name]))
            .await;
        let reported = described(case, &after);
        let [detail] = reported.as_slice() else {
            panic!("case {case:?}: a redeploy leaves ONE pack of that name, got {after}");
        };

        assert_eq!(
            (
                detail["ConformancePackArn"].as_str(),
                detail["ConformancePackId"].as_str(),
                redeployed_arn.as_str(),
            ),
            (Some(arn.as_str()), Some(first_id.as_str()), arn.as_str()),
            "case {case:?}: a redeploy keeps the identity the pack was created with, got {detail}"
        );
        assert_eq!(
            (
                detail["DeliveryS3Bucket"].as_str(),
                detail["DeliveryS3KeyPrefix"].as_str(),
            ),
            (
                second["DeliveryS3Bucket"].as_str(),
                second["DeliveryS3KeyPrefix"].as_str(),
            ),
            "case {case:?}: the redeploy replaced the delivery location, got {detail}"
        );
        assert_eq!(
            detail["ConformancePackInputParameters"],
            second
                .get("ConformancePackInputParameters")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: the redeploy replaced the input parameters, got {detail}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_pack_reports_them_all_and_names_select_them() {
    // Rows differ along how the request selects packs: no names at all, one name,
    // several, names sent in an order other than the packs', and the same name twice.
    // The expected column is the names the response reports, in the order reported.
    let packs = ["alpha", "bravo", "charlie", "delta"];
    let cases: [(&str, &[&str], &[&str]); 5] = [
        ("no names at all", &[], &packs),
        ("one name", &["bravo"], &["bravo"]),
        ("several names", &["alpha", "charlie"], &["alpha", "charlie"]),
        (
            "names sent out of the packs' order",
            &["delta", "alpha"],
            &["alpha", "delta"],
        ),
        ("the same name twice", &["bravo", "bravo"], &["bravo"]),
    ];

    let emulator = Emulator::new();
    for name in packs {
        deploy("the shared packs", &emulator, name, &json!({})).await;
    }

    for (case, names, expected) in cases {
        let body = emulator
            .ok("DescribeConformancePacks", &describe_of(names))
            .await;
        assert_eq!(
            described_names(case, &body),
            expected.iter().map(|name| name.to_string()).collect::<Vec<String>>(),
            "case {case:?}: expected the packs {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn a_name_no_pack_exists_for_is_refused_naming_every_such_name() {
    // Rows differ along which of the names the request carried resolve to no pack: one,
    // one beside a name that does resolve, two of them, a name differing from a stored
    // one only in case, and two sent in the other order — the refusal lists them as the
    // request carried them.
    let cases: [(&str, &[&str], &str); 5] = [
        ("one name no pack exists for", &["ghost"], "ghost"),
        (
            "an unknown name beside a known one",
            &["alpha", "ghost"],
            "ghost",
        ),
        ("two unknown names", &["ghost", "phantom"], "ghost, phantom"),
        (
            "a name differing from a stored one only in case",
            &["Alpha"],
            "Alpha",
        ),
        (
            "the same two unknown names in the other order",
            &["phantom", "ghost"],
            "phantom, ghost",
        ),
    ];

    let emulator = Emulator::new();
    deploy("the stored pack", &emulator, "alpha", &json!({})).await;

    for (case, names, missing) in cases {
        let (status, parsed) = emulator
            .call("DescribeConformancePacks", &describe_of(names))
            .await;
        let code = parsed["__type"]
            .as_str()
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
            .unwrap_or("<none>");
        assert_eq!(
            (status, code),
            (400, "NoSuchConformancePackException"),
            "case {case:?}: expected 400 NoSuchConformancePackException, got {status} {parsed}"
        );
        assert_eq!(
            parsed["message"].as_str(),
            Some(format!("{NO_SUCH_PACK_SENTENCE}[{missing}]").as_str()),
            "case {case:?}: expected the refusal to name {missing:?}, got {parsed}"
        );
    }

    // The refusal leaves the packs that DO exist readable: it refused the request, it
    // did not change the account's packs.
    let body = emulator.ok("DescribeConformancePacks", &json!({})).await;
    assert_eq!(
        described_names("after the refusals", &body),
        vec!["alpha".to_string()],
        "a refused request must leave the stored packs as they were, got {body}"
    );
}

#[tokio::test]
async fn the_pack_details_page_over_the_packs_name_ordering() {
    // Rows differ along the page size asked for: one pack per page, several, more than
    // the account holds, and the modelled default of 0 (the maximum).
    let packs = ["alpha", "bravo", "charlie", "delta", "echo"];
    let cases: [(&str, i32, usize); 4] = [
        ("one pack per page", 1, 1),
        ("two packs per page", 2, 2),
        ("more than the account holds", 20, packs.len()),
        (
            "the modelled default",
            0,
            packs.len().min(DESCRIBE_CONFORMANCE_PACKS_PAGE_SIZE),
        ),
    ];

    let emulator = Emulator::new();
    for name in packs {
        deploy("the shared packs", &emulator, name, &json!({})).await;
    }

    for (case, limit, expected_page) in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({ "Limit": limit });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let body = emulator.ok("DescribeConformancePacks", &request).await;
            let page = described_names(case, &body);
            assert!(
                page.len() <= expected_page,
                "case {case:?}: a page carries at most {expected_page} packs, got {page:?}"
            );
            seen.extend(page);
            next_token = body["NextToken"].as_str().map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen,
            packs.map(str::to_string).to_vec(),
            "case {case:?}: paging reports every pack once, in the packs' name ordering, got \
             {seen:?}"
        );
    }

    // A token that is not in the opaque form this operation issues cannot be read as a
    // page: it is refused rather than taken for a resume point. (A token the operation
    // DID issue is exercised by the paging loop above, which walks the whole set through
    // the tokens it answered.)
    for (case, token) in [
        ("not base64 at all", "**not base64**"),
        (
            "a string outside the token codec's own base64 form",
            "bm90LWEtcGFjaw",
        ),
    ] {
        let (status, code) = emulator
            .refused("DescribeConformancePacks", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidNextTokenException"),
            "case {case:?}: expected 400 InvalidNextTokenException, got {status} {code}"
        );
    }

    // `Limit`'s modelled 0..20 range and `ConformancePackNamesList`'s modelled length of
    // at most 25 are enforced by the request boundary, BEFORE the handler runs, so what a
    // wire caller meets for either is the boundary's `ValidationException` — the rows
    // below pin that, not the handler's own modelled refusals, which stand for a request
    // that reaches it. Rows differ along which modelled constraint the request breaks:
    // the limit past its maximum, below its minimum, and the name list past its length.
    for (case, request) in [
        (
            "a limit past the modelled maximum",
            json!({ "Limit": DESCRIBE_CONFORMANCE_PACKS_PAGE_SIZE + 1 }),
        ),
        ("a limit below the modelled minimum", json!({ "Limit": -1 })),
        (
            "a name list past the modelled maximum",
            json!({
                "ConformancePackNames":
                    (0..26).map(|index| format!("pack-{index}")).collect::<Vec<String>>(),
            }),
        ),
    ] {
        let (status, code) = emulator.refused("DescribeConformancePacks", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the boundary's 400 ValidationException, got {status} {code}"
        );
    }
}
