// SPDX-License-Identifier: CC0-1.0

//! The single sign-on prerequisite from the ASSIGNMENT side: only an alias that really
//! landed on THIS directory supplies its access URL, so every way an assignment fails to
//! happen leaves the enable refused — and the directory's build stage is not a
//! prerequisite of its own.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from the state
//! struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test: `EnableSso` succeeds only for a directory whose own access URL
//! exists, which is only once `CreateAlias` has ASSIGNED an alias to that directory;
//! until then the directory reports the directory id as its `Alias` and that id's host as
//! its `AccessUrl`, and the enable is refused with the modelled `ClientException`
//! (HTTP 400) naming the directory the request asked for, recording nothing. The rows
//! differ along what the rule generalises over — WHY no alias is assigned: none was ever
//! asked for, the value was one another live directory reserved, the value was one a
//! DELETED directory still reserves, or the request was made while the directory was
//! still being built and the assignment needs a serving one. The access URL is also a
//! property of one directory: an aliased, enabled sibling in the same store satisfies no
//! other directory's prerequisite.
//!
//! And the stage is not a prerequisite of the enable itself: a directory still in a
//! non-serving stage is refused for the missing access URL — the same class and the same
//! sentence as a serving one — because the model declares no stage error for this
//! operation.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, describe_one, minimal_create, ok, service, string, PAST_ONE_SPAN,
    STAGE_ACTIVE, STAGE_CREATING, STAGE_REQUESTED,
};

/// The DNS suffix an access URL is built on, from the `AccessUrl` member's own
/// documentation: the alias "is used to construct the access URL for the directory, such
/// as `http://<alias>.awsapps.com`"
/// (`smithy_query(query="doc", shape="DirectoryDescription", member="AccessUrl")`). The
/// member reports the HOST alone.
const ACCESS_URL_SUFFIX: &str = ".awsapps.com";

/// Why a row's directory ends up with no alias of its own, which is the axis this table
/// spans.
#[derive(Clone, Copy, Debug)]
enum Unassigned {
    /// No `CreateAlias` is ever asked for.
    NeverAsked,
    /// The value asked for is the one the live sibling directory reserved.
    TakenByLiveDirectory,
    /// The value asked for is the one a since-deleted directory still reserves.
    TakenByDeletedDirectory,
    /// The value is asked for while the directory is still being built, which is not a
    /// stage an assignment is served in.
    AskedWhileBuilding,
}

/// One row: why no alias is assigned, and the value that finally assigns one.
struct Row {
    case: &'static str,
    unassigned: Unassigned,
    /// A value nobody reserved, which supplies the access URL at the end of the row.
    recovery: &'static str,
}

/// The rows: each way an assignment does not happen, plus the value that finally does.
const ROWS: [Row; 4] = [
    Row {
        case: "no alias was ever asked for",
        unassigned: Unassigned::NeverAsked,
        recovery: "sso-never-asked",
    },
    Row {
        case: "the value asked for is reserved by a live directory",
        unassigned: Unassigned::TakenByLiveDirectory,
        recovery: "sso-after-live-collision",
    },
    Row {
        case: "the value asked for is reserved by a deleted directory",
        unassigned: Unassigned::TakenByDeletedDirectory,
        recovery: "sso-after-deleted-collision",
    },
    Row {
        case: "the value was asked for while the directory was still being built",
        unassigned: Unassigned::AskedWhileBuilding,
        recovery: "sso-after-building",
    },
];

/// The alias the live sibling holds, and the one the deleted directory still reserves.
const LIVE_SIBLING_ALIAS: &str = "sso-live-sibling";
const DELETED_DIRECTORY_ALIAS: &str = "sso-deleted-holder";

/// The `Message` of a refusal body, whichever casing the envelope carries it in: a
/// modelled error of this service reports `Message`, the boundary's `ValidationException`
/// reports `message`.
fn message(parsed: &Value, case: &str) -> String {
    parsed["Message"]
        .as_str()
        .or_else(|| parsed["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed}"))
        .to_string()
}

/// The diagnostic this service answers a missing access URL with: the sentence, then the
/// request-id suffix it appends to every refusal it words. The directory id is the
/// request's, and the request id is the one the SAME refusal reports.
fn alias_required(directory_id: &str, reported_request_id: &str) -> String {
    format!(
        "An alias is required before enabling SSO. DomainId={directory_id} : RequestId: \
         {reported_request_id}"
    )
}

/// What the reader reports about a directory, as `(Alias, AccessUrl, SsoEnabled, Stage)`.
async fn projection(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> (String, String, bool, String) {
    let described = describe_one(router, directory_id, case).await;
    let sso_enabled = described["SsoEnabled"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case:?}: expected a boolean SsoEnabled, got {described}"));
    (
        string(&described, "Alias", case),
        string(&described, "AccessUrl", case),
        sso_enabled,
        string(&described, "Stage", case),
    )
}

/// Assert the directory still has NO access URL of its own: its `Alias` stands in as the
/// directory id and its `AccessUrl` is that id's host, with single sign-on off.
async fn assert_no_access_url(router: &MultiServiceRouter, directory_id: &str, case: &str) {
    let (alias, access_url, sso_enabled, _) = projection(router, directory_id, case).await;
    assert_eq!(
        (alias.as_str(), access_url.as_str(), sso_enabled),
        (
            directory_id,
            format!("{directory_id}{ACCESS_URL_SUFFIX}").as_str(),
            false
        ),
        "case {case:?}: expected the standing-in alias, its host and single sign-on off, \
         got {alias:?}/{access_url:?}/{sso_enabled}"
    );
}

/// `EnableSso` must be refused for the missing access URL, recording nothing.
async fn assert_refused_for_the_access_url(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) {
    let (status, parsed) = call(router, "EnableSso", json!({ "DirectoryId": directory_id })).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected EnableSso to answer 400 with no access URL, got {status} \
         with {parsed}"
    );
    assert_eq!(
        parsed["__type"],
        json!("ClientException"),
        "case {case:?}: expected the modelled client refusal, got {parsed}"
    );
    let reported = string(&parsed, "RequestId", case);
    assert_eq!(
        message(&parsed, case),
        alias_required(directory_id, &reported),
        "case {case:?}: expected the missing-access-URL sentence, got {parsed}"
    );
    assert_no_access_url(router, directory_id, case).await;
}

/// A `CreateAlias` that must be refused with HTTP 400, returning the wire error code.
async fn refused_assignment(
    router: &MultiServiceRouter,
    directory_id: &str,
    alias: &str,
    case: &str,
) -> String {
    let (status, parsed) = call(
        router,
        "CreateAlias",
        json!({ "DirectoryId": directory_id, "Alias": alias }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected CreateAlias to be refused, got {status} with {parsed}"
    );
    parsed["__type"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a wire error type, got {parsed}"))
        .to_string()
}

/// Assign `alias` to `directory_id` and assert the reader reports the pair that landed.
async fn assign(router: &MultiServiceRouter, directory_id: &str, alias: &str, case: &str) {
    let answer = ok(
        router,
        "CreateAlias",
        json!({ "DirectoryId": directory_id, "Alias": alias }),
        case,
    )
    .await;
    assert_eq!(
        (
            string(&answer, "DirectoryId", case),
            string(&answer, "Alias", case)
        ),
        (directory_id.to_string(), alias.to_string()),
        "case {case:?}: expected CreateAlias to report the pair it stored, got {answer}"
    );
}

#[tokio::test]
async fn only_an_alias_assigned_to_the_directory_supplies_its_access_url() {
    let router = service();

    // The sibling that HAS its access URL and single sign-on on, and the one whose alias
    // outlives it: an alias "cannot be deleted or reused", so a deleted directory's value
    // is still taken.
    let sibling = create_directory(
        &router,
        minimal_create("sibling.example.com"),
        "the aliased sibling",
    )
    .await;
    let doomed = create_directory(
        &router,
        minimal_create("doomed.example.com"),
        "the directory whose alias outlives it",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    assign(&router, &sibling, LIVE_SIBLING_ALIAS, "the aliased sibling").await;
    assign(
        &router,
        &doomed,
        DELETED_DIRECTORY_ALIAS,
        "the directory whose alias outlives it",
    )
    .await;
    ok(
        &router,
        "EnableSso",
        json!({ "DirectoryId": &sibling }),
        "the aliased sibling",
    )
    .await;
    ok(
        &router,
        "DeleteDirectory",
        json!({ "DirectoryId": &doomed }),
        "the directory whose alias outlives it",
    )
    .await;

    // One directory per row, each created fresh: the enable is refused for the access URL
    // while the directory is still being built too, which is what shows the build stage is
    // not a prerequisite of the enable — the same class and the same sentence as a serving
    // directory gets.
    let mut ids = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let id = create_directory(
            &router,
            minimal_create(&format!("unassigned{index}.example.com")),
            row.case,
        )
        .await;
        // The enable is refused for the access URL whatever stage the directory is in,
        // which is the assertion that does not depend on the clock.
        assert_refused_for_the_access_url(&router, &id, row.case).await;

        // The stage facet on top of it, for as long as the create's transition has not
        // come due: the directory must be in one of this service's pre-serving stages,
        // and an assignment is NOT served there. A loaded runner that let the transition
        // fall due first leaves the facet to the rows that observed it — the refusal
        // above is asserted either way.
        let (_, _, _, stage) = projection(&router, &id, row.case).await;
        if stage != STAGE_ACTIVE {
            assert!(
                [STAGE_REQUESTED, STAGE_CREATING].contains(&stage.as_str()),
                "case {:?}: expected one of this service's pre-serving stages, got \
                 {stage:?}",
                row.case
            );
            if matches!(row.unassigned, Unassigned::AskedWhileBuilding) {
                // An assignment is served for a directory that is already serving, so
                // this one is refused and nothing is assigned or reserved.
                let code = refused_assignment(&router, &id, row.recovery, row.case).await;
                assert_eq!(
                    code, "InvalidParameterException",
                    "case {:?}: expected the stage precondition of the assignment, got \
                     {code:?}",
                    row.case
                );
            }
        }
        ids.push(id);
    }

    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (row, id) in ROWS.iter().zip(&ids) {
        let (_, _, _, stage) = projection(&router, id, row.case).await;
        assert_eq!(
            stage, STAGE_ACTIVE,
            "case {:?}: the directory must be serving before the alias is assigned, got \
             {stage:?}",
            row.case
        );

        // The row's reason no alias lands on this directory.
        match row.unassigned {
            Unassigned::NeverAsked => {}
            Unassigned::TakenByLiveDirectory => {
                let code = refused_assignment(&router, id, LIVE_SIBLING_ALIAS, row.case).await;
                assert_eq!(
                    code, "EntityAlreadyExistsException",
                    "case {:?}: expected the reserved value to be refused as taken, got \
                     {code:?}",
                    row.case
                );
            }
            Unassigned::TakenByDeletedDirectory => {
                let code =
                    refused_assignment(&router, id, DELETED_DIRECTORY_ALIAS, row.case).await;
                assert_eq!(
                    code, "EntityAlreadyExistsException",
                    "case {:?}: expected a deleted directory's reservation to still be \
                     taken, got {code:?}",
                    row.case
                );
            }
            Unassigned::AskedWhileBuilding => {}
        }

        // Nothing landed, so the access URL still does not exist and the enable is still
        // refused for exactly that — and the aliased, enabled sibling in the same store
        // satisfies nothing for this directory.
        assert_refused_for_the_access_url(&router, id, row.case).await;

        // The one missing prerequisite, supplied with a value nobody reserved: the
        // IDENTICAL enable is then admitted with the modelled empty body.
        assign(&router, id, row.recovery, row.case).await;
        let answer = ok(&router, "EnableSso", json!({ "DirectoryId": id }), row.case).await;
        assert_eq!(
            answer,
            json!({}),
            "case {:?}: EnableSso has no modelled output members, got {answer}",
            row.case
        );
        let (alias, access_url, sso_enabled, _) = projection(&router, id, row.case).await;
        assert_eq!(
            (alias.as_str(), access_url.as_str(), sso_enabled),
            (
                row.recovery,
                format!("{recovery}{ACCESS_URL_SUFFIX}", recovery = row.recovery).as_str(),
                true
            ),
            "case {:?}: expected the assigned alias, its access URL and single sign-on \
             on, got {alias:?}/{access_url:?}/{sso_enabled}",
            row.case
        );
    }

    // The sibling's own record is untouched by any of the rows: it still reports the
    // alias it was given and the flag its own enable set.
    let (alias, access_url, sso_enabled, _) =
        projection(&router, &sibling, "the aliased sibling").await;
    assert_eq!(
        (alias.as_str(), access_url.as_str(), sso_enabled),
        (
            LIVE_SIBLING_ALIAS,
            format!("{LIVE_SIBLING_ALIAS}{ACCESS_URL_SUFFIX}").as_str(),
            true
        ),
        "case {:?}: expected the sibling's own alias, access URL and flag to stand, got \
         {alias:?}/{access_url:?}/{sso_enabled}",
        "the aliased sibling"
    );
}
