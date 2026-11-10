// SPDX-License-Identifier: CC0-1.0

//! `parentIdentifier`, `filter.parentIdentifiers` and `includeChildren` on the enabled
//! baseline listing, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`] the production emulator dispatches with,
//! so what these tests exercise is the wire behaviour a client sees — status codes, error
//! codes and JSON bodies — not the handlers' Rust signatures.
//!
//! THE RULE under test, asserted over a TABLE of inputs rather than one case: an
//! `EnabledBaseline` reports as its `parentIdentifier` the enablement it INHERITED its
//! configuration from, `filter.parentIdentifiers` selects on that same value (so an
//! enablement with no parent is selected by no value at all), and `includeChildren`
//! decides whether the inherited enablements — the ones carrying a parent — are in the
//! listing. A child of an enablement on an organizational unit is a deployment into a
//! member ACCOUNT of that unit, which takes the organization's membership: this service
//! records none, and `EnableBaseline` applies a baseline directly to the target the request
//! names, so every enablement it holds inherits from nothing. The consequences this suite
//! pins are therefore all of one rule: no entry reports a `parentIdentifier`, the listing
//! is the same whichever way `includeChildren` is sent, a `parentIdentifiers` filter
//! selects nothing, and no enablement is reported for an ACCOUNT — not even for the
//! accounts a deployed landing zone designates, which are recorded as members of no
//! particular unit.
//!
//! The values written out by hand name their source: the account-ARN form
//! (`arn:<partition>:organizations::<management account>:account/<organization>/<account
//! id>`) is the one AWS Organizations publishes for an account and is built here from the
//! target unit's own ARN; the modelled maximum length of `parentIdentifiers` (5) comes from
//! `smithy_query(query="shape", shape="EnabledBaselineFilter")`; and the baselines, their
//! versions and the parameter keys they declare come from
//! `service_..._awscontroltowerapis::state::BASELINE_CATALOG`, which is what `ListBaselines`
//! reports. The landing zone manifest is the shared well-formed one, with the two account
//! members a row designates.

use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::entries;
use crate::landing_zone_lifecycle::{
    call, emulator, past_the_settle_span, set_up_account, simple_manifest, Answer,
};

/// One enablement a row applies.
struct Enablement {
    baseline_identifier: &'static str,
    baseline_version: &'static str,
    target_identifier: &'static str,
    /// The `parameters` the enable carries, or `None` when it omits the member.
    parameters: Option<serde_json::Value>,
}

/// One row: the accounts the deployed landing zone designates (`None` for a row that
/// deploys no landing zone at all), and the enablements applied on top of it.
struct Governance {
    case: &'static str,
    /// The manifest's `centralizedLogging.accountId` and `securityRoles.accountId`.
    accounts: Option<(&'static str, &'static str)>,
    enablements: Vec<Enablement>,
}

/// The table: rows differ along what the rule generalises over — what the landing zone
/// designates (the shared manifest's own accounts; two distinct accounts; one account named
/// by both members), how many enablements the store holds, the baseline (one Control Tower
/// assesses inheritance drift on and two it does not), the version, the parameters, and the
/// organization, partition and management account the target units live in.
///
/// Every row deploys a landing zone, because Control Tower enables a baseline only for an
/// account it has been set up in; the row that names no accounts deploys the shared
/// manifest unchanged.
fn governances() -> Vec<Governance> {
    vec![
        Governance {
            case: "a landing zone designating the shared manifest's accounts, one enablement",
            accounts: None,
            enablements: vec![Enablement {
                baseline_identifier: "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
                baseline_version: "1.0",
                target_identifier:
                    "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a",
                parameters: None,
            }],
        },
        Governance {
            case: "a landing zone designating two distinct accounts, three enablements",
            accounts: Some(("222222222222", "333333333333")),
            enablements: vec![
                Enablement {
                    baseline_identifier:
                        "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
                    baseline_version: "4.0",
                    target_identifier:
                        "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-bb22-2b",
                    parameters: Some(serde_json::json!([
                        { "key": "IdentityCenterEnabledForBaseline", "value": true }
                    ])),
                },
                Enablement {
                    baseline_identifier:
                        "arn:aws:controltower:us-east-1::baseline/4T4HA1KMO10S6311",
                    baseline_version: "1.0",
                    target_identifier:
                        "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-cc33-3c",
                    parameters: Some(serde_json::json!([])),
                },
                Enablement {
                    baseline_identifier:
                        "arn:aws:controltower:eu-west-1::baseline/LN25R72TTG6IGPTQ",
                    baseline_version: "4.0",
                    target_identifier:
                        "arn:aws-us-gov:organizations::210987654321:ou/o-govexample/ou-gg88-8h",
                    parameters: None,
                },
            ],
        },
        Governance {
            case: "a landing zone naming one account by both members, two enablements",
            accounts: Some(("444444444444", "444444444444")),
            enablements: vec![
                Enablement {
                    baseline_identifier:
                        "arn:aws:controltower:ap-south-1::baseline/3WPD0NA6TJ9AOMU2",
                    baseline_version: "1.0",
                    target_identifier:
                        "arn:aws:organizations::123456789012:ou/o-anotherorg/ou-dd44-4d",
                    parameters: Some(serde_json::json!([])),
                },
                Enablement {
                    baseline_identifier:
                        "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2",
                    baseline_version: "3.0",
                    target_identifier:
                        "arn:aws:organizations::123456789012:ou/o-anotherorg/ou-ee55-5e",
                    parameters: Some(serde_json::json!([
                        { "key": "IdentityCenterEnabledForBaseline", "value": "true" }
                    ])),
                },
            ],
        },
    ]
}

/// The AWS Organizations ARN of the account `account_id` as a member of the organization
/// the unit `organizational_unit` belongs to: the unit's partition, management account and
/// organization, with the account's own resource type and id. This is the `targetIdentifier`
/// an enablement of an account WOULD report, which is what the cases below assert nothing
/// reports.
fn account_target(organizational_unit: &str, account_id: &str) -> String {
    let (prefix, resource) = organizational_unit
        .split_once(":ou/")
        .expect("every target in this table is an organizational unit ARN");
    let (organization, _) = resource
        .split_once('/')
        .expect("an organizational unit ARN names its organization");
    format!("{prefix}:account/{organization}/{account_id}")
}

/// Deploy a landing zone designating `accounts`, asserting it succeeded. The manifest is
/// the shared well-formed one with its two account members set to the row's.
async fn deploy_landing_zone(router: &MultiServiceRouter, case: &str, accounts: (&str, &str)) {
    let mut manifest = simple_manifest();
    manifest["centralizedLogging"]["accountId"] = serde_json::json!(accounts.0);
    manifest["securityRoles"]["accountId"] = serde_json::json!(accounts.1);
    let created = call(
        router,
        "/create-landingzone",
        serde_json::json!({ "version": "3.3", "manifest": manifest }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: create-landingzone expected 200, got {} body {:?}",
        created.status, created.body
    );
}

/// Enable one row's baseline on its target, handing back the `EnabledBaseline` ARN.
async fn enable(router: &MultiServiceRouter, case: &str, row: &Enablement) -> String {
    let mut body = serde_json::json!({
        "baselineIdentifier": row.baseline_identifier,
        "baselineVersion": row.baseline_version,
        "targetIdentifier": row.target_identifier,
    });
    if let Some(parameters) = row.parameters.clone() {
        body["parameters"] = parameters;
    }
    let enabled = call(router, "/enable-baseline", body).await;
    assert_eq!(
        enabled.status, 200,
        "case {case:?}: enable-baseline on {} expected 200, got {} body {:?}",
        row.target_identifier, enabled.status, enabled.body
    );
    enabled.string("arn")
}

/// POST `/list-enabled-baselines` with `body`, asserting the call succeeded.
async fn list(router: &MultiServiceRouter, case: &str, body: serde_json::Value) -> Answer {
    let listed = call(router, "/list-enabled-baselines", body.clone()).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?} ({body}): list-enabled-baselines expected 200, got {} body {:?}",
        listed.status, listed.body
    );
    listed
}

/// A member of a JSON object as a `&str`.
fn text<'a>(value: &'a serde_json::Value, member: &str) -> Option<&'a str> {
    value.get(member).and_then(serde_json::Value::as_str)
}

/// The `arn` of every entry of a listing, sorted, so a case compares SETS without
/// depending on the order the ARNs were minted in.
fn listed_arns(listed: &Answer) -> Vec<String> {
    let mut arns: Vec<String> = entries(listed.get("enabledBaselines"))
        .iter()
        .map(|entry| text(entry, "arn").unwrap_or_default().to_string())
        .collect();
    arns.sort();
    arns
}

/// The `targetIdentifier` of every entry of a listing, sorted.
fn listed_targets(listed: &Answer) -> Vec<String> {
    let mut targets: Vec<String> = entries(listed.get("enabledBaselines"))
        .iter()
        .map(|entry| {
            text(entry, "targetIdentifier")
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    targets.sort();
    targets
}

/// `GetEnabledBaseline`'s `enabledBaselineDetails` for `identifier`, asserting it
/// resolved.
async fn details(router: &MultiServiceRouter, case: &str, identifier: &str) -> serde_json::Value {
    let got = call(
        router,
        "/get-enabled-baseline",
        serde_json::json!({ "enabledBaselineIdentifier": identifier }),
    )
    .await;
    assert_eq!(
        got.status, 200,
        "case {case:?}: get-enabled-baseline {identifier} expected 200, got {} body {:?}",
        got.status, got.body
    );
    got.get("enabledBaselineDetails").clone()
}

/// RULE: a reported `EnabledBaseline` carries as its `parentIdentifier` the enablement it
/// inherited its configuration from, and `includeChildren` decides whether the enablements
/// carrying a parent are listed — so for the enablements `EnableBaseline` applies directly
/// to their targets, which inherit from nothing, the listing is the same whichever way the
/// flag is sent, every entry reports no parent, and no enablement is reported for an
/// account the landing zone designates.
#[tokio::test]
async fn the_listing_reports_the_parent_each_enablement_inherits_from() {
    for row in governances() {
        let case = row.case;
        let router = emulator();
        match row.accounts {
            // The rows that name accounts deploy a landing zone designating them; the one
            // that names none still deploys the shared manifest, because Control Tower
            // enables a baseline only for an account it has been set up in.
            Some(accounts) => deploy_landing_zone(&router, case, accounts).await,
            None => {
                set_up_account(&router).await;
            }
        }

        let mut expected: Vec<String> = Vec::new();
        for enablement in &row.enablements {
            expected.push(enable(&router, case, enablement).await);
        }
        expected.sort();

        // `includeChildren` sent either way, and omitted (the modelled default is false),
        // reports the enablements the store holds: none of them inherited its configuration
        // from another one, so none is a child the flag adds.
        let mut bodies: Vec<(String, serde_json::Value)> = Vec::new();
        for (name, body) in [
            ("includeChildren omitted", serde_json::json!({})),
            (
                "includeChildren false",
                serde_json::json!({ "includeChildren": false }),
            ),
            (
                "includeChildren true",
                serde_json::json!({ "includeChildren": true }),
            ),
        ] {
            let listed = list(&router, case, body).await;
            assert_eq!(
                listed_arns(&listed),
                expected,
                "case {case:?}, {name}: expected exactly the enablements the enables \
                 minted, got {:?}",
                listed.body
            );
            // Every entry is one of the enablements a caller applied, on the TARGET that
            // call named: no enablement is reported for a member account, so nothing here
            // is a deployment the service does not hold.
            let mut targets: Vec<String> = row
                .enablements
                .iter()
                .map(|enablement| enablement.target_identifier.to_string())
                .collect();
            targets.sort();
            assert_eq!(
                listed_targets(&listed),
                targets,
                "case {case:?}, {name}: expected every entry to target the organizational \
                 unit its enable named, got {:?}",
                listed.body
            );
            for entry in entries(listed.get("enabledBaselines")) {
                assert!(
                    entry.get("parentIdentifier").is_none(),
                    "case {case:?}, {name}: an enablement applied directly to its target \
                     inherits from nothing, so it must report no parentIdentifier, got \
                     {entry:?}"
                );
            }
            bodies.push((name.to_string(), listed.body.clone()));
        }
        let (first_name, first_body) = &bodies[0];
        for (name, body) in &bodies[1..] {
            assert_eq!(
                body, first_body,
                "case {case:?}: {name} must report the same listing as {first_name} while \
                 no enablement is a child of another, got {body:?}"
            );
        }

        // The same member through `GetEnabledBaseline`, which the model has return it.
        for arn in &expected {
            let detail = details(&router, case, arn).await;
            assert!(
                detail.get("parentIdentifier").is_none(),
                "case {case:?}: expected GetEnabledBaseline to report no parentIdentifier \
                 for {arn}, got {detail:?}"
            );
        }

        // And no inherited enablement exists under an ARN of its own: an account the
        // landing zone designates is the target such an enablement would carry, and
        // nothing reports one.
        let designated: Vec<String> = row
            .accounts
            .map(|(logging, security)| vec![logging, security])
            .unwrap_or_default()
            .iter()
            .flat_map(|account| {
                row.enablements
                    .iter()
                    .map(|enablement| account_target(enablement.target_identifier, account))
                    .collect::<Vec<String>>()
            })
            .collect();
        let widest = list(&router, case, serde_json::json!({ "includeChildren": true })).await;
        for account in &designated {
            assert!(
                !listed_targets(&widest).contains(account),
                "case {case:?}: no enablement is recorded for the account {account}, so \
                 none may be reported for it, got {:?}",
                widest.body
            );
        }

        // `parentIdentifiers` selects on the parent an entry reports, so with no entry
        // carrying one it selects nothing — for a value of every form a caller could send
        // it, alone and up to the modelled maximum of five, with the flag either way, and
        // beside the members that do select.
        let mut values: Vec<String> = expected.clone();
        values.extend(
            row.enablements
                .iter()
                .map(|enablement| enablement.target_identifier.to_string()),
        );
        values.extend(designated.iter().cloned());
        let first_enablement = &row.enablements[0];
        for include_children in [false, true] {
            for value in &values {
                let listed = list(
                    &router,
                    case,
                    serde_json::json!({
                        "includeChildren": include_children,
                        "filter": { "parentIdentifiers": [value] }
                    }),
                )
                .await;
                assert!(
                    listed_arns(&listed).is_empty(),
                    "case {case:?}, includeChildren {include_children}: \
                     parentIdentifiers {value} must select nothing while no enablement \
                     reports a parent, got {:?}",
                    listed.body
                );
            }

            // The modelled maximum length of the filter list, and the filter beside the
            // members that DO select: the narrowing is the intersection, so a member
            // selecting nothing selects nothing however wide the rest is.
            let to_the_bound: Vec<String> = values.iter().take(5).cloned().collect();
            for (name, filter) in [
                (
                    "parentIdentifiers at its modelled maximum length",
                    serde_json::json!({ "parentIdentifiers": to_the_bound }),
                ),
                (
                    "parentIdentifiers beside the members that do select",
                    serde_json::json!({
                        "parentIdentifiers": [expected[0].clone()],
                        "targetIdentifiers": [first_enablement.target_identifier],
                        "baselineIdentifiers": [first_enablement.baseline_identifier],
                        "statuses": ["UNDER_CHANGE"]
                    }),
                ),
            ] {
                let listed = list(
                    &router,
                    case,
                    serde_json::json!({
                        "includeChildren": include_children,
                        "filter": filter
                    }),
                )
                .await;
                assert!(
                    listed_arns(&listed).is_empty(),
                    "case {case:?}, includeChildren {include_children}: {name} must select \
                     nothing, got {:?}",
                    listed.body
                );
            }
        }

        // The members that DO select still select, so the cases above are not an empty
        // listing dressed up as a narrowing.
        let narrowed = list(
            &router,
            case,
            serde_json::json!({
                "includeChildren": true,
                "filter": { "targetIdentifiers": [first_enablement.target_identifier] }
            }),
        )
        .await;
        assert_eq!(
            listed_targets(&narrowed),
            vec![first_enablement.target_identifier.to_string()],
            "case {case:?}: expected targetIdentifiers to select the enablement on that \
             target, got {:?}",
            narrowed.body
        );

        // Once the clock has made every enable due the listing is unchanged in this
        // respect: a deployed enablement still inherits from nothing.
        tokio::time::sleep(past_the_settle_span()).await;
        let settled = list(&router, case, serde_json::json!({ "includeChildren": true })).await;
        assert_eq!(
            listed_arns(&settled),
            expected,
            "case {case:?}: expected the same enablements once every enable is due, got {:?}",
            settled.body
        );
        for entry in entries(settled.get("enabledBaselines")) {
            assert!(
                entry.get("parentIdentifier").is_none(),
                "case {case:?}: expected no parentIdentifier on a deployed enablement, got \
                 {entry:?}"
            );
        }
    }
}
