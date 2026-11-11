// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CancelDomainConfigChange`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness, the baseline `CreateDomain`
//! body and the table of option containers come from [`super::domain_lifecycle`];
//! this suite adds no second copy of them.
//!
//! The rule under test: a cancel acts on the configuration change of a domain that
//! is still in flight — it names that change, reports the properties it was setting,
//! leaves the domain with the configuration the change was applied on top of and
//! marks the change cancelled; a dry run computes the same answer and writes
//! nothing; and a domain that is not in one of the two states a change exists in
//! (`Modifying`, `UpdatingServiceSoftware`) has nothing to cancel, which is the
//! modelled validation refusal naming those states rather than an empty success.
//!
//! So the assertions span the inputs the rule generalises over: every option
//! container in the shared change table (a cluster structure, an EBS structure, a
//! policy string, two maps, an enum, a software-update structure), both values of
//! `DryRun`, a second update refused while the first is still in flight, and four
//! domains with nothing to cancel (a settled one, one whose change has already
//! landed, one that is still being created, and a name no domain is stored under).

use serde_json::{json, Value};

use super::domain_lifecycle::{
    change_rows, create_baseline, error_code, string_member, Answer, Emulator, CHANGE_LOG_GROUP,
    PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// `POST /2021-01-01/opensearch/domain/{DomainName}/config/cancel`.
async fn cancel(emulator: &Emulator, name: &str, dry_run: Option<bool>) -> Answer {
    let body = match dry_run {
        None => json!({}),
        Some(dry_run) => json!({ "DryRun": dry_run }),
    };
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain/{name}/config/cancel"),
            Some(body),
        )
        .await
}

/// `GET /2021-01-01/opensearch/domain/{DomainName}/progress`.
async fn progress(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/progress"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomainChangeProgress({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ChangeProgressStatus"].clone()
}

/// One option's stored value, as `DescribeDomainConfig` reports it.
async fn stored_option(emulator: &Emulator, name: &str, option: &str) -> Value {
    let answer = emulator.describe_config(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomainConfig({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainConfig"][option]["Options"].clone()
}

/// Whether the domain reports a configuration change in flight.
async fn processing(emulator: &Emulator, name: &str) -> bool {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case DescribeDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]["Processing"]
        .as_bool()
        .unwrap_or_else(|| panic!("case DescribeDomain({name:?}): expected Processing"))
}

/// The `CancelledChangeProperties` of an accepted cancel, as (PropertyName,
/// CancelledValue, ActiveValue) rows in the order they were reported.
fn cancelled_properties(answer: &Answer, case: &str) -> Vec<(String, Value, Value)> {
    answer.body["CancelledChangeProperties"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected CancelledChangeProperties, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|property| {
            (
                string_member(property, "PropertyName", case),
                property["CancelledValue"].clone(),
                property["ActiveValue"].clone(),
            )
        })
        .collect()
}

/// The `CancelledChangeIds` of an accepted cancel.
fn cancelled_ids(answer: &Answer, case: &str) -> Vec<String> {
    answer.body["CancelledChangeIds"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected CancelledChangeIds, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("case {case}: expected id strings, got {id:?}"))
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn a_cancel_puts_back_the_configuration_the_change_was_applied_on_top_of() {
    let emulator = Emulator::with_log_groups();
    // The log-publishing row of the shared table publishes to a log group, which has to
    // exist before an option enabling it is accepted.
    emulator.created_log_group(CHANGE_LOG_GROUP).await;
    let rows = change_rows();
    // One domain per option container the shared change table spans, all created
    // first so ONE wait settles every one of them: the change a create starts is not
    // one a cancel can go back from, so each domain has to be settled before the
    // update under test.
    for index in 0..rows.len() {
        create_baseline(&emulator, &format!("cancel-{index}")).await;
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    for (index, (option, value, _)) in rows.into_iter().enumerate() {
        let name = format!("cancel-{index}");
        let case = format!("CancelDomainConfigChange({name}, {option})");
        let before = stored_option(&emulator, &name, option).await;

        let updated = emulator
            .update_config(&name, json!({ option: value.clone() }))
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateDomainConfig to answer 200, got {} with {:?}",
            updated.status, updated.body
        );
        let change_id = string_member(&progress(&emulator, &name).await, "ChangeId", &case);

        let answer = cancel(&emulator, &name, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            cancelled_ids(&answer, &case),
            vec![change_id.clone()],
            "case {case}: expected the change in flight to be the one cancelled, got {:?}",
            answer.body
        );
        let properties = cancelled_properties(&answer, &case);
        assert_eq!(
            properties
                .iter()
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<&str>>(),
            vec![option],
            "case {case}: expected the property the change set, got {properties:?}"
        );
        assert_eq!(
            answer.body["DryRun"].as_bool(),
            Some(false),
            "case {case}: the request was not a dry run, got {:?}",
            answer.body
        );
        // Where an option's value IS a string, the two values it is reported with
        // are the pending one and the one the domain is left with.
        if let (Some(cancelled_value), Some(active_value)) =
            (properties[0].1.as_str(), properties[0].2.as_str())
        {
            assert_eq!(
                (cancelled_value, active_value),
                (
                    value.as_str().unwrap_or_default(),
                    before.as_str().unwrap_or_default()
                ),
                "case {case}: expected the pending value and the value left active, got \
                 {properties:?}"
            );
        }

        // The domain is left with the configuration it was changed FROM, and is no
        // longer applying anything.
        assert_eq!(
            stored_option(&emulator, &name, option).await,
            before,
            "case {case}: expected the option to be back to {before:?}"
        );
        assert!(
            !processing(&emulator, &name).await,
            "case {case}: a domain whose only change was cancelled is not Processing"
        );

        // The cancelled change is still addressable and says what became of it.
        let status = progress(&emulator, &name).await;
        assert_eq!(
            (
                status["ChangeId"].as_str(),
                status["ConfigChangeStatus"].as_str(),
                status["Status"].as_str(),
            ),
            (Some(change_id.as_str()), Some("Cancelled"), Some("FAILED")),
            "case {case}: expected the cancelled change to report it, got {status:?}"
        );

        // Nothing lands later: the configuration is still the one cancelling left.
        tokio::time::sleep(SETTLE_WAIT).await;
        assert_eq!(
            stored_option(&emulator, &name, option).await,
            before,
            "case {case}: a cancelled change does not land once its span has passed"
        );
    }
}

#[tokio::test]
async fn a_dry_run_reports_the_same_cancellation_and_changes_nothing() {
    let emulator = Emulator::new();
    let name = "dry-run-cancel";
    create_baseline(&emulator, name).await;
    tokio::time::sleep(SETTLE_WAIT).await;

    let (option, value, _) = change_rows().remove(0);
    let before = stored_option(&emulator, name, option).await;
    let updated = emulator
        .update_config(name, json!({ option: value.clone() }))
        .await;
    assert_eq!(
        updated.status, 200,
        "case UpdateDomainConfig({name}): expected 200, got {} with {:?}",
        updated.status, updated.body
    );
    let change_id = string_member(&progress(&emulator, name).await, "ChangeId", name);

    let dry = cancel(&emulator, name, Some(true)).await;
    assert_eq!(
        dry.status, 200,
        "case a dry run: expected 200, got {} with {:?}",
        dry.status, dry.body
    );
    assert_eq!(
        (cancelled_ids(&dry, "a dry run"), dry.body["DryRun"].as_bool()),
        (vec![change_id.clone()], Some(true)),
        "case a dry run: expected the change that WOULD be cancelled, got {:?}",
        dry.body
    );
    assert_eq!(
        cancelled_properties(&dry, "a dry run")
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect::<Vec<String>>(),
        vec![option.to_string()],
        "case a dry run: expected the property that WOULD be cancelled, got {:?}",
        dry.body
    );

    // Nothing was cancelled: the change is still in flight, and the clock lands it.
    let status = progress(&emulator, name).await;
    assert_eq!(
        (
            status["ChangeId"].as_str(),
            status["ConfigChangeStatus"].as_str()
        ),
        (Some(change_id.as_str()), Some("ApplyingChanges")),
        "case a dry run: the change is untouched, got {status:?}"
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        stored_option(&emulator, name, option).await,
        super::domain_lifecycle::completed(option, &value),
        "case a dry run: the change the dry run did not cancel lands, leaving {value:?} rather \
         than {before:?}"
    );
    assert_eq!(
        progress(&emulator, name).await["ConfigChangeStatus"].as_str(),
        Some("Completed"),
        "case a dry run: the change completes"
    );
}

#[tokio::test]
async fn a_second_change_while_one_is_in_flight_is_refused() {
    let emulator = Emulator::new();
    let name = "chained-cancel";
    create_baseline(&emulator, name).await;
    tokio::time::sleep(SETTLE_WAIT).await;

    // The service applies one change of a domain at a time: the first update starts a
    // change, and a second while that one is still in flight is refused, so the
    // cancel below has exactly ONE change to go back from.
    let rows = change_rows();
    let (first_option, first_value, _) = rows[0].clone();
    let (second_option, second_value, _) = rows[2].clone();
    let before_first = stored_option(&emulator, name, first_option).await;
    let before_second = stored_option(&emulator, name, second_option).await;
    let accepted = emulator
        .update_config(name, json!({ first_option: first_value }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case UpdateDomainConfig({name}, {first_option}): expected 200, got {} with {:?}",
        accepted.status, accepted.body
    );
    let refused = emulator
        .update_config(name, json!({ second_option: second_value }))
        .await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (400, "ValidationException"),
        "case UpdateDomainConfig({name}, {second_option}, while the first is in flight): expected \
         the modelled validation refusal, got {} with {:?}",
        refused.status,
        refused.body
    );

    let answer = cancel(&emulator, name, Some(false)).await;
    assert_eq!(
        answer.status, 200,
        "case a change in flight: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        cancelled_ids(&answer, "a change in flight").len(),
        1,
        "case a change in flight: expected the one change the domain is applying, got {:?}",
        answer.body
    );
    let properties: Vec<String> = cancelled_properties(&answer, "a change in flight")
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    assert_eq!(
        properties,
        vec![first_option.to_string()],
        "case a change in flight: expected the property that change set, got {properties:?}"
    );
    // Both options are back where they started: the cancelled change went back, and
    // the refused one never happened.
    for (option, before) in [
        (first_option, before_first),
        (second_option, before_second),
    ] {
        assert_eq!(
            stored_option(&emulator, name, option).await,
            before,
            "case a change in flight: expected {option} back to {before:?}"
        );
    }
}

#[tokio::test]
async fn a_domain_outside_the_states_a_change_exists_in_is_refused() {
    let emulator = Emulator::new();

    // A domain whose create is still in flight, and one whose update has already
    // landed: neither is applying a change of its options nor deploying service
    // software, so neither is in a state a cancel is allowed in.
    create_baseline(&emulator, "creating-domain").await;
    create_baseline(&emulator, "settled-domain").await;
    let answer = cancel(&emulator, "creating-domain", None).await;
    assert_refused(&answer, "a domain still being created");

    tokio::time::sleep(SETTLE_WAIT).await;
    let (option, value, _) = change_rows().remove(4);
    let updated = emulator
        .update_config("settled-domain", json!({ option: value }))
        .await;
    assert_eq!(
        updated.status, 200,
        "case UpdateDomainConfig(settled-domain): expected 200, got {} with {:?}",
        updated.status, updated.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    for (name, dry_run, case) in [
        ("creating-domain", Some(true), "a landed create, dry run"),
        ("creating-domain", None, "a landed create"),
        ("settled-domain", None, "a landed update"),
        ("settled-domain", Some(true), "a landed update, dry run"),
    ] {
        let answer = cancel(&emulator, name, dry_run).await;
        assert_refused(&answer, case);
    }

    // A name no domain is stored under is a different answer: the domain itself is
    // not there, which is the modelled not-found naming it.
    let answer = cancel(&emulator, "absent-domain", None).await;
    assert_eq!(
        (answer.status, error_code(&answer).as_str()),
        (RECORDED_NOT_FOUND_STATUS, "ResourceNotFoundException"),
        "case an unknown domain: expected the modelled not-found, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some("Domain not found: absent-domain"),
        "case an unknown domain: expected the miss to name the domain asked for, got {:?}",
        answer.body
    );
}

/// Assert `answer` is the refusal a cancel outside the two states a change exists in
/// gets: the modelled `ValidationException` (HTTP 400) naming those states.
fn assert_refused(answer: &Answer, case: &str) {
    assert_eq!(
        (answer.status, error_code(answer).as_str()),
        (400, "ValidationException"),
        "case {case}: expected the modelled validation refusal, got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(
            "CancelDomainConfigChange only when domain state is in Modifying or \
             UpdatingServiceSoftware."
        ),
        "case {case}: expected the refusal to name the states a cancel is allowed in, got {:?}",
        answer.body
    );
}
