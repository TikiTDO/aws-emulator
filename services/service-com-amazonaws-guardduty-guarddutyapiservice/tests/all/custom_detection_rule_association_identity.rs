// SPDX-License-Identifier: CC0-1.0

//! The IDENTITY of a custom detection rule association — the `AssociationId` and the
//! `Arn` that carries it — end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. An association has ONE `AssociationId` for as long as it exists, and every
//!    payload that names the association repeats it: the create that minted it, each
//!    read reached through it, and every listing that reports it — `AssociationId` is
//!    "the unique identifier for the association"
//!    (`smithy_query(shape="AssociationDetail,AssociationSummary")`), so a read is
//!    identity-preserving and a report cannot hand back a handle other than the one it
//!    was reached through. Asserted over every published rule, in both
//!    `AssociationMode`s, through two Regions, and over the unfiltered listing as well
//!    as the two filters the operation takes.
//! 2. Inside ONE payload the `Arn`'s trailing `/association/<id>` segment is that
//!    payload's own `AssociationId`: the ARN is "the Amazon Resource Name (ARN) of the
//!    association" (`smithy_query(shape="AssociationDetail")`), so selecting an entry by
//!    its ARN and acting on it by its id address the same resource.
//! 3. The id is the association's ONLY handle: it resolves the one association of the
//!    one rule and Region it was minted for, and a well-formed id naming no association
//!    of that rule — another association's id, or one no create ever minted — is the
//!    modelled 404 (`smithy_query(operation="GetCustomDetectionRuleAssociation")`).
//! 4. The identity outlives a mutation and dies with the resource: an update that moves
//!    the mode leaves the id and the ARN exactly where they were, and after a delete the
//!    id stops resolving and its ARN leaves the listing — a later create for the same
//!    rule mints a FRESH handle, which is why the digits of an id are a per-run value
//!    and only its form and its stability are requirements.
//!
//! Every `AssociationId` here is read off the emulator's own response: none is written
//! into the test, because the value is minted per run.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, error_code, new_router, published_rule_ids, Router};

/// `CreateCustomDetectionRuleAssociation` and `ListCustomDetectionRuleAssociations`
/// are both bound at `/custom-detection-rule/association` (POST and GET)
/// (`smithy_query(operation="CreateCustomDetectionRuleAssociation,ListCustomDetectionRuleAssociations")`).
const ASSOCIATIONS_PATH: &str = "/custom-detection-rule/association";

/// The read, the update and the delete of one association are all bound at
/// `/custom-detection-rule/rule/{RuleId}/association/{AssociationId}`
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation,UpdateCustomDetectionRuleAssociation,DeleteCustomDetectionRuleAssociation")`).
const RULES_PATH: &str = "/custom-detection-rule/rule";

/// Every `AssociationMode` member (`smithy_query(shape="AssociationMode")`).
const MODES: [&str; 2] = ["DRY_RUN", "LIVE"];

/// Two Regions, so the Region an identity belongs to is asserted rather than assumed.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// `MaxResults` is `range 1..100`
/// (`smithy_query(operation="ListCustomDetectionRuleAssociations")`), so one page this
/// size holds every association a case makes.
const MAX_RESULTS: i64 = 100;

/// The longest an `AssociationId` may be: `len 1..64`
/// (`smithy_query(shape="AssociationDetail")`).
const ASSOCIATION_ID_MAX: usize = 64;

/// A well-formed `AssociationId` (`^[a-zA-Z0-9_-]{1,64}$`) that no create ever minted.
const ABSENT_ASSOCIATION_ID: &str = "deadbeefdeadbeefdeadbeefdeadbeef";

/// The modelled error shape an association this account does not hold is refused with
/// (`smithy_query(operation="GetCustomDetectionRuleAssociation")`).
const NOT_FOUND: &str = "ResourceNotFoundException";

/// Whether `value` satisfies the `AssociationId` constraints the model carries —
/// `^[a-zA-Z0-9_-]{1,64}$`, `len 1..64`
/// (`smithy_query(shape="AssociationDetail,AssociationSummary")`).
fn is_modelled_association_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= ASSOCIATION_ID_MAX
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
}

/// The id the ARN `arn` names, i.e. its trailing `/association/<id>` segment, or `None`
/// for an ARN that names no association.
pub(crate) fn arn_association_id(arn: &str) -> Option<&str> {
    let (head, id) = arn.rsplit_once('/')?;
    head.ends_with("/association").then_some(id)
}

/// The one identity a payload that names an association carries: its `associationId`,
/// asserted to satisfy the modelled constraints and to be exactly the id its own `arn`
/// names.
///
/// Every assertion of this suite goes through here, so the create, the read and each
/// listing entry are held to the same rule.
fn identity(payload: &Value, case: &str) -> String {
    let id = payload["associationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an associationId, got {payload:?}"))
        .to_string();
    assert!(
        is_modelled_association_id(&id),
        "case {case:?}: expected an AssociationId matching ^[a-zA-Z0-9_-]{{1,64}}$, got {id:?}"
    );
    let arn = payload["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an arn, got {payload:?}"));
    let named = arn_association_id(arn).unwrap_or_else(|| {
        panic!("case {case:?}: expected an arn naming an association, got {arn:?}")
    });
    assert_eq!(
        named, id,
        "case {case:?}: expected the arn to end in its own associationId {id:?}, got {arn:?}"
    );
    id
}

/// Create an association for `rule_id` in `mode` through `region`'s endpoint, failing
/// the test when the create does not succeed.
pub(crate) async fn associate(router: &Router, region: &str, rule_id: &str, mode: &str) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "POST",
        ASSOCIATIONS_PATH,
        Some(json!({"ruleId": rule_id, "mode": mode})),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-association {rule_id}/{mode} in {region}: expected 200, got {status} {body:?}"
    );
    body["ruleAssociation"].clone()
}

/// The `AssociationDetail` `GetCustomDetectionRuleAssociation` reports for the
/// `(rule_id, id)` pair, failing the test when the read does not succeed.
async fn read_association(router: &Router, region: &str, rule_id: &str, id: &str) -> Value {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("{RULES_PATH}/{rule_id}/association/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "get-association {rule_id}/{id} in {region}: expected 200, got {status} {body:?}"
    );
    body["ruleAssociation"].clone()
}

/// Every `AssociationSummary` the listing reports through `region`'s endpoint for the
/// query string `query`.
async fn listed_associations(router: &Router, region: &str, query: &str) -> Vec<Value> {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("{ASSOCIATIONS_PATH}{query}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "list-associations {query} in {region}: expected 200, got {status} {body:?}"
    );
    body["ruleAssociations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The listed entry whose own `arn` is `arn` — the way a caller discovers an
/// association it then acts on by id.
pub(crate) fn entry_with_arn(entries: &[Value], arn: &str, case: &str) -> Value {
    entries
        .iter()
        .find(|entry| entry["arn"].as_str() == Some(arn))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an entry with arn {arn:?}, got {entries:?}"))
}

/// The three listings that report an association: the whole Region's set and each
/// filter the operation takes, paired with the label a failure names.
fn listing_queries(rule_id: &str, mode: &str) -> Vec<(String, String)> {
    vec![
        (
            format!("?maxResults={MAX_RESULTS}"),
            "unfiltered listing".to_string(),
        ),
        (
            format!("?maxResults={MAX_RESULTS}&ruleId={rule_id}"),
            "listing filtered by rule".to_string(),
        ),
        (
            format!("?maxResults={MAX_RESULTS}&mode={mode}"),
            "listing filtered by mode".to_string(),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Rules 1-3: one id per association, repeated by every payload that names it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_payload_naming_an_association_repeats_its_one_id() {
    let router = new_router();
    let rules = published_rule_ids(&router).await;

    // Each row: a published rule, a Region, and the mode the association is made in —
    // the mode alternating with both indices, so every rule is exercised in both modes
    // across the two Regions.
    let mut minted: Vec<(&str, String, &str, String, String)> = Vec::new();

    for (region_index, region) in REGIONS.iter().enumerate() {
        for (rule_index, rule_id) in rules.iter().enumerate() {
            let mode = MODES[(rule_index + region_index) % MODES.len()];
            let case = format!("{rule_id}/{mode} in {region}");

            let created = associate(&router, region, rule_id, mode).await;
            let id = identity(&created, &case);
            let arn = created["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an arn, got {created:?}"))
                .to_string();

            // A read reached THROUGH the id reports that same id back, so the value a
            // caller sent as the path label and the value it gets in the body are
            // interchangeable.
            let read = read_association(&router, region, rule_id, &id).await;
            assert_eq!(
                identity(&read, &case),
                id,
                "case {case:?}: the read must echo the id it was reached through, got {read:?}"
            );
            assert_eq!(
                read, created,
                "case {case:?}: the read must report what the create minted, got {read:?}"
            );

            // Reading twice reports the same identity: a report changes nothing a later
            // caller sees.
            let again = read_association(&router, region, rule_id, &id).await;
            assert_eq!(
                again, created,
                "case {case:?}: a second read must report the same association, got {again:?}"
            );

            // Every listing that reports the association — selected by its own ARN, the
            // way list-then-act discovers it — advertises that same id.
            for (query, label) in listing_queries(rule_id, mode) {
                let case = format!("{case}: {label}");
                let entries = listed_associations(&router, region, &query).await;
                let entry = entry_with_arn(&entries, &arn, &case);
                assert_eq!(
                    identity(&entry, &case),
                    id,
                    "case {case:?}: the listed entry must advertise the id its own arn names, got {entry:?}"
                );
            }

            minted.push((region, rule_id.clone(), mode, id, arn));
        }
    }

    // One association, one id: no two associations of the Region's set, and none across
    // Regions, share a handle.
    for (index, (region, rule_id, _, id, _)) in minted.iter().enumerate() {
        for (other_region, other_rule, _, other_id, _) in minted.iter().skip(index + 1) {
            assert_ne!(
                id, other_id,
                "case {rule_id}/{region} vs {other_rule}/{other_region}: two associations must not share an id"
            );
        }
    }

    // The id is the association's ONLY handle: it resolves the one association it was
    // minted for, and naming it under another rule — or naming an id no create minted —
    // is the modelled 404.
    for (region, rule_id, _, id, _) in &minted {
        for (_, other_rule, _, other_id, _) in &minted {
            if other_rule == rule_id {
                continue;
            }
            let case = format!("{other_id} of {other_rule} read under {rule_id} in {region}");
            let (status, body) = call_in(
                &router,
                region,
                "GET",
                &format!("{RULES_PATH}/{rule_id}/association/{other_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected the modelled 404, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                NOT_FOUND,
                "case {case:?}: expected the {NOT_FOUND} class, got {body:?}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(format!("Association not found: {other_id}").as_str()),
                "case {case:?}: expected the not-found text naming the id asked for, got {body:?}"
            );
        }

        let case = format!("an id never minted, read under {rule_id} in {region}");
        let (status, body) = call_in(
            &router,
            region,
            "GET",
            &format!("{RULES_PATH}/{rule_id}/association/{ABSENT_ASSOCIATION_ID}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the modelled 404, got {status} {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(format!("Association not found: {ABSENT_ASSOCIATION_ID}").as_str()),
            "case {case:?}: expected the not-found text naming the id asked for, got {body:?}"
        );
        // The association the id really names is still reachable: a refused read moved
        // nothing.
        let still = read_association(&router, region, rule_id, id).await;
        assert_eq!(
            identity(&still, &case),
            *id,
            "case {case:?}: the association must still report its own id, got {still:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the identity outlives a mutation and dies with the resource.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_associations_id_survives_a_mode_update_and_dies_with_the_association() {
    let catalogue = new_router();
    let rules = published_rule_ids(&catalogue).await;
    let region = REGIONS[0];

    // Each row: a published rule, the mode it is created in and the mode an update then
    // asks for — so the table spans a move in both directions and an update that asks
    // for the mode the association is already in.
    for rule_id in &rules {
        for from in MODES {
            for to in MODES {
                let case = format!("{rule_id}: created {from}, updated to {to}");
                let router = new_router();

                let created = associate(&router, region, rule_id, from).await;
                let id = identity(&created, &case);
                let arn = created["arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an arn, got {created:?}"))
                    .to_string();

                let (status, body) = call_in(
                    &router,
                    region,
                    "PUT",
                    &format!("{RULES_PATH}/{rule_id}/association/{id}"),
                    Some(json!({"mode": to})),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: update-association expected 200, got {status} {body:?}"
                );

                // The mode is what the update sets; the identity is not its to move.
                let updated = read_association(&router, region, rule_id, &id).await;
                assert_eq!(
                    identity(&updated, &case),
                    id,
                    "case {case:?}: an update must not move the association's id, got {updated:?}"
                );
                assert_eq!(
                    updated["arn"].as_str(),
                    Some(arn.as_str()),
                    "case {case:?}: an update must not move the association's arn, got {updated:?}"
                );
                assert_eq!(
                    updated["mode"].as_str(),
                    Some(to),
                    "case {case:?}: the update must set the mode, got {updated:?}"
                );

                // The listing still reports the association under the same ARN with the
                // same id, now in the mode the update set.
                let entries =
                    listed_associations(&router, region, &format!("?maxResults={MAX_RESULTS}&mode={to}"))
                        .await;
                let entry = entry_with_arn(&entries, &arn, &case);
                assert_eq!(
                    identity(&entry, &case),
                    id,
                    "case {case:?}: the listing must keep advertising the same id, got {entry:?}"
                );

                // The handle dies with the resource.
                let (status, body) = call_in(
                    &router,
                    region,
                    "DELETE",
                    &format!("{RULES_PATH}/{rule_id}/association/{id}"),
                    None,
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: delete-association expected 200, got {status} {body:?}"
                );
                let (status, body) = call_in(
                    &router,
                    region,
                    "GET",
                    &format!("{RULES_PATH}/{rule_id}/association/{id}"),
                    None,
                )
                .await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: a deleted id must stop resolving, got {status} {body:?}"
                );
                let entries =
                    listed_associations(&router, region, &format!("?maxResults={MAX_RESULTS}")).await;
                assert!(
                    !entries
                        .iter()
                        .any(|entry| entry["arn"].as_str() == Some(arn.as_str())),
                    "case {case:?}: a deleted association must leave the listing, got {entries:?}"
                );

                // A later create for the same rule is a NEW association, so it carries a
                // handle of its own — self-consistent in its own payload, and not the one
                // the deleted association had.
                let remade = associate(&router, region, rule_id, to).await;
                let remade_id = identity(&remade, &case);
                assert_ne!(
                    remade_id, id,
                    "case {case:?}: a new association must not reuse the deleted one's id, got {remade:?}"
                );
                let read_back = read_association(&router, region, rule_id, &remade_id).await;
                assert_eq!(
                    identity(&read_back, &case),
                    remade_id,
                    "case {case:?}: the new association must read back under its own id, got {read_back:?}"
                );
            }
        }
    }
}
