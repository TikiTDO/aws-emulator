// SPDX-License-Identifier: CC0-1.0

//! The SUBJECT of a filter expression — the identifier between `INCLUDE`/`EXCLUDE` and
//! `WHERE` — states which catalog the filter selects, so two rules follow, and each is
//! asserted over a table rather than one expression:
//!
//! 1. The subject slot admits the two identifiers the expression language defines and
//!    nothing else, checked where the filter is created, so a stored filter always
//!    names the entity it selects.
//! 2. A stored filter is applicable only to recommendations made of the entity its
//!    subject names: a request naming one whose subject names the other entity is
//!    refused (HTTP 400 `InvalidInputException`), for either filter kind and over
//!    every operation that takes a `filterArn`.
//!
//! The rows of rule 2 are every (operation, subject, clause dataset) combination, so
//! the agreeing combinations are asserted to be served as well as the disagreeing ones
//! refused.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    self as registry, filter, Campaign, Entity, RecipeType,
};

use crate::support::{
    actions, ids, items, Emulator, ACTION_RECOMMENDATIONS, CHANNELS, COLUMN_GENRE, GENRES,
    PERSONALIZE_RANKING, RECOMMENDATIONS,
};

/// A `USER_PERSONALIZATION` campaign, served by `GetRecommendations`.
const CAMPAIGN_USER: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/user-personalization";
/// A `PERSONALIZED_RANKING` campaign, served by `GetPersonalizedRanking`.
const CAMPAIGN_RANKING: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/ranking";
/// A `PERSONALIZED_ACTIONS` campaign, served by `GetActionRecommendations`.
const CAMPAIGN_ACTIONS: &str = "arn:aws:personalize:us-east-1:000000000000:campaign/actions";

/// How many items the seeded Items catalog holds.
const CATALOG_SIZE: usize = 40;
/// How many actions the seeded Actions catalog holds.
const ACTION_CATALOG_SIZE: usize = 12;

/// The `GENRE` an Items clause of these suites selects on.
fn selected_genre() -> &'static str {
    GENRES[0]
}

/// The `CHANNEL` an Actions clause of these suites selects on.
fn selected_channel() -> &'static str {
    CHANNELS[0]
}

/// An emulator holding one campaign per operation under test.
fn seeded() -> Emulator {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;
    for (arn, recipe_type) in [
        (CAMPAIGN_USER, RecipeType::UserPersonalization),
        (CAMPAIGN_RANKING, RecipeType::PersonalizedRanking),
    ] {
        assert!(
            registry::register_campaign(
                engine,
                Campaign::new(arn, "demo", recipe_type, items(CATALOG_SIZE))
            ),
            "case {arn}: the campaign seam must register the campaign"
        );
    }
    assert!(
        registry::register_campaign(
            engine,
            Campaign::new_actions(CAMPAIGN_ACTIONS, "actions", actions(ACTION_CATALOG_SIZE))
        ),
        "the campaign seam must register the action campaign"
    );
    emulator
}

#[test]
fn the_subject_slot_admits_only_the_two_identifiers_the_grammar_defines() {
    let emulator = Emulator::new();
    let engine = &*emulator.engine;

    // The two identifiers the expression language defines for the slot, and the entity
    // each selects. The rows vary the filter kind, the clause's dataset and the
    // predicate, so what they share is the subject.
    let accepted: [(&str, &str, Entity); 6] = [
        (
            "ItemID with a set predicate",
            "INCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\")",
            Entity::Items,
        ),
        (
            "ItemID with an ordering comparison",
            "EXCLUDE ItemID WHERE Items.YEAR >= 2010",
            Entity::Items,
        ),
        (
            "ItemID with a placeholder",
            "INCLUDE ItemID WHERE Items.GENRE IN ($GENRE)",
            Entity::Items,
        ),
        (
            "ActionID with a set predicate",
            "INCLUDE ActionID WHERE Actions.CHANNEL IN (\"App\", \"Email\")",
            Entity::Actions,
        ),
        (
            "ActionID with a negated predicate",
            "EXCLUDE ActionID WHERE Actions.CHANNEL NOT IN (\"App\")",
            Entity::Actions,
        ),
        (
            "ActionID with two clauses",
            "INCLUDE ActionID WHERE Actions.VALUE >= 1 AND Actions.CHANNEL = \"App\"",
            Entity::Actions,
        ),
    ];

    for (case, expression, entity) in accepted {
        let arn = format!("arn:aws:personalize:us-east-1:000000000000:filter/ok-{case}");
        assert_eq!(
            registry::register_filter(engine, &arn, expression),
            Ok(true),
            "case {case}: {expression:?} names a subject the grammar defines and must be accepted"
        );
        let stored = registry::stored_filter(engine, &arn)
            .unwrap_or_else(|| panic!("case {case}: the accepted filter must be stored"));
        assert_eq!(
            stored.parsed.subject,
            entity,
            "case {case}: expected the stored expression to select {:?}, got {:?}",
            entity,
            stored.parsed.subject
        );
        assert_eq!(
            stored.parsed.subject.subject(),
            entity.subject(),
            "case {case}: expected the subject to read back as {}, got {}",
            entity.subject(),
            stored.parsed.subject.subject()
        );
    }

    // Anything else in the slot is a malformed expression, refused where the filter is
    // created. The rows vary the FORM of the word that is not one of the two
    // identifiers: a word of its own, a dataset name, another dataset's id column,
    // either spelled in another case, the keyword that should follow the subject, a
    // placeholder and a quoted string.
    let refused = [
        (
            "a word the grammar does not define",
            "INCLUDE Nonsense WHERE Items.GENRE IN (\"Comedy\")",
        ),
        (
            "the dataset name in the subject slot",
            "INCLUDE Items WHERE Items.GENRE IN (\"Comedy\")",
        ),
        (
            "another dataset's id column",
            "INCLUDE UserID WHERE Items.GENRE IN (\"Comedy\")",
        ),
        (
            "a lowercase spelling",
            "INCLUDE itemid WHERE Items.GENRE IN (\"Comedy\")",
        ),
        (
            "an uppercase spelling",
            "EXCLUDE ACTIONID WHERE Actions.CHANNEL IN (\"App\")",
        ),
        (
            "no subject at all",
            "INCLUDE WHERE Items.GENRE IN (\"Comedy\")",
        ),
        (
            "a placeholder in the subject slot",
            "INCLUDE $SUBJECT WHERE Items.GENRE IN (\"Comedy\")",
        ),
        (
            "a quoted subject",
            "INCLUDE \"ItemID\" WHERE Items.GENRE IN (\"Comedy\")",
        ),
    ];

    for (case, expression) in refused {
        let arn = format!("arn:aws:personalize:us-east-1:000000000000:filter/bad-{case}");
        let outcome = registry::register_filter(engine, &arn, expression);
        let message = match &outcome {
            Err(message) => message.clone(),
            Ok(_) => panic!("case {case}: {expression:?} must be refused, got {outcome:?}"),
        };
        assert!(
            message.contains(filter::SUBJECT_ITEM_ID) && message.contains(filter::SUBJECT_ACTION_ID),
            "case {case}: expected the refusal to name both identifiers the slot admits, got \
             {message:?}"
        );
        assert!(
            registry::stored_filter(engine, &arn).is_none(),
            "case {case}: a refused expression must store no filter"
        );
    }
}

/// One operation under test: its route, the campaign it serves, the entity its
/// recommendations are made of, and the response members that carry them.
struct Operation {
    route: &'static str,
    campaign: &'static str,
    entity: Entity,
    list_member: &'static str,
    id_member: &'static str,
}

/// Every operation that takes a `filterArn`.
fn operations() -> [Operation; 3] {
    [
        Operation {
            route: RECOMMENDATIONS,
            campaign: CAMPAIGN_USER,
            entity: Entity::Items,
            list_member: "itemList",
            id_member: "itemId",
        },
        Operation {
            route: PERSONALIZE_RANKING,
            campaign: CAMPAIGN_RANKING,
            entity: Entity::Items,
            list_member: "personalizedRanking",
            id_member: "itemId",
        },
        Operation {
            route: ACTION_RECOMMENDATIONS,
            campaign: CAMPAIGN_ACTIONS,
            entity: Entity::Actions,
            list_member: "actionList",
            id_member: "actionId",
        },
    ]
}

/// The request body for `operation`, naming `filter_arn`.
fn request(operation: &Operation, filter_arn: &str) -> Value {
    let mut body = json!({
        "campaignArn": operation.campaign,
        "userId": "user-1",
        "filterArn": filter_arn,
    });
    if operation.route == PERSONALIZE_RANKING {
        // Two entries the clause selects and one it does not.
        body["inputList"] = json!(["item-0", "item-1", "item-4"]);
    }
    body
}

/// The ids of the entities `entity`'s clause below selects out of the seeded catalogs.
fn selected_ids(entity: Entity) -> Vec<String> {
    match entity {
        Entity::Items => items(CATALOG_SIZE)
            .iter()
            .filter(|item| {
                item.attributes.get(COLUMN_GENRE).map(String::as_str) == Some(selected_genre())
            })
            .map(|item| item.item_id.clone())
            .collect(),
        Entity::Actions => actions(ACTION_CATALOG_SIZE)
            .iter()
            .filter(|action| {
                action.attributes.get("CHANNEL").map(String::as_str) == Some(selected_channel())
            })
            .map(|action| action.action_id.clone())
            .collect(),
    }
}

#[tokio::test]
async fn a_filter_applies_only_to_recommendations_of_the_entity_its_subject_names() {
    let emulator = seeded();
    let engine = &*emulator.engine;

    // One filter per (subject, clause dataset) combination, in both kinds: the subject
    // and the dataset vary independently, which is what makes the subject's own rule
    // observable.
    let mut registered: Vec<(Entity, Entity, &str, String)> = Vec::new();
    for subject in [Entity::Items, Entity::Actions] {
        for clause_entity in [Entity::Items, Entity::Actions] {
            for kind in ["INCLUDE", "EXCLUDE"] {
                let clause = match clause_entity {
                    Entity::Items => format!("Items.{COLUMN_GENRE} IN (\"{}\")", selected_genre()),
                    Entity::Actions => format!("Actions.CHANNEL IN (\"{}\")", selected_channel()),
                };
                let expression = format!("{kind} {} WHERE {clause}", subject.subject());
                let arn = format!(
                    "arn:aws:personalize:us-east-1:000000000000:filter/{kind}-{}-{}",
                    subject.subject(),
                    clause_entity.dataset()
                );
                assert_eq!(
                    registry::register_filter(engine, &arn, expression.clone()),
                    Ok(true),
                    "case {expression:?}: the expression is well formed and must be stored"
                );
                registered.push((subject, clause_entity, kind, arn));
            }
        }
    }

    for operation in operations() {
        for (subject, clause_entity, kind, arn) in &registered {
            let case = format!(
                "{} with a {kind} filter selecting {} over {}",
                operation.route,
                subject.subject(),
                clause_entity.dataset()
            );
            let body = request(&operation, arn);

            if *subject != operation.entity {
                // The subject names the other entity, so the filter does not select
                // what this operation recommends — whatever its clauses read.
                let refusal = emulator.refusal(operation.route, body).await;
                let expected = format!(
                    "the filter selects {}, which is not the {} this operation recommends",
                    subject.subject(),
                    operation.entity.subject()
                );
                assert_eq!(
                    (refusal.status, refusal.code.as_str()),
                    (400, "InvalidInputException"),
                    "case {case}: expected 400 InvalidInputException, got {refusal:?}"
                );
                assert_eq!(
                    refusal.message, expected,
                    "case {case}: expected {expected:?}, got {:?}",
                    refusal.message
                );
                continue;
            }

            if *clause_entity != operation.entity {
                // The subject agrees, so the refusal is the one the clause's dataset
                // earns: it cannot be evaluated against this entity's attributes.
                let refusal = emulator.refusal(operation.route, body).await;
                assert_eq!(
                    (refusal.status, refusal.code.as_str()),
                    (400, "InvalidInputException"),
                    "case {case}: expected 400 InvalidInputException, got {refusal:?}"
                );
                assert!(
                    refusal.message.contains(clause_entity.dataset()),
                    "case {case}: expected the refusal to name the {} dataset, got {:?}",
                    clause_entity.dataset(),
                    refusal.message
                );
                continue;
            }

            // Subject and clause both name what this operation recommends, so the
            // filter is applied: every reported entity is one the expression keeps.
            let response = emulator.ok(operation.route, body).await;
            let returned = ids(&response, operation.list_member, operation.id_member);
            let selected = selected_ids(operation.entity);
            for id in &returned {
                let kept = if *kind == "INCLUDE" {
                    selected.contains(id)
                } else {
                    !selected.contains(id)
                };
                assert!(
                    kept,
                    "case {case}: {id} is not an entity a {kind} filter keeps, got {returned:?}"
                );
            }
            assert!(
                !returned.is_empty(),
                "case {case}: the filter keeps some of the catalog, got {returned:?}"
            );
        }
    }
}
