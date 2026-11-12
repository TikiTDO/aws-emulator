// SPDX-License-Identifier: CC0-1.0

//! A connector's two identity members name the SAME resource, and a request refused by the
//! state of the resource says so in the service's own words.
//!
//! Two rules are pinned here, both about what an answer says rather than about what it
//! stores:
//!
//! 1. `NetworkConnectorArn`'s resource part is `network-connector:<resource>` and `Id` is
//!    "assigned by the service at creation time": the resource segment IS that Id, so
//!    every ARN a connector is reported under — `Arn`, `VersionArn`, `LatestVersionArn`,
//!    the ARNs its list summaries carry — ends in the Id of the same answer, and a client
//!    can build either identity member from the other. The requirement is the
//!    self-consistency of the answer, not any particular id text.
//! 2. a request refused because the resource is in the wrong state names the resource the
//!    way the service names it: an update refused while the connector is not `ACTIVE` names
//!    it by that same `Id` and states the state it is in, and a delete refused because the
//!    connector is in a state it cannot be deleted from states the state and nothing else.

use serde_json::{json, Value};

use crate::network_connector_lifecycle::{
    assert_conflict, at, create, delete, emulator, expected_arn, get, list, list_versions,
    minimal_create, text, update, vpc_egress, wait_for_the_clock,
};

/// What every ARN of a connector carries before the identifier of the resource.
const RESOURCE_PREFIX: &str = ":network-connector:";

/// Every ARN `answer` reports a connector under, paired with the member it came from.
fn reported_arns(answer: &Value) -> Vec<(&'static str, String)> {
    ["/Arn", "/VersionArn", "/LatestVersionArn"]
        .into_iter()
        .filter(|member| !text(answer, member).is_empty())
        .map(|member| (member, text(answer, member)))
        .collect()
}

/// RULE 1: in every answer that carries both, the `Arn`'s resource segment is the `Id`,
/// and every qualified ARN beside it is that ARN plus a version suffix.
///
/// The rows differ along what the rule generalises over: the name (including one that is
/// nothing like the id and one at the shortest admissible length), how many subnets and
/// groups the configuration carries, and how many versions the connector holds when it is
/// read — so an ARN built from anything but the Id fails.
#[tokio::test]
async fn every_arn_a_connector_is_reported_under_names_its_id() {
    let rows: Vec<(&str, &str, usize)> = vec![
        ("a plain name, one version", "alpha", 0),
        ("a name with underscores, one version", "beta_two_01", 0),
        ("the shortest name the model admits", "a", 0),
        ("a long descriptive name, two versions", "egress-to-the-data-tier", 1),
        ("a name made only of digits, three versions", "1234567890", 2),
    ];

    for (case, name, extra_versions) in rows {
        let router = emulator();
        let created = create(&router, minimal_create(name)).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the create must succeed, got {} with {:?}",
            created.status, created.body
        );
        let id = text(&created.body, "/Id");
        let arn = text(&created.body, "/Arn");
        assert_eq!(
            arn,
            expected_arn(&id),
            "case {case:?}: the ARN's resource segment is the Id the same answer reports, \
             got {arn:?} beside {id:?}"
        );
        assert!(
            arn.ends_with(&format!("{RESOURCE_PREFIX}{id}")),
            "case {case:?}: expected the ARN to end in {RESOURCE_PREFIX}{id}, got {arn:?}"
        );
        assert!(
            !arn.contains(&format!("{RESOURCE_PREFIX}{name}")) || name == id,
            "case {case:?}: the resource segment is the Id, not the Name, got {arn:?}"
        );

        // Each extra version is appended by an update, so the qualified ARNs below span
        // more than the first version.
        wait_for_the_clock();
        for extra in 0..extra_versions {
            let subnets: Vec<String> = (0..=extra)
                .map(|index| format!("subnet-0123456789abcde{index:01x}"))
                .collect();
            let borrowed: Vec<&str> = subnets.iter().map(String::as_str).collect();
            let updated = update(
                &router,
                name,
                json!({ "Configuration": vpc_egress(&borrowed, &[], None, None) }),
            )
            .await;
            assert_eq!(
                updated.status, 202,
                "case {case:?}: each update must succeed, got {} with {:?}",
                updated.status, updated.body
            );
            wait_for_the_clock();
        }

        // Every operation that reports the connector reports the same consistent pair,
        // whichever identifier form the request addressed it by.
        for addressed in [name.to_string(), id.clone(), arn.clone()] {
            let fetched = get(&router, &addressed).await;
            assert_eq!(
                text(&fetched.body, "/Id"),
                id,
                "case {case:?}: {addressed} must resolve to the same connector"
            );
            for (member, reported) in reported_arns(&fetched.body) {
                assert!(
                    reported == arn || reported.starts_with(&format!("{arn}:")),
                    "case {case:?}: {member} must be the connector's own ARN, optionally \
                     version-qualified, got {reported:?} against {arn:?}"
                );
                assert!(
                    reported.contains(&format!("{RESOURCE_PREFIX}{id}")),
                    "case {case:?}: {member} must name the connector by its Id, got \
                     {reported:?}"
                );
            }
        }

        // The list projections carry the same pair.
        let listed = list(&router, "").await;
        let summary = listed.body["NetworkConnectors"]
            .as_array()
            .and_then(|summaries| summaries.first().cloned())
            .unwrap_or(Value::Null);
        assert_eq!(
            text(&summary, "/Arn"),
            arn,
            "case {case:?}: the list summary reports the same ARN, got {:?}",
            at(&summary, "/Arn")
        );
        assert_eq!(
            text(&summary, "/Id"),
            id,
            "case {case:?}: the list summary reports the same Id, got {:?}",
            at(&summary, "/Id")
        );
        for (member, reported) in reported_arns(&summary) {
            assert!(
                reported.contains(&format!("{RESOURCE_PREFIX}{id}")),
                "case {case:?}: the summary's {member} must name the connector by its Id, \
                 got {reported:?}"
            );
        }

        let versions = list_versions(&router, name, "").await;
        for version in versions.body["NetworkConnectorVersions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            assert_eq!(
                text(&version, "/Arn"),
                arn,
                "case {case:?}: a version summary reports the connector's ARN, got {:?}",
                at(&version, "/Arn")
            );
            assert!(
                text(&version, "/VersionArn")
                    .starts_with(&format!("{arn}:")),
                "case {case:?}: a version's qualified ARN is the connector's ARN plus the \
                 version suffix, got {:?}",
                at(&version, "/VersionArn")
            );
        }

        // The ARN a client BUILDS from the Id addresses the connector, which is the point
        // of the two members agreeing.
        let built = get(&router, &expected_arn(&id)).await;
        assert_eq!(
            built.status, 200,
            "case {case:?}: an ARN built from the Id must resolve, got {} with {:?}",
            built.status, built.body
        );
    }
}

/// RULE 2: a connector that is not `ACTIVE` refuses an update with the modelled 409,
/// naming the connector by its `Id` and the state it is in; a connector in a state it
/// cannot be deleted from refuses the delete with the sentence that states that state.
///
/// The rows differ along the state the connector is in when the request arrives, and the
/// identifier form the request addresses it by — the refusal names the resource the way
/// the service names it, whichever of the three forms the caller used.
#[tokio::test]
async fn a_state_conflict_names_the_connector_by_its_id() {
    // A connector is PENDING from the create until the clock owes it ACTIVE, and DELETING
    // from an accepted delete until the removal falls due: the two non-ACTIVE states a
    // caller can observe through this API.
    let addressed: [&str; 3] = ["the name", "the id", "the unqualified ARN"];

    for form in addressed {
        let router = emulator();
        let created = create(&router, minimal_create("conflicted")).await;
        let id = text(&created.body, "/Id");
        let arn = text(&created.body, "/Arn");
        let identifier = match form {
            "the id" => id.clone(),
            "the unqualified ARN" => arn.clone(),
            _ => "conflicted".to_string(),
        };

        // PENDING: the update is refused, and so is the delete.
        let refused = update(
            &router,
            &identifier,
            json!({ "Configuration": vpc_egress(&["subnet-0fedcba9876543210"], &[], None, None) }),
        )
        .await;
        assert_conflict(
            &format!("an update addressed by {form} while PENDING"),
            &refused,
            &format!(
                "Network connector {id} is in state Pending and cannot be updated. Only \
                 ACTIVE connectors can be updated."
            ),
        );
        let refused_delete = delete(&router, &identifier).await;
        assert_conflict(
            &format!("a delete addressed by {form} while PENDING"),
            &refused_delete,
            "Network connector is in PENDING state and cannot be deleted",
        );

        // Neither refusal moved the connector, so the next read still reports PENDING.
        let fetched = get(&router, &identifier).await;
        assert_eq!(
            text(&fetched.body, "/State"),
            "PENDING",
            "case {form:?}: a refused request must not move the connector, got {:?}",
            at(&fetched.body, "/State")
        );

        // DELETING: the same rule, one state along. The delete is accepted once the
        // connector is ACTIVE, and an update is refused for as long as the removal is owed.
        wait_for_the_clock();
        let accepted = delete(&router, &identifier).await;
        assert_eq!(
            accepted.status, 202,
            "case {form:?}: an ACTIVE connector is deletable, got {} with {:?}",
            accepted.status, accepted.body
        );
        let while_deleting = update(
            &router,
            &identifier,
            json!({ "Configuration": vpc_egress(&["subnet-00112233445566778"], &[], None, None) }),
        )
        .await;
        assert_conflict(
            &format!("an update addressed by {form} while DELETING"),
            &while_deleting,
            &format!(
                "Network connector {id} is in state Deleting and cannot be updated. Only \
                 ACTIVE connectors can be updated."
            ),
        );
    }
}
