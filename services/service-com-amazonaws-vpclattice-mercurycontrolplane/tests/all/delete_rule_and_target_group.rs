// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a DELETE leaves behind: `DeleteRule` and
//! `DeleteTargetGroup`.
//!
//! The rule under test: a delete REMOVES the record from this service's store, so every
//! later read of that identifier reports the modelled `ResourceNotFoundException` (404)
//! — the AWS message construction over the submitted identifier, `resourceId` naming it
//! and `resourceType` carrying the kind's wire token — and the keys the record held (a
//! rule's name and priority inside its listener, a target group's account-unique name)
//! are free for a new resource. Where AWS answers the delete with a non-terminal status,
//! the record goes once the span the clock owes its removal has passed, so a read in
//! between reports that transient status and never the resource's live one. A delete the
//! resource's state forbids changes nothing at all.
//!
//! Each case is a row of a TABLE spanning what the rule generalises over:
//!
//! * the resource KIND — a listener rule and a target group, the two kinds whose delete
//!   this suite covers;
//! * the identifier FORM the delete and the read use — the bare id and the resource ARN,
//!   the two forms every `*Identifier` member of this service models;
//! * the SHAPE of the deleted record — a rule's action kind, match form, priority and
//!   name; a target group's type, config and tags;
//! * the OPERATION that reports the removal — the Get, and the delete itself on a
//!   second call;
//! * the state that REFUSES a delete — a default rule, a rule reached through the wrong
//!   listener or service, a target group a listener rule still forwards to, and one
//!   whose creation is still in progress.
//!
//! Nothing here asserts on the wall clock: a transition the clock owes is waited for by
//! polling the resource until the CONDITION holds, under a deadline far longer than any
//! span this service schedules.

use std::time::{Duration, Instant};

use crate::list_rules::default_rule_id;
use crate::listeners::{a_service, create_listener, target_group};
use crate::resource_not_found_messages::{absent_id, assert_not_found};
use crate::rules::{a_listener, assert_refused, create_rule, fixed_response_rule, get_rule};
use crate::target_groups::{create_target_group, get_target_group, A_VPC};
use crate::vpclattice_service_slice::{as_label, expected_arn, message_of, Answer, Lattice};

/// AWS's resource-kind token for the two kinds this suite deletes, written out here
/// rather than read from the service, so no row can pass by agreeing with the code under
/// test. Source: the recorded vpc-lattice `ResourceNotFoundException` bodies, which spell
/// the kind as its CamelCase resource name ("Rule rule-… cannot be found.", "TargetGroup
/// tg-… cannot be found.").
const LABEL_RULE: &str = "Rule";
/// See [`LABEL_RULE`].
const LABEL_TARGET_GROUP: &str = "TargetGroup";

/// The `resourceType` token the same two kinds carry: the resource type in upper snake
/// case, which is NOT the lowercase ARN segment of the resource's own ARN. Source:
/// recorded vpc-lattice error bodies.
const TYPE_RULE: &str = "RULE";
/// See [`TYPE_RULE`].
const TYPE_TARGET_GROUP: &str = "TARGET_GROUP";

/// The id prefixes of those kinds, from the modelled id patterns (`^rule-[0-9a-z]{17}$`,
/// `^tg-[0-9a-z]{17}$`).
const PREFIX_RULE: &str = "rule-";

/// The ARN resource segment of a target group (`TargetGroupArn`'s modelled pattern,
/// `…:targetgroup/tg-…`).
const ARN_KIND_TARGET_GROUP: &str = "targetgroup";

/// The non-terminal status a delete of a target group answers with
/// (`TargetGroupStatus`: ACTIVE | CREATE_FAILED | CREATE_IN_PROGRESS | DELETE_FAILED |
/// DELETE_IN_PROGRESS).
const STATUS_DELETING: &str = "DELETE_IN_PROGRESS";
/// See [`STATUS_DELETING`]: the status a created target group reaches on its own.
const STATUS_ACTIVE: &str = "ACTIVE";
/// See [`STATUS_DELETING`]: the status a create answers with.
const STATUS_CREATING: &str = "CREATE_IN_PROGRESS";

/// How long a wait for a transition the clock owes keeps looking before it fails.
///
/// Far longer than any span this service schedules, so a loaded or slow host cannot fail
/// a case by being slow: the wait ends on the CONDITION, and this is only the backstop
/// that turns a transition that never happens into a failure instead of a hang.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a wait sleeps between two looks at the resource.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// `DELETE /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}`.
async fn delete_rule(
    lattice: &Lattice,
    service_identifier: &str,
    listener_identifier: &str,
    rule_identifier: &str,
) -> Answer {
    lattice
        .call(
            "DELETE",
            &format!(
                "/services/{service_identifier}/listeners/{listener_identifier}/rules/{rule_identifier}"
            ),
            None,
        )
        .await
}

/// `DELETE /targetgroups/{targetGroupIdentifier}`.
async fn delete_target_group(lattice: &Lattice, identifier: &str) -> Answer {
    lattice
        .call("DELETE", &format!("/targetgroups/{identifier}"), None)
        .await
}

/// The shared not-found assertion plus the kind's wire `resourceType` token, which that
/// assertion only checks the presence of.
fn assert_gone(answer: &Answer, case: &str, label: &str, wire_type: &str, identifier: &str) {
    assert_not_found(answer, case, label, identifier);
    assert_eq!(
        answer.body["resourceType"].as_str(),
        Some(wire_type),
        "case {case:?}: expected resourceType {wire_type:?}, got {}",
        answer.body
    );
}

/// A 409 `ConflictException` over the resource whose state refused the request.
fn assert_conflict(answer: &Answer, case: &str, resource_id: &str, wire_type: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case:?}: expected 409, got {} / {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ConflictException"),
        "case {case:?}: expected the ConflictException wire code, got {:?} / {}",
        answer.error_code,
        answer.body
    );
    assert_eq!(
        answer.body["resourceId"].as_str(),
        Some(resource_id),
        "case {case:?}: resourceId must name the refused resource {resource_id:?}, got {}",
        answer.body
    );
    assert_eq!(
        answer.body["resourceType"].as_str(),
        Some(wire_type),
        "case {case:?}: expected resourceType {wire_type:?}, got {}",
        answer.body
    );
    let message = message_of(case, answer);
    assert!(
        message.contains(resource_id),
        "case {case:?}: the refusal must name {resource_id:?}, got {message:?}"
    );
}

/// The status `GetTargetGroup` reports for `identifier`, or `None` once the read is the
/// modelled 404 — the whole answer is returned with it so a caller can assert on it.
async fn target_group_status(lattice: &Lattice, identifier: &str) -> (Answer, Option<String>) {
    let answer = get_target_group(lattice, identifier).await;
    let status = answer.body["status"].as_str().map(str::to_string);
    (answer, status)
}

/// Wait until `identifier` reports `STATUS_ACTIVE`, so a case that needs a target group
/// past its creation span does not depend on how fast this host ran the calls before it.
async fn wait_until_active(lattice: &Lattice, case: &str, identifier: &str) {
    let started = Instant::now();
    loop {
        let (answer, status) = target_group_status(lattice, identifier).await;
        if status.as_deref() == Some(STATUS_ACTIVE) {
            return;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: {identifier} never reached {STATUS_ACTIVE}; last answer {} / {}",
            answer.status,
            answer.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until `identifier` is GONE, answering the not-found the read ended on.
///
/// Every answer seen on the way is checked too: once a delete has started, a read reports
/// the transient delete status and never a live one again.
async fn wait_until_absent(lattice: &Lattice, case: &str, identifier: &str) -> Answer {
    let started = Instant::now();
    loop {
        let (answer, status) = target_group_status(lattice, identifier).await;
        if answer.status == 404 {
            return answer;
        }
        assert_eq!(
            status.as_deref(),
            Some(STATUS_DELETING),
            "case {case:?}: a read after the delete must report {STATUS_DELETING} until the \
             record is gone, got {} / {}",
            answer.status,
            answer.body
        );
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "case {case:?}: {identifier} was still readable; last answer {} / {}",
            answer.status,
            answer.body
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A rule a delete must remove, as a row of the table: what it is, the create body, and
/// whether the delete names it by its ARN rather than its bare id.
type RuleRow = (&'static str, serde_json::Value, bool);

/// The rule table: the action kind, the match form, each published bound of
/// `@range(1, 2000)` for `priority` and the name's shape all vary, and the delete names
/// the rule by each of the two modelled identifier forms. The recorded case — a rule
/// created and then deleted, read back by its id — is one row of it.
fn rule_rows() -> Vec<RuleRow> {
    vec![
        (
            "fixed response, lowest priority, prefix match, deleted by id",
            fixed_response_rule("del-fixed", 1),
            false,
        ),
        (
            "fixed response, highest priority, deleted by arn",
            fixed_response_rule("del-fixed-top", 2000),
            true,
        ),
        (
            "forward to one target group, exact path and method, deleted by id",
            serde_json::json!({
                "name": "del-forward",
                "priority": 10,
                "match": {
                    "httpMatch": {
                        "method": "POST",
                        "pathMatch": { "match": { "exact": "/orders" } },
                    },
                },
                "action": {
                    "forward": {
                        "targetGroups": [{ "targetGroupIdentifier": target_group("1") }],
                    },
                },
            }),
            false,
        ),
        (
            "forward to two weighted target groups, header match, deleted by arn",
            serde_json::json!({
                "name": "del-weighted",
                "priority": 20,
                "match": {
                    "httpMatch": {
                        "headerMatches": [
                            { "name": "x-tenant", "match": { "exact": "acme" } },
                        ],
                    },
                },
                "action": {
                    "forward": {
                        "targetGroups": [
                            { "targetGroupIdentifier": target_group("2"), "weight": 30 },
                            { "targetGroupIdentifier": target_group("3"), "weight": 70 },
                        ],
                    },
                },
            }),
            true,
        ),
        (
            "longest legal name, mid-range priority, deleted by id",
            fixed_response_rule(
                // `RuleName`'s `@length(3, 63)` upper bound, built here rather than
                // hand-typed so the row cannot drift from it.
                &format!("del-{}", "a".repeat(59)),
                1000,
            ),
            false,
        ),
    ]
}

/// A delete removes the rule, every later read of it is the modelled not-found, and the
/// name and priority it held are free for a new rule of the same listener.
#[tokio::test]
async fn deleting_a_rule_removes_it_and_every_later_read_reports_the_not_found() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "rule-delete-host").await;
    let listener_id = a_listener(&lattice, &service_id, "rule-delete-listener").await;

    for (case, body, delete_by_arn) in rule_rows() {
        let created = create_rule(&lattice, &service_id, &listener_id, body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: creating the rule expected 201, got {} / {}",
            created.status, created.body
        );
        let rule_id = created.str_field("id");
        let rule_arn = created.str_field("arn");

        // Readable BEFORE the delete, so the not-found after it can only come from the
        // delete having removed the record.
        let before = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the rule must be readable before the delete, got {} / {}",
            before.status, before.body
        );

        let named = if delete_by_arn { &rule_arn } else { &rule_id };
        let deleted = delete_rule(&lattice, &service_id, &listener_id, &as_label(named)).await;
        assert_eq!(
            deleted.status, 204,
            "case {case:?}: DeleteRule expected the modelled 204, got {} / {}",
            deleted.status, deleted.body
        );
        assert!(
            deleted.body.is_null()
                || deleted
                    .body
                    .as_object()
                    .is_some_and(serde_json::Map::is_empty),
            "case {case:?}: `DeleteRuleResponse` models no member, got {}",
            deleted.body
        );

        // Both modelled identifier forms, through the Get and through the delete itself:
        // the record is gone for every reader, not only for the one the delete named.
        for (form, identifier) in [("by id", &rule_id), ("by arn", &rule_arn)] {
            let read = get_rule(&lattice, &service_id, &listener_id, &as_label(identifier)).await;
            assert_gone(
                &read,
                &format!("{case}: GetRule {form}"),
                LABEL_RULE,
                TYPE_RULE,
                identifier,
            );
            let again =
                delete_rule(&lattice, &service_id, &listener_id, &as_label(identifier)).await;
            assert_gone(
                &again,
                &format!("{case}: DeleteRule again {form}"),
                LABEL_RULE,
                TYPE_RULE,
                identifier,
            );
        }

        // The name and the priority the rule held are free again: the very same create
        // succeeds, and mints a DIFFERENT rule.
        let remade = create_rule(&lattice, &service_id, &listener_id, body).await;
        assert_eq!(
            remade.status, 201,
            "case {case:?}: the deleted rule's name and priority must be free, got {} / {}",
            remade.status, remade.body
        );
        let remade_id = remade.str_field("id");
        assert_ne!(
            remade_id, rule_id,
            "case {case:?}: the re-create must mint a new rule, not resurrect {rule_id}"
        );
        // Leave the listener holding only its default rule, so the next row's name and
        // priority are the only ones in play.
        let cleanup = delete_rule(&lattice, &service_id, &listener_id, &remade_id).await;
        assert_eq!(
            cleanup.status, 204,
            "case {case:?}: deleting the re-created rule expected 204, got {} / {}",
            cleanup.status, cleanup.body
        );
    }
}

/// A delete the request cannot have removes NOTHING: the rule it named is still readable
/// afterwards, through the listener that owns it.
#[tokio::test]
async fn a_rule_delete_that_is_refused_removes_nothing() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "refused-delete-host").await;
    let (other_service, _) = a_service(&lattice, "refused-delete-other").await;
    let listener_id = a_listener(&lattice, &service_id, "refused-delete-listener").await;
    let other_listener = a_listener(&lattice, &service_id, "refused-delete-sibling").await;

    let created = create_rule(
        &lattice,
        &service_id,
        &listener_id,
        fixed_response_rule("kept", 7),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case fixture rule: expected 201, got {} / {}",
        created.status, created.body
    );
    let rule_id = created.str_field("id");

    // "You can delete additional listener rules, but you cannot delete the default rule"
    // (`DeleteRule`'s own documentation): the refusal names the member that named the
    // rule and the rule stays where it is.
    let default_id = default_rule_id(&lattice, &service_id, &listener_id).await;
    let refused = delete_rule(&lattice, &service_id, &listener_id, &default_id).await;
    assert_refused(&refused, "the listener's default rule");
    let message = message_of("the listener's default rule", &refused);
    assert_eq!(
        refused.body["fieldList"][0]["name"].as_str(),
        Some("ruleIdentifier"),
        "case \"the listener's default rule\": the refusal must name the member that named \
         the rule, got {}",
        refused.body
    );
    assert_eq!(
        refused.body["fieldList"][0]["message"].as_str(),
        Some(message.as_str()),
        "case \"the listener's default rule\": the field entry carries the rule's own \
         sentence, got {}",
        refused.body
    );
    let still_there = get_rule(&lattice, &service_id, &listener_id, &default_id).await;
    assert_eq!(
        still_there.status, 200,
        "case \"the listener's default rule\": it must still be readable, got {} / {}",
        still_there.status, still_there.body
    );

    // A rule is reachable — and so deletable — only through the listener and service that
    // hold it; every other path is the not-found, and the rule survives it.
    let wrong_paths: Vec<(&str, String, String, String)> = vec![
        (
            "a real rule through a sibling listener",
            service_id.clone(),
            other_listener.clone(),
            rule_id.clone(),
        ),
        (
            "a real rule through the wrong service",
            other_service.clone(),
            listener_id.clone(),
            rule_id.clone(),
        ),
        (
            "a rule id nothing ever created",
            service_id.clone(),
            listener_id.clone(),
            absent_id(PREFIX_RULE, "0"),
        ),
        (
            "another rule id nothing ever created",
            service_id.clone(),
            listener_id.clone(),
            absent_id(PREFIX_RULE, "a1b2c3d4"),
        ),
    ];
    for (case, service_identifier, listener_identifier, identifier) in wrong_paths {
        let answer = delete_rule(
            &lattice,
            &service_identifier,
            &listener_identifier,
            &as_label(&identifier),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status, answer.body
        );
        let read = get_rule(&lattice, &service_id, &listener_id, &rule_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the rule must survive a refused delete, got {} / {}",
            read.status, read.body
        );
    }
}

/// A target group a delete must remove, as a row of the table: what it is, and the create
/// body, plus whether the delete names it by its ARN rather than its bare id.
type TargetGroupRow = (&'static str, serde_json::Value, bool);

/// The target-group table: the type, which config members are sent and the tags all
/// vary, and the delete names the group by each of the two modelled identifier forms. The
/// recorded case — a `LAMBDA` group created and then polled for its removal — is one row.
fn target_group_rows() -> Vec<TargetGroupRow> {
    vec![
        (
            "LAMBDA with no config, deleted by id",
            serde_json::json!({ "name": "del-lambda", "type": "LAMBDA" }),
            false,
        ),
        (
            "LAMBDA with the non-default event structure version and tags, deleted by arn",
            serde_json::json!({
                "name": "del-lambda-v2",
                "type": "LAMBDA",
                "config": { "lambdaEventStructureVersion": "V2" },
                "tags": { "team": "payments" },
            }),
            true,
        ),
        (
            "IP over HTTP with the documented default port, deleted by id",
            serde_json::json!({
                "name": "del-ip",
                "type": "IP",
                "config": { "protocol": "HTTP", "vpcIdentifier": A_VPC },
            }),
            false,
        ),
        (
            "INSTANCE over HTTPS on an explicit port, deleted by arn",
            serde_json::json!({
                "name": "del-instance",
                "type": "INSTANCE",
                "config": { "protocol": "HTTPS", "port": 8443, "vpcIdentifier": A_VPC },
            }),
            true,
        ),
        (
            "ALB with a health check left out, deleted by id",
            serde_json::json!({
                "name": "del-alb",
                "type": "ALB",
                "config": { "protocol": "HTTP", "port": 80, "vpcIdentifier": A_VPC },
            }),
            false,
        ),
    ]
}

/// A delete answers the non-terminal status, the record disappears once the span the
/// clock owes its removal has passed, every later read is the modelled not-found, and the
/// account-unique name it held is free for a new target group.
#[tokio::test]
async fn deleting_a_target_group_removes_it_and_frees_its_account_unique_name() {
    let lattice = Lattice::new();

    for (case, body, delete_by_arn) in target_group_rows() {
        let created = create_target_group(&lattice, body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: creating the target group expected 201, got {} / {}",
            created.status, created.body
        );
        let id = created.str_field("id");
        let arn = created.str_field("arn");
        assert_eq!(
            arn,
            expected_arn(ARN_KIND_TARGET_GROUP, &id),
            "case {case:?}: the ARN must name the minted id"
        );
        // "You can't delete a target group … if the target group creation is in progress"
        // (`DeleteTargetGroup`'s own documentation), so the row waits for the status the
        // clock owes the create rather than racing it.
        wait_until_active(&lattice, case, &id).await;

        let named = if delete_by_arn { &arn } else { &id };
        let deleted = delete_target_group(&lattice, &as_label(named)).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteTargetGroup expected the modelled 200, got {} / {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body["id"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the answer must report the target group it is deleting, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.body["arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the answer must report the deleted group's ARN, got {}",
            deleted.body
        );
        assert_eq!(
            deleted.body["status"].as_str(),
            Some(STATUS_DELETING),
            "case {case:?}: the delete answers the non-terminal {STATUS_DELETING}, got {}",
            deleted.body
        );

        // "If you retry it while the status is DELETE_IN_PROGRESS, the status doesn't
        // change" (the `status` member's own documentation). Whether the removal has
        // already fallen due by now is the clock's business, so BOTH answers the rule
        // allows are accepted here — and a second live status is not one of them.
        let retried = delete_target_group(&lattice, &as_label(named)).await;
        if retried.status == 200 {
            assert_eq!(
                retried.body["status"].as_str(),
                Some(STATUS_DELETING),
                "case {case:?}: a retried delete must not change the status, got {}",
                retried.body
            );
        } else {
            assert_gone(
                &retried,
                &format!("{case}: DeleteTargetGroup retried after the removal fell due"),
                LABEL_TARGET_GROUP,
                TYPE_TARGET_GROUP,
                named,
            );
        }

        // The removal the clock owes: waited for by looking again, never by assuming how
        // long this host took.
        let gone = wait_until_absent(&lattice, case, &id).await;
        assert_gone(
            &gone,
            &format!("{case}: GetTargetGroup by id"),
            LABEL_TARGET_GROUP,
            TYPE_TARGET_GROUP,
            &id,
        );
        let gone_by_arn = get_target_group(&lattice, &as_label(&arn)).await;
        assert_gone(
            &gone_by_arn,
            &format!("{case}: GetTargetGroup by arn"),
            LABEL_TARGET_GROUP,
            TYPE_TARGET_GROUP,
            &arn,
        );
        let deleted_again = delete_target_group(&lattice, &as_label(&id)).await;
        assert_gone(
            &deleted_again,
            &format!("{case}: DeleteTargetGroup after the removal"),
            LABEL_TARGET_GROUP,
            TYPE_TARGET_GROUP,
            &id,
        );

        // "The name must be unique within the account" (`CreateTargetGroup`'s `name`): the
        // deleted group's name is free, and the create that takes it mints a new group.
        let remade = create_target_group(&lattice, body).await;
        assert_eq!(
            remade.status, 201,
            "case {case:?}: the deleted group's name must be free again, got {} / {}",
            remade.status, remade.body
        );
        assert_ne!(
            remade.str_field("id"),
            id,
            "case {case:?}: the re-create must mint a new target group, not resurrect {id}"
        );
    }
}

/// "You can't delete a target group if it is used in a listener rule"
/// (`DeleteTargetGroup`'s own documentation): the refusal is the modelled 409 and the
/// group stays live; once nothing forwards to it any more, the delete goes through.
#[tokio::test]
async fn a_target_group_a_listener_rule_forwards_to_cannot_be_deleted() {
    let lattice = Lattice::new();
    let (service_id, _) = a_service(&lattice, "tg-user-service").await;

    // A listener's `defaultAction` and a rule's `action` are the two places a forward can
    // name a target group, and a group nothing names is the control row.
    let used_by_default = create_target_group(
        &lattice,
        serde_json::json!({ "name": "used-by-default", "type": "LAMBDA" }),
    )
    .await;
    assert_eq!(
        used_by_default.status, 201,
        "case fixture: creating the target group expected 201, got {} / {}",
        used_by_default.status, used_by_default.body
    );
    let default_user = used_by_default.str_field("id");

    let used_by_rule = create_target_group(
        &lattice,
        serde_json::json!({ "name": "used-by-rule", "type": "LAMBDA" }),
    )
    .await;
    assert_eq!(
        used_by_rule.status, 201,
        "case fixture: creating the target group expected 201, got {} / {}",
        used_by_rule.status, used_by_rule.body
    );
    let rule_user = used_by_rule.str_field("id");

    let unused = create_target_group(
        &lattice,
        serde_json::json!({ "name": "used-by-nothing", "type": "LAMBDA" }),
    )
    .await;
    assert_eq!(
        unused.status, 201,
        "case fixture: creating the target group expected 201, got {} / {}",
        unused.status, unused.body
    );
    let unused_id = unused.str_field("id");

    // The listener whose DEFAULT action forwards to the first group.
    let forwarding_listener = create_listener(
        &lattice,
        &service_id,
        serde_json::json!({
            "name": "forwarding-listener",
            "protocol": "HTTP",
            "port": 8080,
            "defaultAction": {
                "forward": { "targetGroups": [{ "targetGroupIdentifier": default_user }] },
            },
        }),
    )
    .await;
    assert_eq!(
        forwarding_listener.status, 201,
        "case fixture: creating the forwarding listener expected 201, got {} / {}",
        forwarding_listener.status, forwarding_listener.body
    );
    let forwarding_listener_id = forwarding_listener.str_field("id");

    // The listener whose default action is a fixed response, carrying a RULE that
    // forwards to the second group.
    let rule_listener = a_listener(&lattice, &service_id, "rule-forwarding-listener").await;
    let forwarding_rule = create_rule(
        &lattice,
        &service_id,
        &rule_listener,
        serde_json::json!({
            "name": "forwarding-rule",
            "priority": 5,
            "match": { "httpMatch": { "pathMatch": { "match": { "prefix": "/" } } } },
            "action": {
                "forward": { "targetGroups": [{ "targetGroupIdentifier": rule_user }] },
            },
        }),
    )
    .await;
    assert_eq!(
        forwarding_rule.status, 201,
        "case fixture: creating the forwarding rule expected 201, got {} / {}",
        forwarding_rule.status, forwarding_rule.body
    );
    let forwarding_rule_id = forwarding_rule.str_field("id");

    for (case, identifier) in [
        ("used by a listener's default rule", &default_user),
        ("used by an additional listener rule", &rule_user),
    ] {
        wait_until_active(&lattice, case, identifier).await;
        let refused = delete_target_group(&lattice, identifier).await;
        assert_conflict(&refused, case, identifier, TYPE_TARGET_GROUP);
        let (read, status) = target_group_status(&lattice, identifier).await;
        assert_eq!(
            status.as_deref(),
            Some(STATUS_ACTIVE),
            "case {case:?}: a refused delete must leave the group live, got {} / {}",
            read.status,
            read.body
        );
    }

    // The control row: nothing forwards to this one, so the same call succeeds.
    wait_until_active(&lattice, "used by nothing", &unused_id).await;
    let deleted = delete_target_group(&lattice, &unused_id).await;
    assert_eq!(
        deleted.status, 200,
        "case \"used by nothing\": expected 200, got {} / {}",
        deleted.status, deleted.body
    );

    // Once the use is gone, so is the refusal: deleting the rule frees the group it
    // forwarded to, and deleting the listener frees the one its default rule named.
    let rule_deleted = delete_rule(
        &lattice,
        &service_id,
        &rule_listener,
        &forwarding_rule_id,
    )
    .await;
    assert_eq!(
        rule_deleted.status, 204,
        "case \"used by an additional listener rule\": deleting the rule expected 204, got {} / {}",
        rule_deleted.status, rule_deleted.body
    );
    let listener_deleted = lattice
        .call(
            "DELETE",
            &format!("/services/{service_id}/listeners/{forwarding_listener_id}"),
            None,
        )
        .await;
    assert_eq!(
        listener_deleted.status, 204,
        "case \"used by a listener's default rule\": deleting the listener expected 204, got {} / {}",
        listener_deleted.status, listener_deleted.body
    );

    for (case, identifier) in [
        ("freed by deleting the listener", &default_user),
        ("freed by deleting the rule", &rule_user),
    ] {
        let deleted = delete_target_group(&lattice, identifier).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200, got {} / {}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body["status"].as_str(),
            Some(STATUS_DELETING),
            "case {case:?}: the delete answers {STATUS_DELETING}, got {}",
            deleted.body
        );
        let gone = wait_until_absent(&lattice, case, identifier).await;
        assert_gone(
            &gone,
            case,
            LABEL_TARGET_GROUP,
            TYPE_TARGET_GROUP,
            identifier,
        );
    }
}

/// "You can't delete a target group … if the target group creation is in progress"
/// (`DeleteTargetGroup`'s own documentation).
///
/// Which of the two documented answers is correct depends on whether the span the clock
/// owes the create has passed at the instant the delete arrives, so both are asserted in
/// full — the 409 while the create is in progress, the 200 once it is not — and neither is
/// a stale success.
#[tokio::test]
async fn a_target_group_delete_while_its_creation_is_in_progress_is_refused() {
    let lattice = Lattice::new();
    let created = create_target_group(
        &lattice,
        serde_json::json!({ "name": "deleted-while-creating", "type": "LAMBDA" }),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case fixture: creating the target group expected 201, got {} / {}",
        created.status, created.body
    );
    let id = created.str_field("id");
    assert_eq!(
        created.body["status"].as_str(),
        Some(STATUS_CREATING),
        "case fixture: a create answers the non-terminal {STATUS_CREATING}, got {}",
        created.body
    );

    let case = "deleted while its creation is in progress";
    let answer = delete_target_group(&lattice, &id).await;
    if answer.status == 200 {
        // The creation span had already fallen due, so this is the delete of a live
        // group: it answers the non-terminal delete status.
        assert_eq!(
            answer.body["status"].as_str(),
            Some(STATUS_DELETING),
            "case {case:?}: a delete of a created group answers {STATUS_DELETING}, got {}",
            answer.body
        );
        let gone = wait_until_absent(&lattice, case, &id).await;
        assert_gone(&gone, case, LABEL_TARGET_GROUP, TYPE_TARGET_GROUP, &id);
    } else {
        assert_conflict(&answer, case, &id, TYPE_TARGET_GROUP);
        // The refusal removed nothing: the group goes on to reach its live status.
        wait_until_active(&lattice, case, &id).await;
    }
}
