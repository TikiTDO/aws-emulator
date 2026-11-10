// SPDX-License-Identifier: CC0-1.0

//! The unit tests of [`super`]: what this service's stored state computes on its own,
//! away from the wire.

use super::*;

/// A rule named `name` whose evaluated resources carry `compliance_types`, one
/// per resource.
fn rule_with(name: &str, compliance_types: &[&str]) -> ConfigRuleEntry {
    ConfigRuleEntry {
        name: name.to_string(),
        arn: format!("arn:aws:config:us-east-1:000000000000:config-rule/{name}"),
        id: name.to_string(),
        description: None,
        scope: None,
        source: Source::default(),
        input_parameters: None,
        maximum_execution_frequency: None,
        state: CONFIG_RULE_STATE_ACTIVE.to_string(),
        evaluation_modes: default_evaluation_modes(),
        rule_evaluation_visibility: Some(RULE_EVALUATION_VISIBILITY_EXTERNAL.to_string()),
        created_by: None,
        created_at: SystemTime::UNIX_EPOCH,
        tags: TagSet::new(),
        evaluation_results: compliance_types
            .iter()
            .enumerate()
            .map(|(index, compliance)| {
                let entry = EvaluationResultEntry::new(
                    "AWS::EC2::Volume",
                    &format!("vol-{index}"),
                    EVALUATION_MODE_DETECTIVE,
                    compliance,
                    SystemTime::UNIX_EPOCH,
                );
                (entry.key(), entry)
            })
            .collect(),
        evaluation: RuleEvaluationEntry::default(),
    }
}

/// One recorded result for the account resource a process check evaluates, as
/// `PutExternalEvaluation` records it: the deliverer's `OrderingTimestamp` is both
/// the ordering and the invocation instant, and Config stamps when it recorded it.
fn account_result(compliance_type: &str, annotation: Option<&str>) -> EvaluationResultEntry {
    let delivered_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    EvaluationResultEntry {
        annotation: annotation.map(ToString::to_string),
        result_recorded_time: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_001),
        ..EvaluationResultEntry::new(
            EXTERNAL_EVALUATION_RESOURCE_TYPE,
            ::engine::emulator_account_id(),
            EVALUATION_MODE_DETECTIVE,
            compliance_type,
            delivered_at,
        )
    }
}

/// The notification body states the rule, the resource and the verdict that now
/// holds, and carries the verdict it replaced exactly when there was one.
///
/// The rows differ along what the rule generalises over: the verdict recorded, the
/// verdict it replaced (none, or each of the other values), and whether an
/// annotation came with either result.
#[test]
fn a_compliance_change_notification_carries_the_new_verdict_and_the_one_it_replaced() {
    let rule = rule_with("process-check", &[]);
    let cases: [(&str, &str, Option<&str>, Option<&str>); 5] = [
        ("the first verdict", COMPLIANCE_TYPE_COMPLIANT, None, None),
        (
            "compliant replaced by noncompliant",
            COMPLIANCE_TYPE_NON_COMPLIANT,
            Some(COMPLIANCE_TYPE_COMPLIANT),
            None,
        ),
        (
            "noncompliant replaced by compliant",
            COMPLIANCE_TYPE_COMPLIANT,
            Some(COMPLIANCE_TYPE_NON_COMPLIANT),
            None,
        ),
        (
            "not applicable replacing a verdict",
            COMPLIANCE_TYPE_NOT_APPLICABLE,
            Some(COMPLIANCE_TYPE_NON_COMPLIANT),
            None,
        ),
        (
            "a verdict delivered with an annotation",
            COMPLIANCE_TYPE_NON_COMPLIANT,
            Some(COMPLIANCE_TYPE_COMPLIANT),
            Some("the review is overdue"),
        ),
    ];

    for (case, new_type, old_type, annotation) in cases {
        let new_result = account_result(new_type, annotation);
        let old_result = old_type.map(|old| account_result(old, None));
        let detail = compliance_change_detail(
            &rule.name,
            &rule.arn,
            "eu-west-1",
            &new_result,
            old_result.as_ref(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_002),
        );

        assert_eq!(
            detail["messageType"].as_str(),
            Some(COMPLIANCE_CHANGE_MESSAGE_TYPE),
            "case {case:?}: expected a compliance-change notification, got {detail}"
        );
        assert_eq!(
            detail["newEvaluationResult"]["complianceType"].as_str(),
            Some(new_type),
            "case {case:?}: expected the verdict that now holds, got {detail}"
        );
        assert_eq!(
            detail["newEvaluationResult"]["evaluationResultIdentifier"]
                ["evaluationResultQualifier"]["configRuleName"]
                .as_str(),
            Some(rule.name.as_str()),
            "case {case:?}: expected the rule the verdict is about, got {detail}"
        );
        assert_eq!(
            detail["newEvaluationResult"]["annotation"].as_str(),
            annotation,
            "case {case:?}: the annotation is reported exactly when one was \
                 delivered, got {detail}"
        );
        assert_eq!(
            (
                detail["configRuleARN"].as_str(),
                detail["awsRegion"].as_str(),
                detail["resourceType"].as_str(),
                detail["resourceId"].as_str(),
            ),
            (
                Some(rule.arn.as_str()),
                Some("eu-west-1"),
                Some(EXTERNAL_EVALUATION_RESOURCE_TYPE),
                Some(::engine::emulator_account_id()),
            ),
            "case {case:?}: expected the rule and resource the change is about, \
                 got {detail}"
        );
        assert_eq!(
            detail["oldEvaluationResult"]["complianceType"].as_str(),
            old_type,
            "case {case:?}: the replaced verdict is carried exactly when there was \
                 one, got {detail}"
        );
        assert!(
            detail["notificationCreationTime"]
                .as_str()
                .is_some_and(|at| at.starts_with("2023-")),
            "case {case:?}: expected an RFC 3339 notification time, got {detail}"
        );
    }
}

#[test]
fn a_rule_rolls_up_the_compliance_of_the_resources_it_evaluated() {
    let cases: [(&str, &[&str], &str); 6] = [
        ("nothing evaluated", &[], COMPLIANCE_TYPE_INSUFFICIENT_DATA),
        (
            "every resource compliant",
            &[COMPLIANCE_TYPE_COMPLIANT, COMPLIANCE_TYPE_COMPLIANT],
            COMPLIANCE_TYPE_COMPLIANT,
        ),
        (
            "one resource noncompliant",
            &[COMPLIANCE_TYPE_COMPLIANT, COMPLIANCE_TYPE_NON_COMPLIANT],
            COMPLIANCE_TYPE_NON_COMPLIANT,
        ),
        (
            "every resource noncompliant",
            &[COMPLIANCE_TYPE_NON_COMPLIANT],
            COMPLIANCE_TYPE_NON_COMPLIANT,
        ),
        (
            "only not-applicable results",
            &["NOT_APPLICABLE"],
            COMPLIANCE_TYPE_INSUFFICIENT_DATA,
        ),
        (
            "not-applicable beside a compliant result",
            &["NOT_APPLICABLE", COMPLIANCE_TYPE_COMPLIANT],
            COMPLIANCE_TYPE_COMPLIANT,
        ),
    ];

    for (case, evaluated, expected) in cases {
        let actual = rule_with("rule", evaluated).compliance();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the rule to roll up to {expected}, got {actual}"
        );
    }
}

#[test]
fn the_summary_counts_rules_up_to_the_published_cap() {
    // Rows differ along how many rules land in each count, including each side of
    // the published cap of 25 rules.
    let cap = usize::try_from(COMPLIANCE_SUMMARY_RULE_CAP).expect("the cap is positive");
    let cases: [(&str, usize, usize, usize); 6] = [
        ("no rules at all", 0, 0, 0),
        ("one of each", 1, 1, 0),
        ("one short of the cap", cap - 1, 0, 0),
        ("exactly the cap", cap, cap, 0),
        ("one past the cap", cap + 1, 0, 0),
        ("far past the cap", cap * 3, cap * 3, 0),
    ];

    for (case, compliant, non_compliant, insufficient) in cases {
        let mut rules: Vec<ConfigRuleEntry> = Vec::new();
        for index in 0..compliant {
            rules.push(rule_with(
                &format!("c-{index}"),
                &[COMPLIANCE_TYPE_COMPLIANT],
            ));
        }
        for index in 0..non_compliant {
            rules.push(rule_with(
                &format!("n-{index}"),
                &[COMPLIANCE_TYPE_NON_COMPLIANT],
            ));
        }
        for index in 0..insufficient {
            rules.push(rule_with(&format!("i-{index}"), &[]));
        }

        let summary = summarise_rule_compliance(rules.iter(), SystemTime::UNIX_EPOCH);
        let expect = |count: usize| ComplianceContributorCount {
            capped_count: i32::try_from(count.min(cap)).expect("a capped count fits"),
            cap_exceeded: count > cap,
        };
        for (member, actual, count) in [
            (
                "CompliantResourceCount",
                summary.compliant_resource_count.clone(),
                compliant,
            ),
            (
                "NonCompliantResourceCount",
                summary.non_compliant_resource_count.clone(),
                non_compliant,
            ),
        ] {
            let actual = actual
                .unwrap_or_else(|| panic!("case {case:?}: expected a {member}"));
            let expected = expect(count);
            assert_eq!(
                (actual.capped_count, actual.cap_exceeded),
                (expected.capped_count, expected.cap_exceeded),
                "case {case:?}: expected {member} {{CappedCount: {}, CapExceeded: {}}} for \
                     {count} rules, got {{CappedCount: {}, CapExceeded: {}}}",
                expected.capped_count,
                expected.cap_exceeded,
                actual.capped_count,
                actual.cap_exceeded
            );
        }
        assert!(
            summary.compliance_summary_timestamp.is_some(),
            "case {case:?}: expected a ComplianceSummaryTimestamp"
        );
    }
}

#[test]
fn an_evaluation_result_page_token_round_trips_every_key_it_is_given() {
    // Rows differ along what the key's fields hold: ordinary values, the characters
    // a separator-joined token would split on, empty and multi-byte fields, and a
    // resource id at the `BaseResourceId` length maximum of 768.
    let cases: [(&str, EvaluationResultKey); 6] = [
        (
            "an ordinary key",
            (
                "AWS::EC2::Volume".to_string(),
                "vol-0123456789abcdef0".to_string(),
                EVALUATION_MODE_DETECTIVE.to_string(),
            ),
        ),
        (
            "a resource id carrying the separators a joined token would use",
            (
                "AWS::S3::Bucket".to_string(),
                "bucket#with:odd/characters|inside".to_string(),
                EVALUATION_MODE_DETECTIVE.to_string(),
            ),
        ),
        (
            "a resource id that is itself a length prefix",
            (
                "AWS::S3::Bucket".to_string(),
                "3:abc".to_string(),
                "PROACTIVE".to_string(),
            ),
        ),
        (
            "empty fields",
            (String::new(), String::new(), String::new()),
        ),
        (
            "multi-byte characters",
            (
                "AWS::S3::Bucket".to_string(),
                "caf\u{e9}-b\u{fc}cket-\u{1f512}".to_string(),
                "PROACTIVE".to_string(),
            ),
        ),
        (
            "a resource id at the published BaseResourceId maximum of 768",
            (
                "AWS::EC2::Instance".to_string(),
                "i".repeat(768),
                EVALUATION_MODE_DETECTIVE.to_string(),
            ),
        ),
    ];

    for (case, key) in cases {
        let token = encode_evaluation_result_token(&key);
        let decoded = decode_evaluation_result_token(&token);
        assert_eq!(
            decoded.as_ref(),
            Some(&key),
            "case {case:?}: expected the token {token:?} to decode back to {key:?}, \
                 got {decoded:?}"
        );
    }

    // A token this emulator did not issue decodes to nothing, whatever it is.
    for token in [
        "",
        "not-base64-at-all!!",
        &::aws_smithy_types::base64::encode("AWS::EC2::Volume/vol-0"),
        &::aws_smithy_types::base64::encode("3:abc"),
        &::aws_smithy_types::base64::encode("3:abc3:def3:ghi-trailing"),
        &::aws_smithy_types::base64::encode("99:abc0:0:"),
    ] {
        let decoded = decode_evaluation_result_token(token);
        assert!(
            decoded.is_none(),
            "case {token:?}: a token this operation did not issue is malformed, got \
                 {decoded:?}"
        );
    }
}

#[test]
fn an_evaluation_pass_completes_only_once_the_clock_has_made_it_due() {
    // Rows differ along how far past the invocation the clock has moved: before
    // the pass is due, exactly when it is due, and well past it.
    let invoked_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let cases: [(&str, Duration, bool); 4] = [
        ("at the invocation itself", Duration::ZERO, false),
        (
            "one millisecond before the pass is due",
            RULE_EVALUATION_SETTLE_AFTER - Duration::from_millis(1),
            false,
        ),
        ("exactly when it is due", RULE_EVALUATION_SETTLE_AFTER, true),
        ("a minute past it", Duration::from_secs(60), true),
    ];

    assert!(
        RULE_EVALUATION_SETTLE_AFTER >= Duration::from_millis(250)
            && RULE_EVALUATION_SETTLE_AFTER <= Duration::from_millis(750),
        "the span the clock owes an evaluation must stay inside 250-750 ms, got \
             {RULE_EVALUATION_SETTLE_AFTER:?}"
    );

    for (case, elapsed, completed) in cases {
        let mut evaluation = RuleEvaluationEntry::default();
        evaluation.invoke(invoked_at);
        let now = invoked_at + elapsed;
        assert_eq!(
            evaluation.in_progress(now),
            !completed,
            "case {case:?}: expected in_progress {}, got {}",
            !completed,
            evaluation.in_progress(now)
        );

        evaluation.settle(now);
        assert_eq!(
            evaluation.first_evaluation_started, completed,
            "case {case:?}: expected FirstEvaluationStarted {completed}, got {}",
            evaluation.first_evaluation_started
        );
        let expected_invocation = completed.then_some(invoked_at);
        assert_eq!(
            evaluation.last_successful_invocation_at, expected_invocation,
            "case {case:?}: expected the last successful invocation at \
                 {expected_invocation:?}, got {:?}",
            evaluation.last_successful_invocation_at
        );
        let expected_evaluation = completed.then_some(invoked_at + RULE_EVALUATION_SETTLE_AFTER);
        assert_eq!(
            evaluation.last_successful_evaluation_at, expected_evaluation,
            "case {case:?}: expected the last successful evaluation at \
                 {expected_evaluation:?}, got {:?}",
            evaluation.last_successful_evaluation_at
        );

        // Settling again at the same instant changes nothing: the completion is
        // materialised once.
        let before = (
            evaluation.last_successful_invocation_at,
            evaluation.last_successful_evaluation_at,
        );
        evaluation.settle(now);
        assert_eq!(
            (
                evaluation.last_successful_invocation_at,
                evaluation.last_successful_evaluation_at
            ),
            before,
            "case {case:?}: a second settle must not move the recorded instants"
        );
    }
}

#[test]
fn only_an_on_demand_pass_is_recorded_as_one_and_only_while_it_runs() {
    // Rows differ along WHICH pass is in flight and how far the clock has moved:
    // the pass a write owes, the pass an on-demand call owes, and an on-demand
    // pass a later write replaced. Only a pass a `StartConfigRulesEvaluation`
    // call started counts as on demand, and only until it completes.
    let invoked_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    type Invoke = fn(&mut RuleEvaluationEntry, SystemTime);
    let cases: [(&str, &[Invoke], Duration, bool); 6] = [
        (
            "a write's pass, in flight",
            &[RuleEvaluationEntry::invoke],
            Duration::ZERO,
            false,
        ),
        (
            "an on-demand pass, in flight",
            &[RuleEvaluationEntry::invoke_on_demand],
            Duration::ZERO,
            true,
        ),
        (
            "an on-demand pass, one millisecond before it is due",
            &[RuleEvaluationEntry::invoke_on_demand],
            RULE_EVALUATION_SETTLE_AFTER - Duration::from_millis(1),
            true,
        ),
        (
            "an on-demand pass, exactly when it is due",
            &[RuleEvaluationEntry::invoke_on_demand],
            RULE_EVALUATION_SETTLE_AFTER,
            false,
        ),
        (
            "a write's pass over an on-demand one",
            &[
                RuleEvaluationEntry::invoke_on_demand,
                RuleEvaluationEntry::invoke,
            ],
            Duration::ZERO,
            false,
        ),
        (
            "an on-demand pass over a write's one",
            &[
                RuleEvaluationEntry::invoke,
                RuleEvaluationEntry::invoke_on_demand,
            ],
            Duration::ZERO,
            true,
        ),
    ];

    for (case, invocations, elapsed, on_demand_in_progress) in cases {
        let mut evaluation = RuleEvaluationEntry::default();
        for invoke in invocations {
            invoke(&mut evaluation, invoked_at);
        }
        let now = invoked_at + elapsed;
        assert_eq!(
            evaluation.on_demand_in_progress(now),
            on_demand_in_progress,
            "case {case:?}: expected on_demand_in_progress {on_demand_in_progress}, got {}",
            evaluation.on_demand_in_progress(now)
        );

        // A completed pass is no longer in flight, whichever kind it was.
        evaluation.settle(invoked_at + RULE_EVALUATION_SETTLE_AFTER);
        assert!(
            !evaluation.on_demand,
            "case {case:?}: a completed pass must leave no pass in flight, got {evaluation:?}"
        );
    }
}

#[test]
fn a_recorder_id_is_distinct_for_every_recorder_and_is_part_of_the_arn() {
    let mut ids: Vec<String> = (0..1_000).map(|_| mint_recorder_id()).collect();
    let minted = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "every configuration recorder must get a distinct id, got {} distinct of {minted}",
        ids.len()
    );
    for id in &ids {
        assert!(
            id.len() == 16
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "a recorder id is sixteen base-36 characters, got {id:?}"
        );
    }

    let id = mint_recorder_id();
    let arn = recorder_arn("us-west-2", "probe", &id);
    assert!(
        arn.contains(&format!(":{RECORDER_ARN_RESOURCE_TYPE}/probe/{id}")),
        "the recorder ARN carries the resource type, the name and the id, got {arn:?}"
    );
}

#[test]
fn a_resource_evaluation_id_is_distinct_for_every_evaluation() {
    let mut ids: Vec<String> = (0..1_000).map(|_| mint_resource_evaluation_id()).collect();
    let minted = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "every resource evaluation must get a distinct id, got {} distinct of {minted}",
        ids.len()
    );
    for id in &ids {
        assert!(
            (1..=128).contains(&id.len())
                && id
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "a ResourceEvaluationId is lowercase hexadecimal inside the modelled 1..128 \
             length, got {id:?}"
        );
    }
}

#[test]
fn a_resource_evaluation_page_token_round_trips_its_key() {
    // Rows differ along the instant the key names (the epoch itself, before it,
    // after it, and a sub-second offset) and along the id beside it.
    let cases: [(&str, i128, &str); 5] = [
        ("the epoch itself", 0, "0000000000000000"),
        ("one nanosecond past the epoch", 1, "abcdefgh12345678"),
        ("before the epoch", -1_500_000_000, "zzzzzzzzzzzzzzzz"),
        ("a whole second", 1_700_000_000_000_000_000, "7f3k9q1m5x8c2v0b"),
        (
            "a sub-second offset",
            1_700_000_000_123_456_789,
            "0123456789abcdef",
        ),
    ];

    for (case, nanos, id) in cases {
        let key = (::std::cmp::Reverse(nanos), id.to_string());
        let token = encode_resource_evaluation_token(&key);
        let decoded = decode_resource_evaluation_token(&token)
            .unwrap_or_else(|| panic!("case {case:?}: {token:?} must decode"));
        assert_eq!(
            decoded, key,
            "case {case:?}: expected the token to round trip, got {decoded:?}"
        );
    }

    // Rows differ along how the token is malformed; none of them is a token this
    // service issued, so none of them decodes.
    let malformed: [(&str, &str); 4] = [
        ("not base64", "**not base64**"),
        ("base64 with no separator", "MTIzNDU2"),
        ("base64 whose instant is not a number", "d2hlbjppZA=="),
        ("base64 with no id", "MTIzNDU2Nzg5MDo="),
    ];
    for (case, token) in malformed {
        let decoded = decode_resource_evaluation_token(token);
        assert!(
            decoded.is_none(),
            "case {case:?}: expected {token:?} to be refused, got {decoded:?}"
        );
    }
}

#[test]
fn the_resource_evaluation_store_stays_bounded_and_drops_the_oldest() {
    // Rows differ along how far the starts go past the bound: one short of it,
    // exactly it, one past it and well past it.
    let cases: [(&str, usize); 4] = [
        ("one short of the bound", RESOURCE_EVALUATIONS_KEPT - 1),
        ("exactly the bound", RESOURCE_EVALUATIONS_KEPT),
        ("one past the bound", RESOURCE_EVALUATIONS_KEPT + 1),
        ("well past the bound", RESOURCE_EVALUATIONS_KEPT + 25),
    ];

    for (case, started) in cases {
        let mut store = ComAmazonawsConfigserviceStarlingdoveserviceState::default();
        let mut ids: Vec<String> = Vec::new();
        for index in 0..started {
            let id = mint_resource_evaluation_id();
            ids.push(id.clone());
            store.record_resource_evaluation(ResourceEvaluationEntry {
                id,
                evaluation_mode: EVALUATION_MODE_PROACTIVE.to_string(),
                // One millisecond apart, so the oldest is the first one started.
                started_at: SystemTime::UNIX_EPOCH + Duration::from_millis(index as u64),
                evaluation_context_identifier: None,
                resource_details: ResourceDetails::default(),
                evaluation_timeout_seconds: DEFAULT_EVALUATION_TIMEOUT_SECONDS,
                client_token: Some(format!("token-{index}")),
            });
        }

        let kept = started.min(RESOURCE_EVALUATIONS_KEPT);
        assert_eq!(
            store.resource_evaluations.len(),
            kept,
            "case {case:?}: expected the store to hold {kept} evaluations after \
                 {started} starts, got {}",
            store.resource_evaluations.len()
        );
        assert_eq!(
            store.resource_evaluation_tokens.len(),
            kept,
            "case {case:?}: expected the token index to hold one entry per kept \
                 evaluation, got {}",
            store.resource_evaluation_tokens.len()
        );

        // What is kept is the MOST RECENT window: the last `kept` evaluations
        // started, each with the token that started it.
        for (index, id) in ids.iter().enumerate() {
            let expected = index >= started - kept;
            assert_eq!(
                store.resource_evaluations.contains_key(id),
                expected,
                "case {case:?}: evaluation {index} ({id:?}) kept should be {expected}"
            );
            assert_eq!(
                store
                    .resource_evaluation_tokens
                    .contains_key(&format!("token-{index}")),
                expected,
                "case {case:?}: the token of evaluation {index} indexed should be \
                     {expected}"
            );
        }
    }
}
