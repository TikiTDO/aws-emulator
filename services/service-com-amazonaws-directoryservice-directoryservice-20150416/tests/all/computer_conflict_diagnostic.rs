// SPDX-License-Identifier: CC0-1.0

//! What a computer-account conflict DISCLOSES, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`), so nothing here reaches into
//! the state struct and an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test, over a TABLE whose rows differ along everything the refusal must
//! NOT vary with: a `CreateComputer` naming an account the directory already holds is
//! refused with the modelled `EntityAlreadyExistsException`, HTTP 400, whose diagnostic is
//! this service's FIXED sentence — the only variable part is the call's own request id,
//! which the sentence names and the envelope's suffix repeats, and which is the id the
//! response's modelled `RequestId` member and its `x-amzn-requestid` header report. The
//! sentence says nothing about what collided, so neither the directory, nor either spelling
//! of the account name, nor the organizational unit the account sits in, nor any attribute
//! the account carries can be read out of it.
//!
//! The sibling suite `computer_account_unavailable_wording` asserts the same sentence over
//! the shape of the colliding NAME (and owns the wording helper both suites state it
//! through); the rows here vary the directory's type, the unit the stored account was
//! placed in, the attributes it carries, the unit and password the colliding call sends,
//! and the number of accounts the directory already holds. `computers` is what asserts the
//! uniqueness rule itself.
//!
//! Two invariants of the refusal come with it: it is a refusal, so it changes nothing — the
//! same call refused twice reports two different request ids and nothing else differs, and
//! another name in the same directory is still admitted afterwards.

use serde_json::{json, Value};

use crate::computer_account_unavailable_wording::could_not_create_computer;
use crate::directory_lifecycle::{create_directory, minimal_create, PAST_ONE_SPAN};
use crate::harness::{call_raw, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The header AWS reports a response's own request id in, and where an SDK's response
/// metadata reads a failed call's request id from.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// A one-time join `Password` within the modelled `length 8..64` that meets the directory's
/// own password policy, for the create that persists the account.
const JOIN_PASSWORD: &str = "Cmp-1-Time!Pw";

/// A DIFFERENT well-formed join password, for the create that collides: the one-time
/// password plays no part in whether the name is already taken.
const COLLIDING_PASSWORD: &str = "Cmp-2-Time!Pw";

/// One row: a directory to hold the account, how the account was created, and how the
/// colliding create asks for it again.
struct ConflictCase {
    case: &'static str,
    /// The directory's fully qualified name — its labels are the `DC=` components an
    /// organizational unit of its own domain has to end in.
    domain: &'static str,
    /// Whether the row's directory is a Managed Microsoft AD one rather than a Simple AD:
    /// the refusal is the computer-account family's either way.
    microsoft_ad: bool,
    /// The name the first create persists.
    created: &'static str,
    /// The name the colliding create sends — the same account, in whatever case.
    sent: &'static str,
    /// The organizational unit the first create places the account in, or `None` for the
    /// directory's default computers container.
    stored_unit: Option<&'static str>,
    /// The organizational unit the colliding create asks for, or `None` to ask for none.
    /// A named one is a distinguished name of the same domain, so the request is one the
    /// handler's parameter rules admit and the conflict is what refuses it.
    colliding_unit: Option<&'static str>,
    /// The LDAP attributes the first create applies to the stored account.
    attributes: &'static [(&'static str, &'static str)],
    /// Names the row creates in the same directory BEFORE the one that collides, so the
    /// refusal is asserted on a directory holding more than the colliding account alone.
    neighbours: &'static [&'static str],
}

/// The `CreateComputer` body for one call.
fn create_computer(
    directory_id: &str,
    computer_name: &str,
    password: &str,
    unit: Option<&str>,
    attributes: &[(&str, &str)],
) -> Value {
    let mut body = json!({
        "DirectoryId": directory_id,
        "ComputerName": computer_name,
        "Password": password
    });
    if let Some(unit) = unit {
        body["OrganizationalUnitDistinguishedName"] = json!(unit);
    }
    if !attributes.is_empty() {
        body["ComputerAttributes"] = Value::Array(
            attributes
                .iter()
                .map(|(name, value)| json!({ "Name": name, "Value": value }))
                .collect(),
        );
    }
    body
}

/// One `CreateComputer` call, returning `(status, parsed body, the id the header carried)`
/// — read from the WHOLE response, so the header the rule names is asserted rather than
/// assumed.
async fn answered(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
    case: &str,
) -> (u16, Value, Option<String>) {
    let response = call_raw(router, "CreateComputer", body).await;
    let status = response.status().as_u16();
    let header = response
        .headers()
        .get(REQUEST_ID_HEADER)
        .map(|value| value.to_str().expect("a textual header").to_string());
    let payload = response.into_body();
    let parsed: Value = serde_json::from_slice(&payload)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a JSON body, got {payload:?}"));
    (status, parsed, header)
}

/// One `CreateComputer` that must be admitted, for a name a later call collides with.
async fn create_account(
    router: &smithy_server_runtime::MultiServiceRouter,
    directory_id: &str,
    computer_name: &str,
    unit: Option<&str>,
    attributes: &[(&str, &str)],
    case: &str,
) {
    let body = create_computer(directory_id, computer_name, JOIN_PASSWORD, unit, attributes);
    let (status, parsed, _) = answered(router, body, case).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create of {computer_name:?} to be admitted, got {status} \
         with {parsed}"
    );
    assert_eq!(
        parsed["Computer"]["ComputerName"],
        json!(computer_name),
        "case {case:?}: expected the created account to be reported, got {parsed}"
    );
}

/// The conflict refusal of `body`, asserted down to its diagnostic and returning the
/// request id it reported — the ONE id the message, the modelled member and the header all
/// carry.
async fn refused_as_a_conflict(
    router: &smithy_server_runtime::MultiServiceRouter,
    body: Value,
    withheld: &[&str],
    case: &str,
) -> String {
    let (status, parsed, header) = answered(router, body, case).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the colliding create to be refused, got {status} with {parsed}"
    );
    let code = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        code.contains("EntityAlreadyExistsException"),
        "case {case:?}: expected EntityAlreadyExistsException, got {parsed}"
    );

    // The id is reported three times over and is one id: the modelled member, the header an
    // SDK reads response metadata from, and the sentence itself.
    let reported = string(&parsed, "RequestId", case);
    let in_header = header.unwrap_or_else(|| {
        panic!("case {case:?}: expected the {REQUEST_ID_HEADER} header, got none")
    });
    assert_eq!(
        in_header, reported,
        "case {case:?}: expected the header to carry the reported request id {reported:?}, got \
         {in_header:?}"
    );
    let message = string(&parsed, "Message", case);
    assert_eq!(
        message,
        could_not_create_computer(&reported),
        "case {case:?}: expected the fixed conflict sentence closing with this call's request \
         id, got {message:?}"
    );

    // Nothing of what collided is disclosed.
    for value in withheld {
        assert!(
            !message.contains(value),
            "case {case:?}: expected the diagnostic to withhold {value:?}, got {message:?}"
        );
    }
    reported
}

/// RULE: a `CreateComputer` naming an account the directory already holds is refused with
/// `EntityAlreadyExistsException` whose diagnostic is this service's fixed sentence, whose
/// only variable part is the call's own request id, and which discloses nothing of the
/// directory, the account, its unit or its attributes.
#[tokio::test]
async fn the_conflict_diagnostic_is_fixed_text_and_discloses_nothing_that_collided() {
    // The rows differ along the directory's type, where the stored account sits, what it
    // carries, and what the colliding call asks for — none of which the refusal varies
    // with.
    let rows: [ConflictCase; 5] = [
        ConflictCase {
            case: "a Simple AD account in the default container",
            domain: "plain.example.com",
            microsoft_ad: false,
            created: "node-01",
            sent: "node-01",
            stored_unit: None,
            colliding_unit: None,
            attributes: &[],
            neighbours: &[],
        },
        ConflictCase {
            case: "a Simple AD account placed in a named unit",
            domain: "units.example.com",
            microsoft_ad: false,
            created: "inunit",
            sent: "InUnit",
            stored_unit: Some("OU=Workstations,DC=units,DC=example,DC=com"),
            colliding_unit: None,
            attributes: &[],
            neighbours: &["neighbour-a", "neighbour-b"],
        },
        ConflictCase {
            case: "a Managed Microsoft AD account carrying attributes",
            domain: "managed.corp.example.com",
            microsoft_ad: true,
            created: "withattrs",
            sent: "WITHATTRS",
            stored_unit: None,
            colliding_unit: None,
            // `description` is the LDAP attribute `CreateComputer$ComputerAttributes`
            // applies and the account reports back; `computers` and `computer_attributes`
            // are what assert the naming rule it follows.
            attributes: &[("description", "a marker the refusal must not echo")],
            neighbours: &["other-node"],
        },
        ConflictCase {
            case: "a Managed Microsoft AD account asked for again in another unit",
            domain: "elsewhere.corp.example.com",
            microsoft_ad: true,
            created: "moved",
            sent: "moved",
            stored_unit: Some("OU=Servers,DC=elsewhere,DC=corp,DC=example,DC=com"),
            colliding_unit: Some("OU=Desktops,DC=elsewhere,DC=corp,DC=example,DC=com"),
            attributes: &[],
            neighbours: &[],
        },
        ConflictCase {
            case: "a Simple AD account whose name is the longest the model admits",
            domain: "longest.example.com",
            microsoft_ad: false,
            created: "fifteencharsok1",
            sent: "FifteenCharsOk1",
            stored_unit: Some("OU=Computers,DC=longest,DC=example,DC=com"),
            colliding_unit: Some("OU=Computers,DC=longest,DC=example,DC=com"),
            // Two recognized LDAP attributes, written in the casings a caller sends them
            // in: `description` and `operatingSystem` are both entries of the names the
            // service holds an attribute under (`crate::computer_attributes` is what
            // asserts that rule).
            attributes: &[
                ("description", "longest"),
                ("operatingSystem", "Windows Server"),
            ],
            neighbours: &["short"],
        },
    ];

    // One router, one directory per row, and ONE wait for the stage chain every create
    // owes: the rows are about the refusal, not about the clock.
    let router = service();
    let mut directory_ids = Vec::new();
    for row in &rows {
        let directory_id = if row.microsoft_ad {
            create_microsoft_ad(&router, minimal_microsoft_ad(row.domain), row.case).await
        } else {
            create_directory(&router, minimal_create(row.domain), row.case).await
        };
        directory_ids.push(directory_id);
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;

    for (row, directory_id) in rows.iter().zip(&directory_ids) {
        for neighbour in row.neighbours {
            create_account(&router, directory_id, neighbour, None, &[], row.case).await;
        }
        create_account(
            &router,
            directory_id,
            row.created,
            row.stored_unit,
            row.attributes,
            row.case,
        )
        .await;

        // Everything the refusal must not disclose: the directory, both spellings of the
        // account name, the unit it sits in and every attribute value it carries.
        let mut withheld = vec![
            directory_id.as_str(),
            row.created,
            row.sent,
            row.domain,
        ];
        withheld.extend(row.stored_unit);
        withheld.extend(row.colliding_unit);
        withheld.extend(row.attributes.iter().map(|(_, value)| *value));

        let colliding = create_computer(
            directory_id,
            row.sent,
            COLLIDING_PASSWORD,
            row.colliding_unit,
            &[],
        );
        let first = refused_as_a_conflict(&router, colliding.clone(), &withheld, row.case).await;

        // A refusal changes nothing: the very same call is refused the same way again, and
        // reports its OWN request id rather than repeating the first answer's.
        let again = refused_as_a_conflict(&router, colliding, &withheld, row.case).await;
        assert_ne!(
            again, first,
            "case {:?}: expected each refusal to report its own request id, got {first} twice",
            row.case
        );

        // And it consumed nothing: another name in the same directory is still admitted.
        create_account(
            &router,
            directory_id,
            "after-refusal",
            None,
            &[],
            row.case,
        )
        .await;
    }
}
