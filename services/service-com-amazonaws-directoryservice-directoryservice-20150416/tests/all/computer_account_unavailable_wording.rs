// SPDX-License-Identifier: CC0-1.0

//! How a computer-account write is refused, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`), so nothing here reaches into
//! the state struct and an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each over a TABLE of inputs rather than one call:
//!
//! * a computer account is created in the directory's own domain, and a directory that is
//!   still being built has no domain to create it in — so `CreateComputer` against one is
//!   refused with the modelled `DirectoryUnavailableException` worded in the directory-LOOKUP
//!   family's sentence, the quoted id the request sent and that no such directory is there,
//!   with the request-id suffix every refusal of this service carries and that same id in the
//!   response's `x-amzn-requestid` header. The class is what separates this failure from an
//!   id that resolves to no directory at all, which is answered with
//!   `EntityDoesNotExistException` in the same sentence. The family-NEUTRAL sentence
//!   (`Operation could not be performed because directory <id> is unavailable.`) belongs to
//!   the operations whose family states none of its own, which
//!   `tests/all/refusal_wording.rs` is what asserts;
//! * an account name the directory already holds is refused with the modelled
//!   `EntityAlreadyExistsException` whose diagnostic is FIXED text closing with the call's
//!   own request id — it names neither the directory nor the account, so nothing of the
//!   account the directory holds is disclosed.

use std::time::Duration;

use serde_json::{json, Value};

use crate::directory_lifecycle::{
    create_directory, describe_one, minimal_create, PAST_ONE_SPAN, PAST_ONE_SPAN_ONLY,
    STAGE_CREATING, STAGE_REQUESTED,
};
use crate::harness::{call_raw, service};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The header AWS reports a response's own request id in, and where an SDK's response
/// metadata reads a failed call's request id from.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// A `DirectoryId` the modelled `^d-[0-9a-f]{10}$` admits and no case here creates, so the
/// boundary passes it and the handler has to resolve it in the store.
const ABSENT_DIRECTORY_ID: &str = "d-0123456789";

/// A `ComputerName` within the modelled `length 1..15`.
const COMPUTER_NAME: &str = "early";

/// A one-time join `Password` within the modelled `length 8..64` that meets the directory's
/// own password policy.
const JOIN_PASSWORD: &str = "Cmp-1-Time!Pw";

/// One row: a directory type, how long after its create the call is made, and the stage the
/// directory is then in.
struct StageCase {
    case: &'static str,
    /// Whether the row's directory is a Managed Microsoft AD one, rather than the Simple AD
    /// the other rows use: the refusal is the computer-account family's either way.
    microsoft_ad: bool,
    /// How long to wait after the create before the computer-account write — zero for the
    /// first stage of the chain the create owes, one transition span for the next.
    wait: Duration,
    stage: &'static str,
}

/// The `CreateComputer` body for `directory_id`.
fn create_computer(directory_id: &str) -> Value {
    json!({
        "DirectoryId": directory_id,
        "ComputerName": COMPUTER_NAME,
        "Password": JOIN_PASSWORD
    })
}

/// The sentence this service states for a directory the request named and the service does
/// not have, with the request id the answer itself reported appended.
fn no_such_directory(directory_id: &str, request_id: &str) -> String {
    format!("Directory: '{directory_id}' does not exist. : RequestId: {request_id}")
}

/// One `CreateComputer` call, returning `(status, error code, message, the id the header
/// carried)` — read from the WHOLE response, so the header the rules name is asserted on
/// rather than assumed. A success answers 200 with no code and no message.
async fn answered(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
    case: &str,
) -> (u16, String, String, Option<String>) {
    let response = call_raw(router, "CreateComputer", body).await;
    let status = response.status().as_u16();
    let header = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .map(|value| value.to_str().expect("a textual header").to_string());
    let payload = response.into_body();
    let parsed: Value = serde_json::from_slice(&payload)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {payload:?}"));
    let code = parsed["__type"].as_str().unwrap_or_default().to_string();
    let message = parsed["Message"].as_str().unwrap_or_default().to_string();
    (status, code, message, header)
}

/// RULE: `CreateComputer` on a directory that is not in service is refused
/// `DirectoryUnavailableException` in the directory-lookup family's sentence, and the same
/// call is admitted once the stage chain its create owed has run out.
#[tokio::test]
async fn a_computer_account_write_on_a_directory_being_built_is_refused_as_no_such_directory() {
    // The rows differ along the directory's type and along how far through the stage chain
    // its create owes the call arrives, so the refusal is the family's rather than one
    // type's or one stage's.
    let rows: [StageCase; 4] = [
        StageCase {
            case: "a Simple AD directory just requested",
            microsoft_ad: false,
            wait: Duration::ZERO,
            stage: STAGE_REQUESTED,
        },
        StageCase {
            case: "a Simple AD directory being created",
            microsoft_ad: false,
            wait: PAST_ONE_SPAN_ONLY,
            stage: STAGE_CREATING,
        },
        StageCase {
            case: "a Managed Microsoft AD directory just requested",
            microsoft_ad: true,
            wait: Duration::ZERO,
            stage: STAGE_REQUESTED,
        },
        StageCase {
            case: "a Managed Microsoft AD directory being created",
            microsoft_ad: true,
            wait: PAST_ONE_SPAN_ONLY,
            stage: STAGE_CREATING,
        },
    ];

    for row in &rows {
        // A fresh router per row: the window before the next transition falls due is what
        // the row exercises, so each row starts its own directory's clock.
        let router = service();
        let directory_id = if row.microsoft_ad {
            create_microsoft_ad(
                &router,
                minimal_microsoft_ad("pending.corp.example.com"),
                row.case,
            )
            .await
        } else {
            create_directory(&router, minimal_create("pending.example.com"), row.case).await
        };
        if !row.wait.is_zero() {
            tokio::time::sleep(row.wait).await;
        }

        // The stage the row is about, read back through the reporting operation.
        let described = describe_one(&router, &directory_id, row.case).await;
        assert_eq!(
            described["Stage"],
            json!(row.stage),
            "case {:?}: expected the directory in {}, got {described}",
            row.case,
            row.stage
        );

        let (status, code, message, header) =
            answered(&router, create_computer(&directory_id), row.case).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} with {message:?}",
            row.case
        );
        assert!(
            code.contains("DirectoryUnavailableException"),
            "case {:?}: expected DirectoryUnavailableException, got {code:?}",
            row.case
        );
        let reported = header.clone().unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the {REQUEST_ID_HEADER} header, got none",
                row.case
            )
        });
        assert_eq!(
            message,
            no_such_directory(&directory_id, &reported),
            "case {:?}: expected the lookup sentence for the id the request sent, got \
             {message:?}",
            row.case
        );

        // The id that resolves to NO directory is answered in the same sentence under the
        // other class, so what tells a caller the two apart is the error the SDK types —
        // and neither sentence says where in its build a directory is.
        let (absent_status, absent_code, absent_message, absent_header) =
            answered(&router, create_computer(ABSENT_DIRECTORY_ID), row.case).await;
        assert_eq!(
            absent_status, 400,
            "case {:?}: expected 400 for an absent id, got {absent_status} with {absent_message:?}",
            row.case
        );
        assert!(
            absent_code.contains("EntityDoesNotExistException"),
            "case {:?}: expected EntityDoesNotExistException for an absent id, got \
             {absent_code:?}",
            row.case
        );
        let absent_reported = absent_header.unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the {REQUEST_ID_HEADER} header for an absent id, got none",
                row.case
            )
        });
        assert_eq!(
            absent_message,
            no_such_directory(ABSENT_DIRECTORY_ID, &absent_reported),
            "case {:?}: expected the lookup sentence for an absent id, got {absent_message:?}",
            row.case
        );
        assert_ne!(
            absent_reported, reported,
            "case {:?}: expected each refusal to report its own request id, got \
             {absent_reported} twice",
            row.case
        );

        // The refusal was about the directory not being in service, so once the stage chain
        // its create owed has run out the very same call is admitted — and the account it
        // creates is reported back.
        tokio::time::sleep(PAST_ONE_SPAN).await;
        let response = call_raw(&router, "CreateComputer", create_computer(&directory_id)).await;
        let admitted = response.status().as_u16();
        let payload = response.into_body();
        let parsed: Value = serde_json::from_slice(&payload)
            .unwrap_or_else(|_| panic!("case {:?}: expected a JSON body", row.case));
        assert_eq!(
            admitted, 200,
            "case {:?}: expected the same call to be admitted once the directory is in \
             service, got {admitted} with {parsed}",
            row.case
        );
        assert_eq!(
            parsed["Computer"]["ComputerName"],
            json!(COMPUTER_NAME),
            "case {:?}: expected the created account to be reported, got {parsed}",
            row.case
        );
    }
}

/// The sentence this service states for an account the directory already holds: fixed text
/// closing with the call's own request id, after which the envelope's suffix repeats it.
///
/// THE one copy of that expectation in this crate's tests: the sibling suite
/// `computer_conflict_diagnostic`, which asserts the same sentence over the directory, the
/// unit and the attributes that collided, states it through this.
pub(crate) fn could_not_create_computer(request_id: &str) -> String {
    format!("Could not create computer. RequestId: {request_id} : RequestId: {request_id}")
}

/// One `CreateComputer` that must succeed, for a name a later row collides with.
async fn create_account(
    router: &smithy_server_runtime::MultiServiceRouter,
    directory_id: &str,
    computer_name: &str,
    case: &str,
) {
    let (status, code, message, _) = answered(
        router,
        json!({
            "DirectoryId": directory_id,
            "ComputerName": computer_name,
            "Password": JOIN_PASSWORD
        }),
        case,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the first create of {computer_name} to be admitted, got \
         {status} with {code:?} {message:?}"
    );
}

/// RULE: an account name the directory already holds is refused with the modelled
/// `EntityAlreadyExistsException` whose diagnostic is the fixed sentence closing with this
/// call's request id, and names neither the directory nor the account.
#[tokio::test]
async fn a_computer_account_the_directory_already_holds_is_refused_without_naming_it() {
    let router = service();
    let directory_id = create_directory(
        &router,
        minimal_create("accounts.example.com"),
        "the duplicates",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // (case, the name created first, the name the second create sends)
    //
    // The rows differ along the name: the same spelling, the other case (Active Directory
    // compares account names case-insensitively, so both name one account), the shortest
    // name the modelled `length 1..15` admits and the longest.
    let rows: [(&str, &str, &str); 4] = [
        ("the same name", "dup-plain", "dup-plain"),
        ("the same name in upper case", "dup-case", "DUP-CASE"),
        ("the shortest name the model admits", "z", "z"),
        (
            "the longest name the model admits",
            "fifteencharsok1",
            "fifteencharsok1",
        ),
    ];

    for (case, created, sent) in rows {
        create_account(&router, &directory_id, created, case).await;
        let (status, code, message, header) = answered(
            &router,
            json!({
                "DirectoryId": &directory_id,
                "ComputerName": sent,
                "Password": JOIN_PASSWORD
            }),
            case,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second create to be refused, got {status} with \
             {message:?}"
        );
        assert!(
            code.contains("EntityAlreadyExistsException"),
            "case {case:?}: expected EntityAlreadyExistsException, got {code:?}"
        );
        let reported = header.unwrap_or_else(|| {
            panic!("case {case:?}: expected the {REQUEST_ID_HEADER} header, got none")
        });
        assert_eq!(
            message,
            could_not_create_computer(&reported),
            "case {case:?}: expected the fixed conflict sentence, got {message:?}"
        );
        // Nothing of what collided is disclosed: neither the directory nor either spelling
        // of the account name appears in the diagnostic.
        for withheld in [directory_id.as_str(), created, sent] {
            assert!(
                !message.contains(withheld),
                "case {case:?}: expected the diagnostic to withhold {withheld:?}, got \
                 {message:?}"
            );
        }
    }
}
