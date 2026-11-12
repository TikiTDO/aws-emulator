// SPDX-License-Identifier: CC0-1.0

//! The stage chain a directory create owes, the operations that chain gates, and the
//! password policy a create's administrator account is made under — driven end to end over
//! HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, and reads the directory back through
//! `DescribeDirectories`. Nothing reaches into the state struct, so a stage that is not
//! computed from persisted state cannot pass.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call:
//!
//! * the chain `Requested` -> `Creating` -> `Active` over a table of creates spanning
//!   every directory type this service creates and both Managed Microsoft AD editions;
//! * the stage gate over a table of the operations a directory that is still being built
//!   refuses, each with the error class and the sentence its own family states, asserted
//!   from BOTH transient stages so the stage in the message is the one the store holds;
//! * the administrator password policy over a table of passwords spanning the word the
//!   policy forbids in upper, lower and mixed case, inside a longer word, and absent,
//!   against both creates that make an administrator account.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ad_connector::minimal_connect;
use crate::certificates::certificate_pem;
use crate::directory_lifecycle::{
    describe_one, descriptions, minimal_create, ok, service, string, PASSWORD, PAST_ONE_SPAN,
    STAGE_ACTIVE, STAGE_CREATING, STAGE_REQUESTED,
};
use crate::microsoft_ad::minimal_microsoft_ad;

/// The separator `state::refusal` puts between a diagnostic and the request id it appends.
const REQUEST_ID_SEPARATOR: &str = " : RequestId: ";

/// Long enough to be past the first transition a create owes and comfortably short of the
/// build window that follows it, so a read lands inside the stage the build happens in.
///
/// Shared with the sibling suite `certificate_writes_during_a_build`, so both land in that
/// stage the one way.
pub(crate) const INSIDE_THE_BUILD: Duration = Duration::from_millis(1200);

/// The `DirectoryUnavailableException` sentence this service states for a directory a call
/// cannot reach (`state::directory_unavailable`): fixed text around the `DirectoryId` the
/// request named, which is its only variable part.
///
/// Shared with the sibling suite `certificate_writes_during_a_build`, so both expect that
/// class's sentence from the one place.
pub(crate) fn directory_unavailable(directory_id: &str) -> String {
    format!("Operation could not be performed because directory {directory_id} is unavailable.")
}

/// The administrator account a Managed Microsoft AD directory is created with, from
/// `CreateMicrosoftADRequest$Password`'s own documentation: "The password for the default
/// administrative user named `Admin`".
const MICROSOFT_AD_ADMINISTRATOR: &str = "Admin";

/// A password the modelled `Password` pattern admits and the directory's own policy
/// accepts: 8–64 characters from all four character classes, naming no account.
const GOOD_PASSWORD: &str = "Sim2Real-Build-Pw1!";

/// The `Type` of LDAPS the modelled `LDAPSType` enum declares
/// (`smithy_query(query="shape",shape="LDAPSType")`): `Client`.
const LDAPS_TYPE_CLIENT: &str = "Client";

/// The message of a refusal, with the request id [`REQUEST_ID_SEPARATOR`] introduces
/// stripped: the diagnostic the service stated, and nothing per-run.
fn diagnostic(body: &Value, case: &str) -> String {
    let message = body["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Message, got {body}"));
    message
        .split_once(REQUEST_ID_SEPARATOR)
        .map_or(message, |(text, _)| text)
        .to_string()
}

/// A call that must be refused with HTTP 400, the given modelled error code AND the given
/// diagnostic, so a client that branches on either reads what AWS states for the class.
///
/// Shared with the sibling suite `certificate_writes_during_a_build`, so both assert a
/// refusal the one way.
pub(crate) async fn refused_with(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    expected_code: &str,
    expected_message: &str,
    case: &str,
) {
    let (status, parsed) = crate::directory_lifecycle::call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    let code = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        code.contains(expected_code),
        "case {case:?}: expected {expected_code} from {operation}, got {parsed}"
    );
    let text = diagnostic(&parsed, case);
    assert_eq!(
        text, expected_message,
        "case {case:?}: expected the sentence {expected_message:?} from {operation}, got \
         {text:?}"
    );
}

/// The `Stage` of the one description of `directory_id`.
async fn stage(router: &MultiServiceRouter, directory_id: &str, case: &str) -> String {
    let description = describe_one(router, directory_id, case).await;
    string(&description, "Stage", case)
}

/// RULE: a create answers before the directory exists, so the stage it owes is a chain —
/// the request is `Requested`, the directory is then `Creating` for as long as its servers
/// take to come up, and it is `Active` only once they are — and every directory type this
/// service creates goes through the same chain.
#[tokio::test]
async fn a_create_drives_the_directory_through_requested_creating_and_active() {
    let router = service();

    // The rows span the types this service creates — Simple AD at both of its sizes, an AD
    // Connector, and Managed Microsoft AD at both of its editions — so the chain is a rule
    // about a directory build rather than about one create.
    let rows: [(&str, &str, Value); 5] = [
        (
            "Simple AD, Small",
            "CreateDirectory",
            minimal_create("small.example.com"),
        ),
        (
            "Simple AD, Large",
            "CreateDirectory",
            json!({ "Name": "large.example.com", "Password": PASSWORD, "Size": "Large" }),
        ),
        (
            "AD Connector",
            "ConnectDirectory",
            minimal_connect("connected.example.com"),
        ),
        (
            "Managed Microsoft AD, default edition",
            "CreateMicrosoftAD",
            minimal_microsoft_ad("managed.example.com"),
        ),
        (
            "Managed Microsoft AD, Standard",
            "CreateMicrosoftAD",
            {
                let mut body = minimal_microsoft_ad("standard.example.com");
                body["Edition"] = json!("Standard");
                body
            },
        ),
    ];

    // Every create first, so the three milestones below are read for all of them inside
    // one chain rather than one chain per row.
    let mut created: Vec<(&str, String)> = Vec::new();
    for (case, operation, body) in rows {
        let answer = ok(&router, operation, body, case).await;
        created.push((case, string(&answer, "DirectoryId", case)));
    }

    // A caller acting straight after the create sees the stage the chain opens with.
    let mut opened: Vec<(&str, String, f64)> = Vec::new();
    for (case, id) in &created {
        let description = describe_one(&router, id, case).await;
        assert_eq!(
            description["Stage"],
            json!(STAGE_REQUESTED),
            "case {case:?}: expected the stage the chain opens with straight after the \
             create, got {description}"
        );
        let updated = description["StageLastUpdatedDateTime"]
            .as_f64()
            .unwrap_or_default();
        opened.push((case, id.clone(), updated));
    }

    // The build the create started is what the next stage reports, and it is reached
    // without anybody writing.
    tokio::time::sleep(INSIDE_THE_BUILD).await;
    for (case, id, opened_at) in &opened {
        let description = describe_one(&router, id, case).await;
        assert_eq!(
            description["Stage"],
            json!(STAGE_CREATING),
            "case {case:?}: expected the build stage once the first transition fell due, \
             got {description}"
        );
        let updated = description["StageLastUpdatedDateTime"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            updated >= *opened_at,
            "case {case:?}: expected StageLastUpdatedDateTime ({updated}) at or after the \
             stage it left ({opened_at})"
        );
        // Reading the stage does not move it on: the clock does.
        let again = stage(&router, id, case).await;
        assert_eq!(
            again, STAGE_CREATING,
            "case {case:?}: expected a second read to report the same stage, got {again}"
        );
    }

    // And once the build window has elapsed, the directory is in service.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (case, id) in &created {
        let settled = stage(&router, id, case).await;
        assert_eq!(
            settled, STAGE_ACTIVE,
            "case {case:?}: expected the stage the chain ends on, got {settled}"
        );
    }
}

/// RULE: a Managed Microsoft AD directory is the domain controllers it is made of, so it
/// reports `Active` no earlier than they do — they are `Creating` for as long as the
/// directory is being built.
#[tokio::test]
async fn the_directory_is_active_no_earlier_than_the_controllers_it_is_made_of() {
    let router = service();
    let case = "the controllers of a build";
    let answer = ok(
        &router,
        "CreateMicrosoftAD",
        minimal_microsoft_ad("controllers.example.com"),
        case,
    )
    .await;
    let id = string(&answer, "DirectoryId", case);

    // (the moment to read at, the stage the directory is in, the status its controllers
    // are in) — the two transient stages of the chain, then the one it ends on.
    for (wait, directory_stage, controller_status) in [
        (Duration::ZERO, STAGE_REQUESTED, "Creating"),
        (INSIDE_THE_BUILD, STAGE_CREATING, "Creating"),
        (PAST_ONE_SPAN, STAGE_ACTIVE, "Active"),
    ] {
        tokio::time::sleep(wait).await;
        let reported = stage(&router, &id, case).await;
        assert_eq!(
            reported, directory_stage,
            "case {case:?}: expected the directory in {directory_stage}, got {reported}"
        );
        let listed = ok(
            &router,
            "DescribeDomainControllers",
            json!({ "DirectoryId": &id }),
            case,
        )
        .await;
        let controllers = listed["DomainControllers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected DomainControllers, got {listed}"))
            .clone();
        assert!(
            !controllers.is_empty(),
            "case {case:?}: expected the directory's controllers to be placed, got {listed}"
        );
        for controller in &controllers {
            assert_eq!(
                controller["Status"],
                json!(controller_status),
                "case {case:?}: expected a controller in {controller_status} while the \
                 directory is {directory_stage}, got {controller}"
            );
        }
    }
}

/// RULE: an operation whose subject is a directory already built is refused while the
/// directory is still being built, under the error class and in the sentence its own family
/// states — and the stage that sentence names is the one the store holds.
#[tokio::test]
async fn the_build_stages_gate_the_operations_that_need_a_directory_in_service() {
    let router = service();
    let case = "the stage gate";
    let answer = ok(
        &router,
        "CreateMicrosoftAD",
        minimal_microsoft_ad("gated.example.com"),
        case,
    )
    .await;
    let id = string(&answer, "DirectoryId", case);
    let (pem, _) = certificate_pem(Some("gated.example.com"), (2033, 8, 9));

    let tags = json!([{ "Key": "env", "Value": "dev" }]);
    // Both transient stages of the chain: the gate refuses from each of them, and the
    // sentence that names a stage names the one the directory is actually in.
    for (when, expected_stage) in [
        (Duration::ZERO, STAGE_REQUESTED),
        (INSIDE_THE_BUILD, STAGE_CREATING),
    ] {
        tokio::time::sleep(when).await;
        let reported = stage(&router, &id, case).await;
        assert_eq!(
            reported, expected_stage,
            "case {case:?}: expected the directory in {expected_stage}, got {reported}"
        );

        // (the operation, its body, the modelled error class, the sentence that class is
        // stated in)
        let rows: [(&str, Value, &str, String); 6] = [
            (
                "AddTagsToResource",
                json!({ "ResourceId": &id, "Tags": tags }),
                "ClientException",
                format!(
                    "The operation is not supported for directories in this state. Directory \
                     Status: {expected_stage}"
                ),
            ),
            (
                "RemoveTagsFromResource",
                json!({ "ResourceId": &id, "TagKeys": ["env"] }),
                "ClientException",
                format!(
                    "The operation is not supported for directories in this state. Directory \
                     Status: {expected_stage}"
                ),
            ),
            (
                "EnableLDAPS",
                json!({ "DirectoryId": &id, "Type": LDAPS_TYPE_CLIENT }),
                "DirectoryUnavailableException",
                format!("Directory: '{id}' is not available for LDAPS operation."),
            ),
            (
                "DisableLDAPS",
                json!({ "DirectoryId": &id, "Type": LDAPS_TYPE_CLIENT }),
                "DirectoryUnavailableException",
                format!("Directory: '{id}' is not available for LDAPS operation."),
            ),
            (
                "RegisterCertificate",
                json!({ "DirectoryId": &id, "CertificateData": &pem }),
                "DirectoryUnavailableException",
                directory_unavailable(&id),
            ),
            (
                "ResetUserPassword",
                json!({
                    "DirectoryId": &id,
                    "UserName": MICROSOFT_AD_ADMINISTRATOR,
                    "NewPassword": GOOD_PASSWORD
                }),
                "DirectoryUnavailableException",
                format!("Could not reset user password as directory: {id} is not in Active state."),
            ),
        ];

        for (operation, body, code, message) in rows {
            refused_with(
                &router,
                operation,
                body,
                code,
                &message,
                &format!("{case}: {operation} in {expected_stage}"),
            )
            .await;
        }

        // The certificate itself is read before the directory's stage is, so data that is
        // not a certificate is reported as such from either transient stage rather than as
        // the directory being unavailable.
        refused_with(
            &router,
            "RegisterCertificate",
            json!({ "DirectoryId": &id, "CertificateData": "not a certificate" }),
            "InvalidCertificateException",
            &format!(
                "Certificate is not valid. Could not register the certificate with the \
                 directory {id}"
            ),
            &format!("{case}: a non-PEM body in {expected_stage}"),
        )
        .await;
    }

    // Nothing the refused calls carried was stored: a gate that refused and wrote would
    // leave the tag behind.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let listed = ok(
        &router,
        "ListTagsForResource",
        json!({ "ResourceId": &id }),
        case,
    )
    .await;
    assert_eq!(
        crate::directory_lifecycle::tag_map(&listed, case).len(),
        0,
        "case {case:?}: expected the refused tag writes to have stored nothing, got {listed}"
    );

    // And once the chain has run out, the same calls are served.
    for (operation, body) in [
        (
            "AddTagsToResource",
            json!({ "ResourceId": &id, "Tags": [{ "Key": "env", "Value": "dev" }] }),
        ),
        (
            "RemoveTagsFromResource",
            json!({ "ResourceId": &id, "TagKeys": ["env"] }),
        ),
        (
            "ResetUserPassword",
            json!({
                "DirectoryId": &id,
                "UserName": MICROSOFT_AD_ADMINISTRATOR,
                "NewPassword": GOOD_PASSWORD
            }),
        ),
        (
            "RegisterCertificate",
            json!({ "DirectoryId": &id, "CertificateData": &pem }),
        ),
    ] {
        ok(
            &router,
            operation,
            body,
            &format!("{case}: {operation} once the directory is in service"),
        )
        .await;
    }

    // The gate the certificate opened is the LDAPS one: with the registration in service,
    // the activation the build refused is served.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "EnableLDAPS",
        json!({ "DirectoryId": &id, "Type": LDAPS_TYPE_CLIENT }),
        &format!("{case}: EnableLDAPS once the directory is in service"),
    )
    .await;
}

/// RULE: a create that makes a directory's administrator account refuses a `Password` the
/// directory's own policy does not accept — and that policy forbids the name of the account
/// the password belongs to, in any case and anywhere in the password — stating the policy in
/// the sentence Directory Service words it, and creating nothing.
#[tokio::test]
async fn a_create_refuses_a_password_naming_the_administrator_account() {
    let router = service();

    // The sentence Directory Service states the whole administrator-password policy in.
    let expected = "Invalid password. The password must be between 8 and 64 characters, not \
                    contain the word \"admin\", and must have at least one character from \
                    three of these four categories: lowercase, uppercase, numeric, and \
                    special characters.";

    // The rows span the case of the forbidden word (as written, lower, upper, mixed), where
    // in the password it sits (start, middle, end, inside a longer word), and passwords that
    // do not contain it at all — including one a letter away from it. Every row satisfies
    // the modelled `Password` pattern, so what accepts or refuses it is the policy and not
    // the boundary validator.
    let rows: [(&str, &str, bool); 9] = [
        ("the word as the docs write it", "Admin12345!", false),
        ("the word in lower case", "admin12345!", false),
        ("the word in upper case", "ADMIN12345!", false),
        ("the word in mixed case", "aDmIn12345!", false),
        ("the word at the end", "12345!Admin", false),
        ("the word inside a longer one", "Administrator1!", false),
        ("the word with a letter changed", "Adm1nistrator1!", true),
        ("a password naming no account", GOOD_PASSWORD, true),
        ("the suite's own password", PASSWORD, true),
    ];

    // Both creates that make an administrator account state the one policy, so the table
    // runs against each of them.
    for (operation, body_of) in [
        (
            "CreateDirectory",
            (|name: &str| minimal_create(name)) as fn(&str) -> Value,
        ),
        (
            "CreateMicrosoftAD",
            (|name: &str| minimal_microsoft_ad(name)) as fn(&str) -> Value,
        ),
    ] {
        for (index, (class, password, accepted)) in rows.iter().enumerate() {
            let case = format!("{operation}: {class}");
            let mut body = body_of(&format!("pw{index}.{}.example.com", operation.to_lowercase()));
            body["Password"] = json!(password);

            let before = stored_directory_ids(&router, &case).await;
            if *accepted {
                let answer = ok(&router, operation, body, &case).await;
                let id = string(&answer, "DirectoryId", &case);
                let after = stored_directory_ids(&router, &case).await;
                assert!(
                    after.contains(&id) && after.len() == before.len() + 1,
                    "case {case:?}: expected the accepted create to store exactly one \
                     directory, got {after:?} after {before:?}"
                );
            } else {
                refused_with(
                    &router,
                    operation,
                    body,
                    "InvalidParameterException",
                    expected,
                    &case,
                )
                .await;
                let after = stored_directory_ids(&router, &case).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: expected the refused create to store nothing"
                );
            }
        }
    }
}

/// The `DirectoryId` of every stored directory, so a create's effect on the store can be
/// asserted rather than assumed.
async fn stored_directory_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let answer = ok(router, "DescribeDirectories", json!({}), case).await;
    descriptions(&answer, case)
        .iter()
        .map(|description| string(description, "DirectoryId", case))
        .collect()
}
