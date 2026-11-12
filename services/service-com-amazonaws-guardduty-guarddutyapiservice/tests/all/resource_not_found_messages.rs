// SPDX-License-Identifier: CC0-1.0

//! The WORDS GuardDuty refuses an identifier that names no resource with, across
//! every family that addresses a resource by one.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch` (the
//! harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded call:
//!
//! 1. An operation handed an identifier that resolves to no resource answers the ONE
//!    fixed sentence the service words for THAT resource family and operation class,
//!    with nothing of the request interpolated into it: the uploaded lists (an IPSet,
//!    a threat list) and a filter READ say no such resource was found, the mutating
//!    filter paths say the given filter name is invalid, a publishing-destination read
//!    reports an unusable input parameter while a write says no such destination
//!    exists, a Malware Protection plan says the plan was not found, and a findings
//!    write says an invalid finding identifier was specified.
//! 2. The account-ownership sentence is the DETECTOR family's own refusal: it answers
//!    a `detectorId` this account and Region do not hold, and no other family's
//!    absence is reported as an ownership problem.
//! 3. Each refusal is a LOOKUP and not a constant: the same operation over the
//!    identifier the create minted succeeds and reports the resource back.
//!
//! The rows differ along what the rule generalises over: the resource FAMILY, the
//! OPERATION CLASS (read, update, delete), and the FORM of the identifier — one the
//! service could have minted but never did, one a delete removed, a stored one
//! upper-cased, and a value that could not be an identifier of that family at all.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, enabled_detector, error_code, error_shape, new_router, Router, ABSENT_DETECTOR_ID,
};

/// The modelled error SHAPE restJson1 resolves every 400 of this service to (the
/// `x-amzn-errortype` header).
const ENVELOPE: &str = "BadRequestException";

/// The error CLASS a 400 carries in the body's `__type` and `type`.
const INVALID_INPUT: &str = "InvalidInputException";

/// The modelled not-found error the Malware Protection plan operations declare
/// (`smithy_query(operation="GetMalwareProtectionPlan")`: `ResourceNotFoundException`,
/// `httpError` 404).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

// The fixed sentences under test, written out here rather than read from the service
// so that this suite pins the WORDS and not merely the service's agreement with
// itself.

/// What the uploaded-list families and a filter READ answer for an identifier that
/// names nothing.
const NO_SUCH_RESOURCE: &str = "The request is rejected since no such resource found.";

/// What the mutating filter paths answer for a name they cannot act on.
const INVALID_FILTER_NAME: &str = "The request is rejected because the given filter name is invalid.";

/// What a publishing-destination READ answers for a destination id that names nothing.
const INVALID_INPUT_PARAMETERS: &str =
    "The request is rejected because the one or more input parameters have invalid values.";

/// What a publishing-destination WRITE answers for the same.
const DESTINATION_ABSENT: &str =
    "The request failed because no publishingDestination exists with the provided publishingDestinationId.";

/// What the Malware Protection plan operations answer for a plan id this account and
/// Region do not hold — absent and foreign collapsed into one wording.
const PLAN_NOT_FOUND: &str =
    "The request was rejected because the specified malware protection plan was not found.";

/// What a findings write answers for a well-formed finding id the detector does not
/// hold.
const INVALID_FINDING_IDS: &str =
    "The request is rejected because an invalid finding identifiers is specified.";

/// The DETECTOR family's own refusal, which no other family's absence may borrow.
const DETECTOR_NOT_OWNED: &str =
    "The request is rejected because the input detectorId is not owned by the current account.";

/// The claim rule 2 forbids every non-detector refusal from making.
const OWNERSHIP_CLAIM: &str = "is not owned by the current account";

/// A value that cannot be an identifier of a family that mints 32 hexadecimal
/// characters.
const MALFORMED_ID: &str = "not-an-id-this-service-mints";

/// A well-formed identifier (32 hexadecimal characters, the form of every id this
/// service mints) that no create here ever minted.
const ABSENT_ID: &str = "0123456789abcdef0123456789abcdef";

/// A well-formed Malware Protection plan id (the 20 hexadecimal characters that
/// family's ids are spelled in) that no create here ever minted.
const ABSENT_PLAN_ID: &str = "0123456789abcdef0123";

/// A well-formed finding id no `CreateSampleFindings` minted.
const FOREIGN_FINDING_ID: &str = "ffffffffffffffffffffffffffffffff";

/// The collection URI of the Malware Protection plans
/// (`smithy_query(operation="CreateMalwareProtectionPlan")`).
const PLANS_PATH: &str = "/malware-protection-plan";

/// An IAM role ARN of the form `CreateMalwareProtectionPlan.Role` takes.
const PLAN_ROLE: &str = "arn:aws:iam::000000000000:role/GuardDutyMalwareProtectionRole";

/// The S3 ARN a publishing destination is created with.
const DESTINATION_ARN: &str = "arn:aws:s3:::guardduty-not-found-probe";

/// The KMS key ARN a publishing destination is created with.
const DESTINATION_KEY: &str =
    "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// One probe: a request whose identifier resolves to nothing, and the answer it must
/// get.
struct Row {
    /// What the row is, for every assertion message.
    case: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The HTTP status the operation's own model gives this refusal.
    status: u16,
    /// The error class the body carries.
    class: &'static str,
    /// The fixed sentence the refusal must carry, word for word.
    expected: &'static str,
    /// The identifier the request sent, which the refusal must not quote back.
    sent: String,
}

/// Create one resource of each family under `detector`, read each back to prove the
/// create landed, and return the identifiers: the IPSet, the threat list, the filter
/// name, the publishing destination, the plan and one finding.
struct Stored {
    ip_set: String,
    threat_intel_set: String,
    filter: String,
    destination: String,
    plan: String,
    finding: String,
}

/// A successful call's parsed body, failing the test when the call did not succeed.
async fn ok(router: &Router, case: &str, method: &str, path: &str, body: Option<Value>) -> Value {
    let (status, response) = call(router, method, path, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {response:?}"
    );
    response
}

/// The string at `member` of a successful create's body.
fn minted(case: &str, body: &Value, member: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {body:?}"))
        .to_string()
}

async fn store_one_of_each(router: &Router, detector: &str) -> Stored {
    let ip_set = minted(
        "the stored IPSet",
        &ok(
            router,
            "the stored IPSet",
            "POST",
            &format!("/detector/{detector}/ipset"),
            Some(json!({
                "name": "probe-list",
                "format": "TXT",
                "location": "s3://my-bucket/probe.txt",
                "activate": false,
            })),
        )
        .await,
        "ipSetId",
    );
    let threat_intel_set = minted(
        "the stored threat list",
        &ok(
            router,
            "the stored threat list",
            "POST",
            &format!("/detector/{detector}/threatintelset"),
            Some(json!({
                "name": "probe-threats",
                "format": "TXT",
                "location": "s3://my-bucket/threats.txt",
                "activate": false,
            })),
        )
        .await,
        "threatIntelSetId",
    );
    let filter = minted(
        "the stored filter",
        &ok(
            router,
            "the stored filter",
            "POST",
            &format!("/detector/{detector}/filter"),
            Some(json!({
                "name": "probe-filter",
                "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 2}}},
            })),
        )
        .await,
        "name",
    );
    let destination = minted(
        "the stored destination",
        &ok(
            router,
            "the stored destination",
            "POST",
            &format!("/detector/{detector}/publishingDestination"),
            Some(json!({
                "destinationType": "S3",
                "destinationProperties": {
                    "destinationArn": DESTINATION_ARN,
                    "kmsKeyArn": DESTINATION_KEY,
                },
            })),
        )
        .await,
        "destinationId",
    );
    let plan = minted(
        "the stored plan",
        &ok(
            router,
            "the stored plan",
            "POST",
            PLANS_PATH,
            Some(json!({
                "role": PLAN_ROLE,
                "protectedResource": {"s3Bucket": {"bucketName": "guardduty-probe-bucket"}},
            })),
        )
        .await,
        "malwareProtectionPlanId",
    );

    // One finding to archive, so the findings rows refuse an id the detector does not
    // hold rather than an empty detector.
    ok(
        router,
        "the sample findings",
        "POST",
        &format!("/detector/{detector}/findings/create"),
        Some(json!({"findingTypes": ["Backdoor:EC2/DenialOfService.Tcp"]})),
    )
    .await;
    let listed = ok(
        router,
        "the detector's findings",
        "POST",
        &format!("/detector/{detector}/findings"),
        Some(json!({"maxResults": 50})),
    )
    .await;
    let finding = listed["findingIds"][0]
        .as_str()
        .unwrap_or_else(|| panic!("case \"the detector's findings\": expected an id, got {listed:?}"))
        .to_string();

    // Rule 3: each create landed and the matching read reports it back, so every
    // refusal below is a lookup over the store and not a constant.
    let read = ok(
        router,
        "read back the IPSet",
        "GET",
        &format!("/detector/{detector}/ipset/{ip_set}"),
        None,
    )
    .await;
    assert_eq!(
        read["name"].as_str(),
        Some("probe-list"),
        "case \"read back the IPSet\": expected the stored name, got {read:?}"
    );
    let read = ok(
        router,
        "read back the threat list",
        "GET",
        &format!("/detector/{detector}/threatintelset/{threat_intel_set}"),
        None,
    )
    .await;
    assert_eq!(
        read["name"].as_str(),
        Some("probe-threats"),
        "case \"read back the threat list\": expected the stored name, got {read:?}"
    );
    let read = ok(
        router,
        "read back the filter",
        "GET",
        &format!("/detector/{detector}/filter/{filter}"),
        None,
    )
    .await;
    assert_eq!(
        read["name"].as_str(),
        Some(filter.as_str()),
        "case \"read back the filter\": expected the stored name, got {read:?}"
    );
    let read = ok(
        router,
        "read back the destination",
        "GET",
        &format!("/detector/{detector}/publishingDestination/{destination}"),
        None,
    )
    .await;
    assert_eq!(
        read["destinationId"].as_str(),
        Some(destination.as_str()),
        "case \"read back the destination\": expected the stored id, got {read:?}"
    );
    let read = ok(
        router,
        "read back the plan",
        "GET",
        &format!("{PLANS_PATH}/{plan}"),
        None,
    )
    .await;
    assert_eq!(
        read["role"].as_str(),
        Some(PLAN_ROLE),
        "case \"read back the plan\": expected the stored role, got {read:?}"
    );
    let archived = ok(
        router,
        "archive the held finding",
        "POST",
        &format!("/detector/{detector}/findings/archive"),
        Some(json!({"findingIds": [finding.clone()]})),
    )
    .await;
    assert!(
        archived.is_null() || archived.as_object().is_some_and(serde_json::Map::is_empty),
        "case \"archive the held finding\": expected an empty response, got {archived:?}"
    );

    Stored {
        ip_set,
        threat_intel_set,
        filter,
        destination,
        plan,
        finding,
    }
}

/// Assert one refusal: the status its model gives it, the error class, the fixed
/// sentence word for word, no echo of the identifier the request sent, and — for every
/// family but the detector — no ownership claim.
fn assert_refusal(row: &Row, status: u16, body: &Value) {
    let case = &row.case;
    assert_eq!(
        status, row.status,
        "case {case:?}: expected HTTP {}, got {status} {body:?}",
        row.status
    );
    assert_eq!(
        error_code(body),
        row.class,
        "case {case:?}: expected the {} class, got {body:?}",
        row.class
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, row.expected,
        "case {case:?}: expected message {:?}, got {message:?}",
        row.expected
    );
    assert!(
        row.sent.is_empty() || !message.contains(&row.sent),
        "case {case:?}: the refusal must not quote {:?} back, got {message:?}",
        row.sent
    );
    assert!(
        row.expected == DETECTOR_NOT_OWNED || !message.contains(OWNERSHIP_CLAIM),
        "case {case:?}: only a detectorId refusal may claim an ownership problem, got {message:?}"
    );
}

/// Every family answers its own fixed not-found sentence, for every form of
/// identifier that resolves to nothing, and never the detector family's ownership
/// claim.
#[tokio::test]
async fn each_family_words_its_own_absent_resource_refusal() {
    let router = new_router();
    let detector = enabled_detector(&router).await;
    let stored = store_one_of_each(&router, &detector).await;

    // A filter and a destination the caller then removed: a delete is what makes an
    // identifier a client held a moment ago resolve to nothing, which is the case the
    // lifecycle probe "did my delete take effect?" asks about.
    ok(
        &router,
        "delete the stored filter",
        "DELETE",
        &format!("/detector/{detector}/filter/{}", stored.filter),
        None,
    )
    .await;
    ok(
        &router,
        "delete the stored destination",
        "DELETE",
        &format!(
            "/detector/{detector}/publishingDestination/{}",
            stored.destination
        ),
        None,
    )
    .await;
    ok(
        &router,
        "delete the stored plan",
        "DELETE",
        &format!("{PLANS_PATH}/{}", stored.plan),
        None,
    )
    .await;
    // A deleted IPSet and threat list are RETAINED so a read can still report their
    // `DELETED` status, and a change to one collides with the delete already running
    // on it rather than naming an absent resource (the `ip_set_update_and_delete`
    // suite's rule), so the uploaded-list rows below are about identifiers that
    // resolve to NOTHING: ids no create minted, and a stored id in a spelling the
    // store does not hold.

    let mut rows: Vec<Row> = Vec::new();

    // The uploaded-list families: one sentence for the read, the update and the
    // delete, and for every form of the id.
    for (family, segment, id, why) in [
        ("ipset", "ipset", ABSENT_ID.to_string(), "an id no create minted"),
        ("ipset", "ipset", MALFORMED_ID.to_string(), "a value that is not an id at all"),
        (
            "ipset",
            "ipset",
            stored.ip_set.to_uppercase(),
            "the stored id upper-cased",
        ),
        (
            "threatintelset",
            "threatintelset",
            ABSENT_ID.to_string(),
            "an id no create minted",
        ),
        (
            "threatintelset",
            "threatintelset",
            MALFORMED_ID.to_string(),
            "a value that is not an id at all",
        ),
        (
            "threatintelset",
            "threatintelset",
            stored.threat_intel_set.to_uppercase(),
            "the stored id upper-cased",
        ),
    ] {
        rows.push(Row {
            case: format!("{family} read: {why}"),
            method: "GET",
            path: format!("/detector/{detector}/{segment}/{id}"),
            body: None,
            status: 400,
            class: INVALID_INPUT,
            expected: NO_SUCH_RESOURCE,
            sent: id.clone(),
        });
        rows.push(Row {
            case: format!("{family} delete: {why}"),
            method: "DELETE",
            path: format!("/detector/{detector}/{segment}/{id}"),
            body: None,
            status: 400,
            class: INVALID_INPUT,
            expected: NO_SUCH_RESOURCE,
            sent: id.clone(),
        });
        rows.push(Row {
            case: format!("{family} update: {why}"),
            method: "POST",
            path: format!("/detector/{detector}/{segment}/{id}"),
            body: Some(json!({"name": "must-not-land"})),
            status: 400,
            class: INVALID_INPUT,
            expected: NO_SUCH_RESOURCE,
            sent: id.clone(),
        });
    }

    // The filter family: the READ says no such resource, the two WRITES say the name
    // itself is not one they can act on.
    for (name, why) in [
        (stored.filter.clone(), "a name whose delete was accepted"),
        ("never-saved-filter".to_string(), "a name no create saved"),
        ("ab".to_string(), "a name under the modelled minimum length"),
        ("not!a!filter!name".to_string(), "a name outside the documented alphabet"),
    ] {
        rows.push(Row {
            case: format!("filter read: {why}"),
            method: "GET",
            path: format!("/detector/{detector}/filter/{name}"),
            body: None,
            status: 400,
            class: INVALID_INPUT,
            expected: NO_SUCH_RESOURCE,
            sent: name.clone(),
        });
        rows.push(Row {
            case: format!("filter update: {why}"),
            method: "POST",
            path: format!("/detector/{detector}/filter/{name}"),
            body: Some(json!({"rank": 7})),
            status: 400,
            class: INVALID_INPUT,
            expected: INVALID_FILTER_NAME,
            sent: name.clone(),
        });
        rows.push(Row {
            case: format!("filter delete: {why}"),
            method: "DELETE",
            path: format!("/detector/{detector}/filter/{name}"),
            body: None,
            status: 400,
            class: INVALID_INPUT,
            expected: INVALID_FILTER_NAME,
            sent: name.clone(),
        });
    }

    // The publishing-destination family: the READ reports an unusable parameter, the
    // WRITES report a destination that does not exist.
    for (id, why) in [
        (stored.destination.clone(), "an id whose delete was accepted"),
        (ABSENT_ID.to_string(), "an id no create minted"),
        (MALFORMED_ID.to_string(), "a value that is not an id at all"),
    ] {
        rows.push(Row {
            case: format!("publishing destination describe: {why}"),
            method: "GET",
            path: format!("/detector/{detector}/publishingDestination/{id}"),
            body: None,
            status: 400,
            class: INVALID_INPUT,
            expected: INVALID_INPUT_PARAMETERS,
            sent: id.clone(),
        });
        rows.push(Row {
            case: format!("publishing destination update: {why}"),
            method: "POST",
            path: format!("/detector/{detector}/publishingDestination/{id}"),
            body: Some(json!({
                "destinationProperties": {"destinationArn": "arn:aws:s3:::elsewhere"}
            })),
            status: 400,
            class: INVALID_INPUT,
            expected: DESTINATION_ABSENT,
            sent: id.clone(),
        });
        rows.push(Row {
            case: format!("publishing destination delete: {why}"),
            method: "DELETE",
            path: format!("/detector/{detector}/publishingDestination/{id}"),
            body: None,
            status: 400,
            class: INVALID_INPUT,
            expected: DESTINATION_ABSENT,
            sent: id.clone(),
        });
    }

    // The Malware Protection plan family: absent and foreign are ONE not-found
    // wording, under the modelled 404 its operations declare.
    for (id, why) in [
        (stored.plan.clone(), "an id whose delete was accepted"),
        (ABSENT_PLAN_ID.to_string(), "an id no create minted"),
        (stored.plan.to_uppercase(), "the stored id upper-cased"),
    ] {
        rows.push(Row {
            case: format!("malware protection plan read: {why}"),
            method: "GET",
            path: format!("{PLANS_PATH}/{id}"),
            body: None,
            status: 404,
            class: RESOURCE_NOT_FOUND,
            expected: PLAN_NOT_FOUND,
            sent: id.clone(),
        });
        rows.push(Row {
            case: format!("malware protection plan update: {why}"),
            method: "PATCH",
            path: format!("{PLANS_PATH}/{id}"),
            body: Some(json!({"role": PLAN_ROLE})),
            status: 404,
            class: RESOURCE_NOT_FOUND,
            expected: PLAN_NOT_FOUND,
            sent: id.clone(),
        });
        rows.push(Row {
            case: format!("malware protection plan delete: {why}"),
            method: "DELETE",
            path: format!("{PLANS_PATH}/{id}"),
            body: None,
            status: 404,
            class: RESOURCE_NOT_FOUND,
            expected: PLAN_NOT_FOUND,
            sent: id.clone(),
        });
    }

    // The findings writes: a well-formed finding id the detector does not hold is the
    // finding-identifier refusal, alone and beside an id it does hold.
    for (ids, why) in [
        (
            vec![FOREIGN_FINDING_ID.to_string()],
            "a finding id the detector does not hold, alone",
        ),
        (
            vec![stored.finding.clone(), FOREIGN_FINDING_ID.to_string()],
            "a finding id the detector does not hold, after a held one",
        ),
        (
            vec![stored.finding.to_uppercase()],
            "a held finding id upper-cased",
        ),
    ] {
        for direction in ["archive", "unarchive"] {
            rows.push(Row {
                case: format!("findings {direction}: {why}"),
                method: "POST",
                path: format!("/detector/{detector}/findings/{direction}"),
                body: Some(json!({"findingIds": ids.clone()})),
                status: 400,
                class: INVALID_INPUT,
                expected: INVALID_FINDING_IDS,
                sent: FOREIGN_FINDING_ID.to_string(),
            });
        }
    }

    // Rule 2: the ownership sentence is the DETECTOR family's own, so the same
    // operations answer it when it is the detector that resolves to nothing.
    for (case, method, path, body) in [
        (
            "detector read of an IPSet under a detector nothing minted",
            "GET",
            format!("/detector/{ABSENT_DETECTOR_ID}/ipset/{ABSENT_ID}"),
            None,
        ),
        (
            "detector read of a filter under a detector nothing minted",
            "GET",
            format!("/detector/{ABSENT_DETECTOR_ID}/filter/never-saved-filter"),
            None,
        ),
        (
            "detector archive under a detector nothing minted",
            "POST",
            format!("/detector/{ABSENT_DETECTOR_ID}/findings/archive"),
            Some(json!({"findingIds": [FOREIGN_FINDING_ID]})),
        ),
    ] {
        rows.push(Row {
            case: case.to_string(),
            method,
            path,
            body,
            status: 400,
            class: INVALID_INPUT,
            expected: DETECTOR_NOT_OWNED,
            sent: ABSENT_DETECTOR_ID.to_string(),
        });
    }

    for row in &rows {
        let (status, body) = call(&router, row.method, &row.path, row.body.clone()).await;
        assert_refusal(row, status, &body);
        if row.status == 400 {
            assert_eq!(
                error_shape(&body),
                ENVELOPE,
                "case {:?}: expected the {ENVELOPE} envelope shape, got {body:?}",
                row.case
            );
        }
    }

    // And the detector itself is still there, so none of those refusals was the
    // detector's own absence in disguise.
    let read = ok(
        &router,
        "the detector after the refusals",
        "GET",
        &format!("/detector/{detector}"),
        None,
    )
    .await;
    assert_eq!(
        read["status"].as_str(),
        Some("ENABLED"),
        "case \"the detector after the refusals\": expected the detector still enabled, got {read:?}"
    );
}
