// SPDX-License-Identifier: CC0-1.0

//! `CreateReceiptRule`, end to end over the wire.
//!
//! Every call here goes through `MultiServiceRouter::dispatch`, so each one is a
//! real awsQuery request: form-urlencoded body in, XML out, one shared engine
//! behind all of them. Nothing is seeded into the store — each rule set is made
//! with `CreateReceiptRuleSet` and every rule read back through
//! `DescribeReceiptRule` and `DescribeReceiptRuleSet` — so what is asserted is
//! the RULE the operation follows over a SET of inputs rather than one recorded
//! request:
//!
//! * the rule the request carries is STORED WHOLE, every member of it included,
//!   and both readers of a rule report it back unchanged;
//! * the new rule takes the position straight after the rule `After` names, and
//!   the first position when the request names none, leaving the order of the
//!   rules already held alone;
//! * a rule name the set already holds is a naming conflict (`AlreadyExists`), a
//!   name outside the form `ReceiptRule$Name` documents is refused
//!   (`InvalidParameterValue`), an `After` naming no rule of the set
//!   (`RuleDoesNotExist`) and a rule set the account does not hold
//!   (`RuleSetDoesNotExist`) are refused, and each refusal stores NOTHING;
//! * an action whose members name no resource of the service they document is
//!   refused with that member's own modelled error;
//! * a set already holding as many rules as one set may hold refuses another
//!   (`LimitExceeded`).

use crate::harness::{call, element, elements, emulator, field, xml_text, Field};
use service_com_amazonaws_ses_simpleemailservice::state::{
    MAX_RECEIPT_RULES_PER_RULE_SET, MAX_RECEIPT_RULE_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

/// One action of a receipt rule: which action it is, and the members the request
/// writes under it.
#[derive(Debug, Clone)]
struct ActionSpec {
    /// The `ReceiptAction` member the action is set on (`S3Action`,
    /// `SNSAction`, …).
    kind: &'static str,
    /// That action's own members, as `(member name, value)`.
    members: Vec<(&'static str, String)>,
}

/// What one `Rule` member of a `CreateReceiptRule` request carries, and so what
/// the stored rule must look like afterwards.
#[derive(Debug, Clone)]
struct RuleSpec {
    /// `ReceiptRule$Name` — the rule this request creates.
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
    /// The one action in `ReceiptRule$Actions`, or `None` when the request
    /// carries no action at all.
    action: Option<ActionSpec>,
}

impl RuleSpec {
    /// A rule carrying only the members a request cannot leave out.
    fn bare(name: &str) -> Self {
        Self {
            name: name.to_string(),
            enabled: false,
            scan_enabled: false,
            tls_policy: None,
            recipients: Vec::new(),
            action: None,
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
        if let Some(action) = &self.action {
            for (member, value) in &action.members {
                params.push(field(
                    format!("Rule.Actions.member.1.{}.{member}", action.kind),
                    value.clone(),
                ));
            }
        }
        params
    }
}

/// `CreateReceiptRuleSet`.
async fn create_rule_set(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "CreateReceiptRuleSet", &[field("RuleSetName", name)]).await
}

/// `CreateReceiptRule`, placing `rule` after `after` (at the beginning of the
/// rule list when `after` is `None`, which is what leaving the member out
/// means).
async fn create_rule(
    router: &MultiServiceRouter,
    rule_set_name: &str,
    after: Option<&str>,
    rule: &RuleSpec,
) -> (u16, String) {
    let mut params = vec![field("RuleSetName", rule_set_name)];
    if let Some(after) = after {
        params.push(field("After", after));
    }
    params.extend(rule.fields());
    call(router, "CreateReceiptRule", &params).await
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

/// The `<Code>` and `<Message>` of an awsQuery `<ErrorResponse>` envelope, with
/// the message's XML escaping undone.
fn error(body: &str) -> (Option<String>, Option<String>) {
    (
        element(body, "Code"),
        element(body, "Message").map(|message| xml_text(&message)),
    )
}

/// The names of the rules a `DescribeReceiptRuleSet` body reports, in the order
/// the set holds them.
///
/// `<Name>` is the rule's own member: no action or recipient of a rule carries an
/// element of that name, so reading them in document order reads the rule list's
/// order.
fn reported_names(body: &str) -> Vec<String> {
    elements(&element(body, "Rules").unwrap_or_default(), "Name")
}

/// Assert that `rule`, the XML of one reported receipt rule, is exactly what
/// `spec` asked for — including the members the request left out.
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
    // created without one reads back carrying that default rather than nothing.
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
    match &spec.action {
        None => assert_eq!(
            element(rule, "Actions"),
            None,
            "case {case}: expected no action at all, got {rule}"
        ),
        Some(action) => {
            let reported = element(&element(rule, "Actions").unwrap_or_default(), action.kind)
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a reported {}, got {rule}", action.kind)
                });
            for (member, value) in &action.members {
                assert_eq!(
                    element(&reported, member).map(|text| xml_text(&text)).as_deref(),
                    Some(value.as_str()),
                    "case {case}: expected {}.{member}={value:?}, got {reported}",
                    action.kind
                );
            }
        }
    }
}

/// An ARN of `vendor` naming `resource` in the one account this emulator serves.
fn arn(vendor: &str, resource: &str) -> String {
    format!(
        "arn:aws:{vendor}:us-east-1:{}:{resource}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The rule the request carries is stored whole, in a set that held nothing, and
/// both readers of a rule report it back unchanged.
#[tokio::test]
async fn a_created_rule_is_stored_whole_and_reported_back() {
    // The rows differ along what a rule generalises over: each boolean member,
    // both values the `TlsPolicy` enum documents, how many recipients the rule
    // applies to (none, one, several), and which of the actions `ReceiptAction`
    // models the rule takes — the S3 action with its key prefix and KMS key, the
    // SNS action, the header action, the bounce action, the stop action and the
    // Lambda action. The first row is the BARE rule, every optional member left
    // out.
    let rows = [
        RuleSpec::bare("bare-rule"),
        RuleSpec {
            enabled: true,
            scan_enabled: true,
            tls_policy: Some("Require".to_string()),
            recipients: vec![
                "sales@example.com".to_string(),
                "example.net".to_string(),
                "support@example.org".to_string(),
            ],
            action: Some(ActionSpec {
                kind: "S3Action",
                members: vec![
                    ("BucketName", "inbound-mail-bucket".to_string()),
                    ("ObjectKeyPrefix", "inbound/mail/".to_string()),
                    ("KmsKeyArn", arn("kms", "alias/aws/ses")),
                    ("TopicArn", arn("sns", "inbound-notifications")),
                ],
            }),
            ..RuleSpec::bare("s3.rule-1_A")
        },
        RuleSpec {
            enabled: true,
            tls_policy: Some("Optional".to_string()),
            recipients: vec!["single@example.com".to_string()],
            action: Some(ActionSpec {
                kind: "SNSAction",
                members: vec![
                    ("TopicArn", arn("sns", "inbound-copies")),
                    ("Encoding", "Base64".to_string()),
                ],
            }),
            ..RuleSpec::bare("sns-rule")
        },
        RuleSpec {
            scan_enabled: true,
            action: Some(ActionSpec {
                kind: "AddHeaderAction",
                members: vec![
                    ("HeaderName", "X-Inbound-Checked".to_string()),
                    ("HeaderValue", "yes".to_string()),
                ],
            }),
            ..RuleSpec::bare("header-rule")
        },
        RuleSpec {
            enabled: true,
            action: Some(ActionSpec {
                kind: "BounceAction",
                members: vec![
                    ("SmtpReplyCode", "550".to_string()),
                    ("StatusCode", "5.1.1".to_string()),
                    ("Message", "Mailbox does not exist".to_string()),
                    ("Sender", "postmaster@example.com".to_string()),
                    ("TopicArn", arn("sns", "inbound-bounces")),
                ],
            }),
            ..RuleSpec::bare("bounce-rule")
        },
        RuleSpec {
            action: Some(ActionSpec {
                kind: "StopAction",
                members: vec![("Scope", "RuleSet".to_string())],
            }),
            ..RuleSpec::bare("stop-rule")
        },
        RuleSpec {
            enabled: true,
            action: Some(ActionSpec {
                kind: "LambdaAction",
                members: vec![
                    ("FunctionArn", arn("lambda", "function:InboundMail")),
                    ("InvocationType", "RequestResponse".to_string()),
                ],
            }),
            ..RuleSpec::bare("lambda-rule")
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = format!("row {index} {row:?}");
        // A fresh emulator per row, so each row's set holds exactly its own rule.
        let router = emulator();
        let (status, body) = create_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from CreateReceiptRuleSet, got {status} with {body}"
        );

        let (status, body) = create_rule(&router, "inbound-rules", None, row).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from CreateReceiptRule, got {status} with {body}"
        );

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

        // The set reader reports the same one rule, and nothing else.
        let (status, body) = describe_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        assert_eq!(
            reported_names(&body),
            vec![row.name.clone()],
            "case {case}: expected the set to hold exactly the created rule, got {body}"
        );
        assert_reports(
            &format!("{case} (DescribeReceiptRuleSet)"),
            &element(&body, "Rules").unwrap_or_default(),
            row,
        );
    }
}

/// Create the rules named by `names`, in that order, each placed after the one
/// before it — so the set ends up holding them in exactly that order.
async fn create_in_order(router: &MultiServiceRouter, rule_set_name: &str, names: &[&str]) {
    let mut previous: Option<&str> = None;
    for name in names {
        let (status, body) = create_rule(router, rule_set_name, previous, &RuleSpec::bare(name)).await;
        assert_eq!(
            status, 200,
            "seeding {name:?} after {previous:?}: expected 200, got {status} with {body}"
        );
        previous = Some(name);
    }
}

/// The new rule takes the position straight after the rule `After` names, and
/// the first position when the request names none; the rules already held keep
/// their own order.
#[tokio::test]
async fn after_places_the_new_rule_straight_after_the_rule_it_names() {
    let held = ["first-rule", "second.rule", "third_rule"];
    // The rows differ in which position of the held list the new rule lands in:
    // the beginning (no `After` at all), after the first, in the middle, and
    // after the LAST rule, which appends.
    let rows: [(Option<&str>, [&str; 4]); 4] = [
        (
            None,
            ["inserted-rule", "first-rule", "second.rule", "third_rule"],
        ),
        (
            Some("first-rule"),
            ["first-rule", "inserted-rule", "second.rule", "third_rule"],
        ),
        (
            Some("second.rule"),
            ["first-rule", "second.rule", "inserted-rule", "third_rule"],
        ),
        (
            Some("third_rule"),
            ["first-rule", "second.rule", "third_rule", "inserted-rule"],
        ),
    ];

    for (after, expected) in rows {
        let case = format!("After={after:?}");
        let router = emulator();
        let (status, body) = create_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from CreateReceiptRuleSet, got {status} with {body}"
        );
        create_in_order(&router, "inbound-rules", &held).await;

        let inserted = RuleSpec {
            enabled: true,
            recipients: vec!["inbound@example.com".to_string()],
            ..RuleSpec::bare("inserted-rule")
        };
        let (status, body) = create_rule(&router, "inbound-rules", after, &inserted).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from CreateReceiptRule, got {status} with {body}"
        );

        let (status, body) = describe_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        assert_eq!(
            reported_names(&body),
            expected.to_vec(),
            "case {case}: expected the rule order {expected:?}, got {body}"
        );

        // And the inserted rule is the one that was asked for, not a neighbour
        // that moved.
        let (status, body) = describe_rule(&router, "inbound-rules", &inserted.name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRule, got {status} with {body}"
        );
        assert_reports(&case, &element(&body, "Rule").unwrap_or_default(), &inserted);
    }
}

/// A rule name the set already holds is a naming conflict: the call is refused
/// with the modelled `AlreadyExists` naming it, and the stored rule is left
/// exactly as it was.
#[tokio::test]
async fn a_rule_name_the_set_already_holds_is_refused() {
    let router = emulator();
    let (status, body) = create_rule_set(&router, "inbound-rules").await;
    assert_eq!(
        status, 200,
        "creating the set: expected 200, got {status} with {body}"
    );
    // A second set holding a rule of the SAME name, so the refusals below are
    // about the set that was named rather than about the account holding the name
    // anywhere.
    let (status, body) = create_rule_set(&router, "other-rules").await;
    assert_eq!(
        status, 200,
        "creating the second set: expected 200, got {status} with {body}"
    );

    let held = RuleSpec {
        enabled: true,
        tls_policy: Some("Require".to_string()),
        recipients: vec!["held@example.com".to_string()],
        ..RuleSpec::bare("held.rule-1")
    };
    for set in ["inbound-rules", "other-rules"] {
        let (status, body) = create_rule(&router, set, None, &held).await;
        assert_eq!(
            status, 200,
            "seeding {set}: expected 200, got {status} with {body}"
        );
    }

    // The rows differ in what the second request carries under the same name and
    // where it asks for it: a bare rule, a rule with an action, and one placed
    // after the held rule rather than at the beginning.
    let rows: [(Option<&str>, RuleSpec); 3] = [
        (None, RuleSpec::bare(&held.name)),
        (
            None,
            RuleSpec {
                scan_enabled: true,
                action: Some(ActionSpec {
                    kind: "S3Action",
                    members: vec![("BucketName", "another-bucket".to_string())],
                }),
                ..RuleSpec::bare(&held.name)
            },
        ),
        (Some(&held.name), RuleSpec::bare(&held.name)),
    ];
    for (after, row) in &rows {
        let case = format!("After={after:?} {row:?}");
        let (status, body) = create_rule(&router, "inbound-rules", *after, row).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("AlreadyExists"),
            "case {case}: expected the modelled AlreadyExists, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("Rule already exists: {}", held.name).as_str()),
            "case {case}: expected the held rule named in the message, got {body}"
        );

        // The set still holds the one rule, exactly as it was stored.
        let (status, body) = describe_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        assert_eq!(
            reported_names(&body),
            vec![held.name.clone()],
            "case {case}: the refused create must add no rule, got {body}"
        );
        assert_reports(&case, &element(&body, "Rules").unwrap_or_default(), &held);
    }

    // A name differing only in case is a DIFFERENT name, so it is created rather
    // than refused.
    let other_case = RuleSpec::bare("HELD.RULE-1");
    let (status, body) = create_rule(&router, "inbound-rules", None, &other_case).await;
    assert_eq!(
        status, 200,
        "case a name differing only in case: expected 200, got {status} with {body}"
    );
    let (status, body) = describe_rule_set(&router, "inbound-rules").await;
    assert_eq!(
        status, 200,
        "case a name differing only in case: expected 200 from DescribeReceiptRuleSet, \
         got {status} with {body}"
    );
    assert_eq!(
        reported_names(&body),
        vec![other_case.name.clone(), held.name.clone()],
        "case a name differing only in case: expected both rules, got {body}"
    );
}

/// An `After` naming no rule of the set names no existing rule: the call is
/// refused with the modelled `RuleDoesNotExist` naming it, and nothing is stored.
#[tokio::test]
async fn an_after_naming_no_rule_of_the_set_is_refused() {
    let router = emulator();
    for set in ["inbound-rules", "other-rules"] {
        let (status, body) = create_rule_set(&router, set).await;
        assert_eq!(
            status, 200,
            "creating {set}: expected 200, got {status} with {body}"
        );
    }
    let held = RuleSpec::bare("held-rule");
    let (status, body) = create_rule(&router, "inbound-rules", None, &held).await;
    assert_eq!(
        status, 200,
        "seeding the held rule: expected 200, got {status} with {body}"
    );
    let (status, body) = create_rule(
        &router,
        "other-rules",
        None,
        &RuleSpec::bare("only-in-other-set"),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding the other set's rule: expected 200, got {status} with {body}"
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
        "n".repeat(MAX_RECEIPT_RULE_NAME),
        "not a legal name!".to_string(),
    ];
    for after in &absent {
        let case = format!("After={after:?}");
        let new = RuleSpec::bare("new-rule");
        let (status, body) = create_rule(&router, "inbound-rules", Some(after), &new).await;
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
            Some(format!("Rule does not exist: {after}").as_str()),
            "case {case}: expected the absent rule named in the message, got {body}"
        );

        // The refused create stored nothing: the set still holds only the rule it
        // was seeded with.
        let (status, body) = describe_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        assert_eq!(
            reported_names(&body),
            vec![held.name.clone()],
            "case {case}: the refused create must add no rule, got {body}"
        );
    }
}

/// A rule set the account does not hold is refused with the modelled
/// `RuleSetDoesNotExist` naming it, and no set is created for it.
#[tokio::test]
async fn creating_a_rule_in_a_set_that_does_not_exist_is_refused() {
    let router = emulator();
    let (status, body) = create_rule_set(&router, "present-set").await;
    assert_eq!(
        status, 200,
        "creating the present set: expected 200, got {status} with {body}"
    );

    // The rows differ in the form and size of the absent set name, including one
    // that differs from the present set only in case, one carrying a period —
    // which a rule set name, unlike a rule name, may not — and one that is not
    // even a legal rule set name.
    let absent = [
        "absent-set".to_string(),
        "a".to_string(),
        "PRESENT-SET".to_string(),
        "present.set".to_string(),
        "x".repeat(MAX_RECEIPT_RULE_NAME),
        "not a legal name!".to_string(),
    ];
    for rule_set_name in &absent {
        let case = format!("absent rule set {rule_set_name:?}");
        let (status, body) =
            create_rule(&router, rule_set_name, None, &RuleSpec::bare("new-rule")).await;
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

        // The refused create made no set under that name.
        let (status, body) = describe_rule_set(&router, rule_set_name).await;
        assert_eq!(
            status, 400,
            "case {case}: the refused create must make no rule set, got {status} with {body}"
        );
        assert_eq!(
            error(&body).0.as_deref(),
            Some("RuleSetDoesNotExist"),
            "case {case}: {rule_set_name:?} must not exist, got {body}"
        );
    }

    // The set the account does hold is still empty: no refusal above reached it.
    let (status, body) = describe_rule_set(&router, "present-set").await;
    assert_eq!(
        status, 200,
        "present-set after the refusals: expected 200, got {status} with {body}"
    );
    assert!(
        reported_names(&body).is_empty(),
        "present-set after the refusals: expected no rule at all, got {body}"
    );
}

/// A rule name outside the form `ReceiptRule$Name` documents is refused, and one
/// inside it is stored.
#[tokio::test]
async fn a_rule_name_outside_the_documented_form_is_refused() {
    // The catalogue comes from `ReceiptRule$Name`: "Contain only ASCII letters
    // (a-z, A-Z), numbers (0-9), underscores (_), dashes (-), or periods (.)",
    // "Start and end with a letter or number", "Contain 64 characters or fewer".
    // The accepted rows walk that character set, both ends and the published
    // ceiling; the refused rows break one clause each, including the character a
    // rule name admits one past its ceiling.
    let accepted = [
        "a".to_string(),
        "A1".to_string(),
        "0rule9".to_string(),
        "rule_name".to_string(),
        "rule-name".to_string(),
        "rule.name".to_string(),
        "n".repeat(MAX_RECEIPT_RULE_NAME),
    ];
    let refused = [
        "-leading-dash".to_string(),
        "trailing-dash-".to_string(),
        ".leading-period".to_string(),
        "trailing-period.".to_string(),
        "_leading-underscore".to_string(),
        "has space".to_string(),
        "has!bang".to_string(),
        "has/slash".to_string(),
        "n".repeat(MAX_RECEIPT_RULE_NAME + 1),
        "rüle".to_string(),
    ];

    let router = emulator();
    let (status, body) = create_rule_set(&router, "inbound-rules").await;
    assert_eq!(
        status, 200,
        "creating the set: expected 200, got {status} with {body}"
    );

    for name in &accepted {
        let case = format!("accepted name {name:?}");
        let rule = RuleSpec::bare(name);
        let (status, body) = create_rule(&router, "inbound-rules", None, &rule).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from CreateReceiptRule, got {status} with {body}"
        );
        let (status, body) = describe_rule(&router, "inbound-rules", name).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRule, got {status} with {body}"
        );
        assert_reports(&case, &element(&body, "Rule").unwrap_or_default(), &rule);
    }

    let held = reported_names(&describe_rule_set(&router, "inbound-rules").await.1);
    for name in &refused {
        let case = format!("refused name {name:?}");
        let (status, body) = create_rule(&router, "inbound-rules", None, &RuleSpec::bare(name)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("InvalidParameterValue"),
            "case {case}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(
                format!(
                    "Rule name must contain only ASCII letters, numbers, underscores, dashes \
                     or periods, start and end with a letter or number, and contain \
                     {MAX_RECEIPT_RULE_NAME} characters or fewer, got {name:?}"
                )
                .as_str()
            ),
            "case {case}: expected the rule the name broke in the message, got {body}"
        );

        // The refused create stored nothing.
        let (status, body) = describe_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        assert_eq!(
            reported_names(&body),
            held,
            "case {case}: the refused create must add no rule, got {body}"
        );
    }
}

/// An action whose member names no resource of the service that member documents
/// is an action SES could never take: the call is refused with that member's own
/// modelled error, and nothing is stored.
#[tokio::test]
async fn an_action_naming_no_resource_of_its_service_is_refused() {
    // Each row is (the action the request carries, the error code it is refused
    // with). The rows span every member of an action that names another service's
    // resource — the topic ARN any action may publish to, the bucket and KMS key
    // of an S3 action, the function of a Lambda action — in the forms that name
    // no such resource: not an ARN at all, an ARN of another vendor, and an ARN
    // of another resource type of the right vendor. The last row carries TWO
    // actions in one entry, which `ReceiptAction` documents as impossible: "An
    // instance of this data type can represent only one action."
    let rows: [(ActionSpec, &str); 11] = [
        (
            ActionSpec {
                kind: "S3Action",
                members: vec![("BucketName", " ".to_string())],
            },
            "InvalidS3Configuration",
        ),
        (
            ActionSpec {
                kind: "S3Action",
                members: vec![
                    ("BucketName", "inbound-mail-bucket".to_string()),
                    ("KmsKeyArn", "not-an-arn".to_string()),
                ],
            },
            "InvalidS3Configuration",
        ),
        (
            ActionSpec {
                kind: "S3Action",
                members: vec![
                    ("BucketName", "inbound-mail-bucket".to_string()),
                    ("KmsKeyArn", arn("sns", "inbound-notifications")),
                ],
            },
            "InvalidS3Configuration",
        ),
        (
            ActionSpec {
                kind: "S3Action",
                members: vec![
                    ("BucketName", "inbound-mail-bucket".to_string()),
                    ("TopicArn", arn("sqs", "inbound-queue")),
                ],
            },
            "InvalidSnsTopic",
        ),
        (
            ActionSpec {
                kind: "SNSAction",
                members: vec![("TopicArn", "inbound-notifications".to_string())],
            },
            "InvalidSnsTopic",
        ),
        (
            ActionSpec {
                kind: "SNSAction",
                members: vec![(
                    "TopicArn",
                    // A SUBSCRIPTION ARN: a topic ARN names no subresource, so
                    // the further `:<subscription-id>` segment names no topic.
                    arn(
                        "sns",
                        "inbound-notifications:00000000-0000-0000-0000-000000000000",
                    ),
                )],
            },
            "InvalidSnsTopic",
        ),
        (
            ActionSpec {
                kind: "StopAction",
                members: vec![
                    ("Scope", "RuleSet".to_string()),
                    ("TopicArn", arn("sns", "")),
                ],
            },
            "InvalidSnsTopic",
        ),
        (
            ActionSpec {
                kind: "BounceAction",
                members: vec![
                    ("SmtpReplyCode", "550".to_string()),
                    ("Message", "Mailbox does not exist".to_string()),
                    ("Sender", "postmaster@example.com".to_string()),
                    ("TopicArn", "arn:aws:sns".to_string()),
                ],
            },
            "InvalidSnsTopic",
        ),
        (
            ActionSpec {
                kind: "LambdaAction",
                members: vec![("FunctionArn", "InboundMail".to_string())],
            },
            "InvalidLambdaFunction",
        ),
        (
            ActionSpec {
                kind: "LambdaAction",
                members: vec![("FunctionArn", arn("lambda", "layer:InboundMail"))],
            },
            "InvalidLambdaFunction",
        ),
        (
            ActionSpec {
                kind: "S3Action",
                members: vec![("BucketName", "inbound-mail-bucket".to_string())],
            },
            "InvalidParameterValue",
        ),
    ];

    let router = emulator();
    let (status, body) = create_rule_set(&router, "inbound-rules").await;
    assert_eq!(
        status, 200,
        "creating the set: expected 200, got {status} with {body}"
    );

    for (index, (action, expected_code)) in rows.iter().enumerate() {
        let case = format!("row {index} {action:?} -> {expected_code}");
        let mut params = vec![field("RuleSetName", "inbound-rules")];
        params.extend(
            RuleSpec {
                action: Some(action.clone()),
                ..RuleSpec::bare("inbound-rule")
            }
            .fields(),
        );
        // The LAST row's second action, written at the SAME list entry as the
        // first: one `ReceiptAction` carrying two actions.
        if *expected_code == "InvalidParameterValue" {
            params.push(field("Rule.Actions.member.1.StopAction.Scope", "RuleSet"));
        }
        let (status, body) = call(&router, "CreateReceiptRule", &params).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error(&body).0.as_deref(),
            Some(*expected_code),
            "case {case}: expected the refusal {expected_code}, got {body}"
        );
        assert!(
            error(&body)
                .1
                .is_some_and(|message| message.contains("Actions.member.1")),
            "case {case}: expected the refused action member named in the message, got {body}"
        );

        // The refused create stored nothing.
        let (status, body) = describe_rule_set(&router, "inbound-rules").await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from DescribeReceiptRuleSet, got {status} with {body}"
        );
        assert!(
            reported_names(&body).is_empty(),
            "case {case}: the refused create must add no rule, got {body}"
        );
    }
}

/// A set already holding as many rules as one rule set may hold refuses another
/// one with the modelled `LimitExceeded`, and keeps the rules it holds.
#[tokio::test]
async fn a_set_at_its_rule_ceiling_refuses_another_rule() {
    let router = emulator();
    let (status, body) = create_rule_set(&router, "inbound-rules").await;
    assert_eq!(
        status, 200,
        "creating the set: expected 200, got {status} with {body}"
    );

    for index in 0..MAX_RECEIPT_RULES_PER_RULE_SET {
        let (status, body) = create_rule(
            &router,
            "inbound-rules",
            None,
            &RuleSpec::bare(&format!("rule-{index}")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case rule {index} of {MAX_RECEIPT_RULES_PER_RULE_SET}: expected 200, got {status} \
             with {body}"
        );
    }

    // The rows differ in where the refused rule asked to be placed: the
    // ceiling is on the set, not on one position in it.
    let rows = [None, Some("rule-0")];
    for after in rows {
        let case = format!("After={after:?}");
        let (status, body) =
            create_rule(&router, "inbound-rules", after, &RuleSpec::bare("one-too-many")).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        let (code, message) = error(&body);
        assert_eq!(
            code.as_deref(),
            Some("LimitExceeded"),
            "case {case}: expected the modelled LimitExceeded, got {body}"
        );
        assert_eq!(
            message.as_deref(),
            Some(
                format!(
                    "Rule limit exceeded: a receipt rule set may hold \
                     {MAX_RECEIPT_RULES_PER_RULE_SET} receipt rules."
                )
                .as_str()
            ),
            "case {case}: expected the ceiling named in the message, got {body}"
        );
    }

    // The set still holds exactly the rules that fit.
    let (status, body) = describe_rule_set(&router, "inbound-rules").await;
    assert_eq!(
        status, 200,
        "the set at its ceiling: expected 200, got {status} with {body}"
    );
    assert_eq!(
        reported_names(&body).len(),
        MAX_RECEIPT_RULES_PER_RULE_SET,
        "the set at its ceiling: expected {MAX_RECEIPT_RULES_PER_RULE_SET} rules, got {body}"
    );
}
