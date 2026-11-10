// SPDX-License-Identifier: CC0-1.0

//! `UpdateReceiptRule`, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. What is asserted is the RULE the operation follows over a
//! SET of inputs, not one recorded request:
//!
//! * the rule the request carries REPLACES the stored rule of the same name
//!   whole — every member of it, including the ones the request leaves at their
//!   modelled default — and the replacement keeps the position the stored rule
//!   held in the set's ordered rule list, leaving every other rule alone;
//! * a rule name the set does not hold is nothing to update: the call is refused
//!   with the modelled `RuleDoesNotExist` and no rule is added for it;
//! * a rule set name the account does not hold is refused with the modelled
//!   `RuleSetDoesNotExist`, and no set is created for it.
//!
//! Each stored rule is read back through `DescribeReceiptRule` as well as
//! through `DescribeReceiptRuleSet`, so the two readers of one rule agree, and
//! each reader's own two refusals are asserted beside the update's.

use crate::harness::{call, element, elements, field, Field};
use service_com_amazonaws_ses_simpleemailservice::ops::types::ReceiptRule;
use service_com_amazonaws_ses_simpleemailservice::state::{
    ComAmazonawsSesSimpleemailserviceState, ReceiptRuleSet, MAX_RECEIPT_RULE_SET_NAME,
};
use service_com_amazonaws_ses_simpleemailservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::SystemTime;

/// A router holding only this service, beside the engine it was built over — the
/// same construction `crate::harness::emulator` performs, handing back the
/// engine as well so a test can seed a rule set holding RULES directly, keeping
/// each case about the update under test rather than about `CreateReceiptRule`.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Store a receipt rule set holding `rules` directly in the service's state.
fn seed_rule_set(engine: &engine::InMemoryEngine, name: &str, rules: Vec<ReceiptRule>) {
    engine
        .state::<ComAmazonawsSesSimpleemailserviceState>()
        .lock()
        .receipt_rule_sets
        .insert(
            name.to_string(),
            ReceiptRuleSet {
                name: name.to_string(),
                created_at: SystemTime::now(),
                rules,
            },
        );
}

/// What one `Rule` member of an `UpdateReceiptRule` request carries, and so what
/// the stored rule must look like afterwards.
#[derive(Debug, Clone)]
struct RuleSpec {
    /// `ReceiptRule$Name` — the rule this request updates.
    name: String,
    /// `ReceiptRule$Enabled`, whose modelled default is `false`.
    enabled: bool,
    /// `ReceiptRule$ScanEnabled`, whose modelled default is `false`.
    scan_enabled: bool,
    /// `ReceiptRule$TlsPolicy`, one of the modelled enum's values, or `None`
    /// when the request leaves the member out.
    tls_policy: Option<String>,
    /// `ReceiptRule$Recipients`, the domains and addresses the rule applies to.
    recipients: Vec<String>,
    /// The bucket of one `S3Action` in `ReceiptRule$Actions`, or `None` when the
    /// request carries no action at all.
    s3_bucket: Option<String>,
}

impl RuleSpec {
    /// A rule carrying only the two members a request cannot leave out.
    fn bare(name: &str) -> Self {
        Self {
            name: name.to_string(),
            enabled: false,
            scan_enabled: false,
            tls_policy: None,
            recipients: Vec::new(),
            s3_bucket: None,
        }
    }

    /// The `Rule.*` form fields of an awsQuery request carrying this rule.
    fn fields(&self) -> Vec<Field> {
        let mut params = vec![
            field("Rule.Name", self.name.clone()),
            field("Rule.Enabled", self.enabled.to_string()),
            field("Rule.ScanEnabled", self.scan_enabled.to_string()),
        ];
        if let Some(policy) = &self.tls_policy {
            params.push(field("Rule.TlsPolicy", policy.clone()));
        }
        for (index, recipient) in self.recipients.iter().enumerate() {
            params.push(field(
                format!("Rule.Recipients.member.{}", index + 1),
                recipient.clone(),
            ));
        }
        if let Some(bucket) = &self.s3_bucket {
            params.push(field(
                "Rule.Actions.member.1.S3Action.BucketName",
                bucket.clone(),
            ));
        }
        params
    }

    /// The same rule as the stored shape, for seeding a set with it.
    ///
    /// Seeded the way a rule the account HOLDS really looks: the members whose default the
    /// service supplies where a rule is stored
    /// ([`materialize_receipt_rule_defaults`]) are filled in here too, so a seeded neighbour
    /// is not a rule no operation could have produced.
    ///
    /// [`materialize_receipt_rule_defaults`]: service_com_amazonaws_ses_simpleemailservice::state::materialize_receipt_rule_defaults
    fn stored(&self) -> ReceiptRule {
        let mut rule = ReceiptRule {
            name: self.name.clone(),
            enabled: self.enabled,
            scan_enabled: self.scan_enabled,
            tls_policy: self.tls_policy.clone(),
            recipients: if self.recipients.is_empty() {
                None
            } else {
                Some(self.recipients.clone())
            },
            actions: None,
        };
        service_com_amazonaws_ses_simpleemailservice::state::materialize_receipt_rule_defaults(
            &mut rule,
        );
        rule
    }
}

/// `UpdateReceiptRule`.
async fn update_rule(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    rule: &RuleSpec,
) -> (u16, String) {
    let mut params = vec![field("RuleSetName", rule_set_name)];
    params.extend(rule.fields());
    call(router, "UpdateReceiptRule", &params).await
}

/// `DescribeReceiptRuleSet`.
async fn describe_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "DescribeReceiptRuleSet",
        &[field("RuleSetName", name)],
    )
    .await
}

/// `DescribeReceiptRule`.
async fn describe_rule(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    rule_name: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeReceiptRule",
        &[
            field("RuleSetName", rule_set_name),
            field("RuleName", rule_name),
        ],
    )
    .await
}

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (element(body, "Code"), element(body, "Message"))
}

/// The TOP-LEVEL `<member>…</member>` elements of a list's inner XML, in
/// document order.
///
/// [`crate::harness::elements`] pairs an opening tag with the NEXT closing one,
/// which is not what a receipt rule is: a rule carries `<Recipients>` and
/// `<Actions>` lists with `<member>` elements of their own, so the rules are
/// split with the nesting counted.
fn list_members(xml: &str) -> Vec<String> {
    let open = "<member";
    let close = "</member>";
    let mut members = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut index = 0usize;
    while index < xml.len() {
        let rest = &xml[index..];
        if rest.starts_with(close) {
            index += close.len();
            depth = depth.saturating_sub(1);
            if depth == 0 {
                members.push(xml[start..index - close.len()].to_string());
            }
            continue;
        }
        // `<member>`, or `<member xmlns="…">` as the codec writes the first
        // element of a nested struct.
        if rest.starts_with(open) {
            let Some(tag_end) = rest.find('>') else { break };
            index += tag_end + 1;
            depth += 1;
            if depth == 1 {
                start = index;
            }
            continue;
        }
        index += 1;
    }
    members
}

/// Every rule of the rule set a `DescribeReceiptRuleSet` body reports, in the
/// order the set holds them.
fn reported_rules(body: &str) -> Vec<String> {
    list_members(&element(body, "Rules").unwrap_or_default())
}

/// Assert that `rule`, one rule of a `DescribeReceiptRuleSet` body, is exactly
/// what `spec` asked for — including the members the request left out.
fn assert_reports(case: &str, rule: &str, spec: &RuleSpec) {
    assert_eq!(
        element(rule, "Name").as_deref(),
        Some(spec.name.as_str()),
        "case {case}: expected the rule named {:?}, got {rule}",
        spec.name
    );
    assert_eq!(
        element(rule, "Enabled").as_deref(),
        Some(spec.enabled.to_string().as_str()),
        "case {case}: expected Enabled={}, got {rule}",
        spec.enabled
    );
    assert_eq!(
        element(rule, "ScanEnabled").as_deref(),
        Some(spec.scan_enabled.to_string().as_str()),
        "case {case}: expected ScanEnabled={}, got {rule}",
        spec.scan_enabled
    );
    // `ReceiptRule$TlsPolicy` is documented "The default is Optional", so a rule
    // stored without one reads back carrying that default rather than nothing.
    let tls_policy = spec
        .tls_policy
        .clone()
        .or_else(|| Some("Optional".to_string()));
    assert_eq!(
        element(rule, "TlsPolicy"),
        tls_policy,
        "case {case}: expected TlsPolicy={tls_policy:?}, got {rule}"
    );
    let recipients = element(rule, "Recipients")
        .map(|list| elements(&list, "member"))
        .unwrap_or_default();
    assert_eq!(
        recipients, spec.recipients,
        "case {case}: expected the recipients {:?}, got {rule}",
        spec.recipients
    );
    let actions = element(rule, "Actions").unwrap_or_default();
    assert_eq!(
        element(&actions, "BucketName"),
        spec.s3_bucket,
        "case {case}: expected the S3 action bucket {:?}, got {rule}",
        spec.s3_bucket
    );
}

/// The rule the request carries replaces the stored rule of the same name whole,
/// in the position that rule already held, and no other rule of the set moves or
/// changes.
#[tokio::test]
async fn an_update_replaces_the_stored_rule_whole_and_in_place() {
    let (router, engine) = emulator_with_engine();
    // Three rules, so the updated one has a neighbour on each side: the rule in
    // the MIDDLE is the one every row below updates.
    let first = RuleSpec {
        recipients: vec!["first@example.com".to_string()],
        ..RuleSpec::bare("first-rule")
    };
    let target = RuleSpec {
        enabled: true,
        tls_policy: Some("Optional".to_string()),
        recipients: vec!["inbound@example.com".to_string()],
        ..RuleSpec::bare("target.rule-1_A")
    };
    let last = RuleSpec {
        scan_enabled: true,
        ..RuleSpec::bare("last-rule")
    };
    seed_rule_set(
        &engine,
        "inbound-rules",
        vec![first.stored(), target.stored(), last.stored()],
    );

    // The rows differ along what the replacement generalises over: each boolean
    // member, both values the `TlsPolicy` enum documents (`Require` and
    // `Optional`), how many recipients the rule applies to (none, one, several),
    // and whether it carries an action at all. The last row is the BARE rule —
    // every optional member left out — which is what proves the replacement is
    // whole rather than a merge with what the previous row stored.
    let rows = [
        RuleSpec {
            enabled: true,
            scan_enabled: true,
            tls_policy: Some("Require".to_string()),
            recipients: vec![
                "sales@example.com".to_string(),
                "example.net".to_string(),
                "support@example.org".to_string(),
            ],
            s3_bucket: Some("inbound-mail-bucket".to_string()),
            ..RuleSpec::bare(&target.name)
        },
        RuleSpec {
            enabled: false,
            scan_enabled: true,
            tls_policy: Some("Optional".to_string()),
            recipients: vec!["single@example.com".to_string()],
            ..RuleSpec::bare(&target.name)
        },
        RuleSpec {
            enabled: true,
            tls_policy: Some("Require".to_string()),
            ..RuleSpec::bare(&target.name)
        },
        RuleSpec::bare(&target.name),
    ];


    for (index, row) in rows.iter().enumerate() {
        let case = format!("row {index} {row:?}");
        let (status, body) = update_rule(&router, "inbound-rules", row).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from UpdateReceiptRule, got {status} with {body}"
        );

        let (status, body) = describe_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );

        // The set still holds three rules, in the order it held them.
        let names: Vec<Option<String>> = reported_rules(&body)
            .iter()
            .map(|rule| element(rule, "Name"))
            .collect();
        assert_eq!(
            names,
            vec![
                Some(first.name.clone()),
                Some(target.name.clone()),
                Some(last.name.clone()),
            ],
            "case {case}: an update must leave the rule list and its order alone, got {body}"
        );

        let rules = reported_rules(&body);
        assert_reports(&case, &rules[1], row);
        // The neighbours are still exactly as they were stored.
        assert_reports(&format!("{case} (neighbour before)"), &rules[0], &first);
        assert_reports(&format!("{case} (neighbour after)"), &rules[2], &last);

        // And the single-rule reader reports the same stored rule.
        let (status, body) = describe_rule(&router, "inbound-rules", &row.name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRule, got {status} with {body}"
        );
        assert_reports(
            &format!("{case} (DescribeReceiptRule)"),
            &element(&body, "Rule").unwrap_or_default(),
            row,
        );
    }
}

/// A rule name the set does not hold is nothing to update: the call is refused
/// with the modelled `RuleDoesNotExist` naming it, and no rule is added.
#[tokio::test]
async fn updating_a_rule_the_set_does_not_hold_is_refused_and_adds_nothing() {
    let (router, engine) = emulator_with_engine();
    let held = RuleSpec {
        enabled: true,
        recipients: vec!["held@example.com".to_string()],
        ..RuleSpec::bare("held-rule")
    };
    seed_rule_set(&engine, "present-set", vec![held.stored()]);
    // A second set holding a rule of its own, so a refusal below is about the
    // rule not being in the set that was NAMED, not about the account holding no
    // such rule anywhere.
    seed_rule_set(
        &engine,
        "other-set",
        vec![RuleSpec::bare("only-in-other-set").stored()],
    );

    // The rows differ in the form and size of the absent name: a plain name, the
    // held name in another case, the held name with a character added, a name
    // only a receipt RULE may carry (periods are legal in one, per
    // `ReceiptRule$Name`), a rule held by a DIFFERENT set, the longest name the
    // member documents, and one that is not even a legal rule name — a name SES
    // cannot hold is a name the set does not hold.
    let absent = [
        "absent-rule".to_string(),
        "HELD-RULE".to_string(),
        "held-rule2".to_string(),
        "held.rule".to_string(),
        "only-in-other-set".to_string(),
        "n".repeat(MAX_RECEIPT_RULE_SET_NAME),
        "not a legal name!".to_string(),
    ];
    for name in &absent {
        let case = format!("absent rule {name:?}");
        let (status, body) = update_rule(&router, "present-set", &RuleSpec::bare(name)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("RuleDoesNotExist"),
            "case {case}: expected the modelled RuleDoesNotExist, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule does not exist: {name}").as_str()),
            "case {case}: expected the absent rule named in the message, got {body}"
        );

        // The refused update added nothing and changed nothing.
        let (status, body) = describe_rule_set(&router, "present-set").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        let rules = reported_rules(&body);
        assert_eq!(
            rules.len(),
            1,
            "case {case}: the refused update must add no rule, got {body}"
        );
        assert_reports(&case, &rules[0], &held);

        // The single-rule reader refuses the same absent name the same way.
        let (status, body) = describe_rule(&router, "present-set", name).await;
        assert_eq!(
            status, 400,
            "case {case} (DescribeReceiptRule): expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("RuleDoesNotExist"),
            "case {case} (DescribeReceiptRule): expected the modelled RuleDoesNotExist, \
             got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule does not exist: {name}").as_str()),
            "case {case} (DescribeReceiptRule): expected the absent rule named in the \
             message, got {body}"
        );
    }

    // And the set that does hold the rule named in one of those rows is untouched
    // too: an update reaches only the set the request names.
    let (status, body) = describe_rule_set(&router, "other-set").await;
    assert_eq!(
        status, 200,
        "other-set after the refusals: expected 200, got {status} with {body}"
    );
    let rules = reported_rules(&body);
    assert_eq!(
        rules.len(),
        1,
        "other-set after the refusals: expected its one rule, got {body}"
    );
    assert_reports(
        "other-set after the refusals",
        &rules[0],
        &RuleSpec::bare("only-in-other-set"),
    );
}

/// A rule set the account does not hold is refused with the modelled
/// `RuleSetDoesNotExist` naming it, and no set is created for it.
#[tokio::test]
async fn updating_a_rule_of_a_set_that_does_not_exist_is_refused() {
    let (router, engine) = emulator_with_engine();
    let held = RuleSpec {
        tls_policy: Some("Require".to_string()),
        ..RuleSpec::bare("held-rule")
    };
    seed_rule_set(&engine, "present-set", vec![held.stored()]);

    // The rows differ in the form and size of the absent set name, including one
    // that differs from the present set only in case and one that is not even a
    // legal rule set name.
    let absent = [
        "absent-set".to_string(),
        "a".to_string(),
        "PRESENT-SET".to_string(),
        "Absent_Rule-Set_2024".to_string(),
        "x".repeat(MAX_RECEIPT_RULE_SET_NAME),
        "not a legal name!".to_string(),
    ];
    for rule_set_name in &absent {
        let case = format!("absent rule set {rule_set_name:?}");
        let (status, body) = update_rule(&router, rule_set_name, &held).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case {case}: expected the modelled RuleSetDoesNotExist, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule set does not exist: {rule_set_name}").as_str()),
            "case {case}: expected the absent rule set named in the message, got {body}"
        );

        // The refused update created no set under that name.
        let (status, body) = describe_rule_set(&router, rule_set_name).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused update must create no rule set, got {status} with {body}"
        );
        assert_eq!(
            error(&body).0.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case {case}: {rule_set_name:?} must not exist, got {body}"
        );

        // The single-rule reader refuses the same absent set the same way.
        let (status, body) = describe_rule(&router, rule_set_name, &held.name).await;
        assert_eq!(
            status, 400,
            "case {case} (DescribeReceiptRule): expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case {case} (DescribeReceiptRule): expected the modelled RuleSetDoesNotExist, \
             got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule set does not exist: {rule_set_name}").as_str()),
            "case {case} (DescribeReceiptRule): expected the absent rule set named in the \
             message, got {body}"
        );
    }

    // The one set the account does hold still reports the rule it was seeded
    // with.
    let (status, body) = describe_rule_set(&router, "present-set").await;
    assert_eq!(
        status, 200,
        "present-set after the refusals: expected 200, got {status} with {body}"
    );
    let rules = reported_rules(&body);
    assert_eq!(
        rules.len(),
        1,
        "present-set after the refusals: expected its one rule, got {body}"
    );
    assert_reports("present-set after the refusals", &rules[0], &held);
}
