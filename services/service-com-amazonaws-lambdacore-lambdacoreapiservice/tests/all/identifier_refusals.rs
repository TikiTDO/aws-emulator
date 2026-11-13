// SPDX-License-Identifier: CC0-1.0

//! What this service answers for an `Identifier` it settles BEFORE looking for a connector.
//!
//! THE RULES, each over a SET of identifiers and through every operation a
//! `NetworkConnectorIdentifier` addresses (`GetNetworkConnector`,
//! `UpdateNetworkConnector`, `DeleteNetworkConnector`, `ListNetworkConnectorVersions`):
//!
//! 1. an identifier spelled as an ARN that is NOT one of this service's network-connector
//!    ARNs names no connector anywhere, so it is refused as a bad parameter: `HTTP 400`
//!    `InvalidParameterValueException`, `Type` `User`, naming the resource the service was
//!    asked to address — the `network-connector` resource type and the value as supplied.
//!    `NetworkConnectorArn`'s own expression is what an ARN of a connector is
//!    (`smithy_query(query="shape",shape="NetworkConnectorArn")`):
//!    `arn:<partition>:lambda:<region>:<12-digit account>:network-connector:<resource>`,
//!    optionally version-qualified;
//! 2. a well-formed network-connector ARN whose ACCOUNT is not the caller's addresses
//!    another account's resource, which is an authorization refusal and not an existence
//!    answer: `HTTP 403` `AccessDeniedException` with the fixed sentence
//!    `Cross-account access is not allowed.`;
//! 3. both precede the lookup, so the answer is the same whether or not this account holds
//!    a connector of that name — which is the point: the refusal says nothing about what is
//!    there. An ARN of the caller's OWN account that names no connector is still the
//!    modelled `404 ResourceNotFoundException`.
//!
//! The harness (router, request helpers) is the lifecycle suite's; it is `pub(crate)` there
//! so every suite drives the emulator the same way.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use super::network_connector_lifecycle::{
    at, create, delete, emulator, expected_arn, get, list_versions, minimal_configuration,
    minimal_create, text, update, Answer,
};

/// The account every ARN of an emulated resource carries, which is the caller's
/// (`engine::EMULATOR_ACCOUNT_ID`), spelled out here so a wrong account in the
/// implementation fails.
const OWN_ACCOUNT: &str = "000000000000";

/// An account id that is not the caller's, well formed as an account id.
const ANOTHER_ACCOUNT: &str = "111122223333";

/// A second one, so no row's outcome can depend on the digits.
const A_THIRD_ACCOUNT: &str = "444455556666";

/// The `Type` a refusal of the parameter class carries.
const USER: &str = "User";

/// The sentence a cross-account refusal states, in full.
const CROSS_ACCOUNT: &str = "Cross-account access is not allowed.";

/// One operation a `NetworkConnectorIdentifier` addresses.
#[derive(Clone, Copy, Debug)]
enum Addressed {
    Get,
    Update,
    Delete,
    ListVersions,
}

impl Addressed {
    /// Every one of them, so a row never covers a subset by accident.
    const ALL: [Self; 4] = [Self::Get, Self::Update, Self::Delete, Self::ListVersions];
}

/// Address `identifier` through `operation`.
async fn send(router: &MultiServiceRouter, operation: Addressed, identifier: &str) -> Answer {
    match operation {
        Addressed::Get => get(router, identifier).await,
        // Every member of the update request but `Identifier` is optional, so the body
        // carries only a configuration — the refusal under test is about the identifier.
        Addressed::Update => {
            update(
                router,
                identifier,
                json!({ "Configuration": minimal_configuration() }),
            )
            .await
        }
        Addressed::Delete => delete(router, identifier).await,
        Addressed::ListVersions => list_versions(router, identifier, "").await,
    }
}

/// Assert `answer` is the refusal rule 1 requires for `identifier`.
fn assert_invalid_arn(case: &str, identifier: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: an ARN that is not a network connector's is refused with HTTP 400, \
         got {} with {:?}",
        answer.status, answer.body
    );
    let code = text(&answer.body, "/__type");
    assert!(
        code.ends_with("InvalidParameterValueException"),
        "case {case:?}: expected InvalidParameterValueException, got {code:?} with {:?}",
        answer.body
    );
    assert_eq!(
        text(&answer.body, "/Type"),
        USER,
        "case {case:?}: expected the parameter class's Type {USER:?}, got {:?}",
        at(&answer.body, "/Type")
    );
    let message = text(&answer.body, "/message");
    let expected = format!("Network connector arn invalid: network-connector:{identifier}");
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
}

/// Assert `answer` is the refusal rule 2 requires.
fn assert_cross_account(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: an ARN of another account is refused with HTTP 403, got {} with {:?}",
        answer.status, answer.body
    );
    let code = text(&answer.body, "/__type");
    assert!(
        code.ends_with("AccessDeniedException"),
        "case {case:?}: expected AccessDeniedException, got {code:?} with {:?}",
        answer.body
    );
    assert_eq!(
        text(&answer.body, "/Message"),
        CROSS_ACCOUNT,
        "case {case:?}: expected the refusal to state {CROSS_ACCOUNT:?}, got {:?}",
        at(&answer.body, "/Message")
    );
}

/// RULE 1: every ARN that is not one of this service's network-connector ARNs is the
/// parameter refusal, through every operation an identifier addresses.
///
/// The rows differ along what the rule generalises over: which part of the ARN is wrong
/// (the service, the resource type, the account's shape, the resource being unnamed, the
/// value being barely an ARN at all) and whether a version suffix is present.
#[tokio::test]
async fn an_arn_that_is_not_a_connectors_is_refused_as_a_bad_parameter() {
    let router = emulator();
    let rows = [
        ("an ARN of another service", "arn:aws:s3:::probe-bucket".to_string()),
        (
            "a Lambda ARN of another resource type",
            format!("arn:aws:lambda:eu-west-1:{OWN_ACCOUNT}:function:some-function"),
        ),
        (
            "a connector ARN naming no resource",
            format!("arn:aws:lambda:eu-west-1:{OWN_ACCOUNT}:network-connector:"),
        ),
        (
            "a connector ARN whose account is too short",
            "arn:aws:lambda:eu-west-1:12345:network-connector:theirs".to_string(),
        ),
        (
            "a connector ARN whose account is not digits",
            "arn:aws:lambda:eu-west-1:not-an-accou:network-connector:theirs".to_string(),
        ),
        (
            "a Lambda ARN of another resource type, version-qualified",
            format!("arn:aws:lambda:eu-west-1:{OWN_ACCOUNT}:function:some-function:2"),
        ),
        ("a value that only starts like an ARN", "arn:nonsense".to_string()),
    ];

    for (case, identifier) in &rows {
        for operation in Addressed::ALL {
            let answer = send(&router, operation, identifier).await;
            assert_invalid_arn(
                &format!("{case} through {operation:?}"),
                identifier,
                &answer,
            );
        }
    }
}

/// RULE 2: every well-formed network-connector ARN of another account is the access
/// refusal, through every operation an identifier addresses.
///
/// The rows differ along the account, the Region, the partition and whether the ARN
/// qualifies a version, so the rule holds for the ARN's account alone.
#[tokio::test]
async fn a_connector_arn_of_another_account_is_refused_as_access_denied() {
    let router = emulator();
    let rows = [
        (
            "another account, another Region",
            format!("arn:aws:lambda:us-west-2:{ANOTHER_ACCOUNT}:network-connector:nc-11111111-2222"),
        ),
        (
            "another account, this Region",
            format!("arn:aws:lambda:eu-west-1:{ANOTHER_ACCOUNT}:network-connector:theirs"),
        ),
        (
            "a third account, version-qualified",
            format!("arn:aws:lambda:eu-west-1:{A_THIRD_ACCOUNT}:network-connector:theirs:3"),
        ),
        (
            "another partition and account",
            format!("arn:aws-us-gov:lambda:us-gov-west-1:{A_THIRD_ACCOUNT}:network-connector:gov"),
        ),
    ];

    for (case, identifier) in &rows {
        for operation in Addressed::ALL {
            let answer = send(&router, operation, identifier).await;
            assert_cross_account(&format!("{case} through {operation:?}"), &answer);
        }
    }
}

/// RULE 3: both refusals precede the lookup, and an ARN of the caller's OWN account is
/// still answered by the lookup.
///
/// A connector exists here under a name, so each row shows the refusal does not depend on
/// the store: the other account's ARN naming THAT connector's name is still refused, and
/// this account's ARN naming a connector that is not there is still the modelled 404.
#[tokio::test]
async fn the_identifier_refusals_precede_the_lookup() {
    let router = emulator();
    let name = "lnc-own-account";
    let created = create(&router, minimal_create(name)).await;
    assert_eq!(
        created.status, 202,
        "expected HTTP 202 from CreateNetworkConnector, got {} with {:?}",
        created.status, created.body
    );
    let id = text(&created.body, "/Id");

    // The same connector, named by ANOTHER account's ARN: refused on the account, not
    // reported as found.
    for resource in [name, id.as_str()] {
        let identifier =
            format!("arn:aws:lambda:eu-west-1:{ANOTHER_ACCOUNT}:network-connector:{resource}");
        let answer = get(&router, &identifier).await;
        assert_cross_account(
            &format!("a connector that EXISTS here, addressed by another account's ARN \
                      ({resource})"),
            &answer,
        );
    }

    // And an ARN that is not a connector's, with this account's own connector name in its
    // resource part, is still the parameter refusal.
    let not_a_connector = format!("arn:aws:lambda:eu-west-1:{OWN_ACCOUNT}:function:{name}");
    let answer = get(&router, &not_a_connector).await;
    assert_invalid_arn(
        "an existing connector's name in another resource type's ARN",
        &not_a_connector,
        &answer,
    );

    // This account's own connector ARN resolves: the gate refuses nothing it should not.
    let own = expected_arn(&id);
    let found = get(&router, &own).await;
    assert_eq!(
        found.status, 200,
        "this account's own connector ARN must resolve, got {} with {:?}",
        found.status, found.body
    );
    assert_eq!(
        text(&found.body, "/Id"),
        id,
        "the answer must report the connector the ARN names, got {:?}",
        at(&found.body, "/Id")
    );

    // While this account's ARN naming a connector that is not there is absence, not a
    // parameter or access refusal.
    let absent = expected_arn("nc-ffffffffffffffff");
    let missing = get(&router, &absent).await;
    assert_eq!(
        missing.status, 404,
        "this account's ARN naming no connector is absence, got {} with {:?}",
        missing.status, missing.body
    );
    assert_eq!(
        text(&missing.body, "/Message"),
        format!("Network connector not found: {absent}"),
        "the 404 echoes the identifier as supplied, got {:?}",
        at(&missing.body, "/Message")
    );
}
