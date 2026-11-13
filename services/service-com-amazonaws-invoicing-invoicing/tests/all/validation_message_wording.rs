// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request its modelled input constraints reject, driven end
//! to end over its wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and a JSON body, so
//! what these tests see is exactly the error document an SDK client reads.
//!
//! The rules pinned here, each over a table spanning the class rather than the one
//! request a recording happened to carry:
//!
//! 1. A constraint-violation clause names the offending MEMBER PATH and the CONSTRAINT
//!    that member violated, and NOTHING of the value the caller sent: `Value at
//!    '<path>' failed to satisfy constraint: <requirement>`, under the AWS envelope
//!    (`1 validation error detected: …` / `N validation errors detected: …; …`). An
//!    absent member is the one exception, reported as the `null` it was.
//! 2. A `@pattern` requirement quotes the expression WITHOUT the `^` / `$` anchors the
//!    model spells it with, so a caller can lift the advertised expression out of the
//!    message and re-apply it locally.
//! 3. Both halves of the modelled refusal body state the SAME requirement for the same
//!    rejected member: the `message` envelope, as prose about the whole request, names
//!    the member path, and the `fieldList` entry of that member names it in its own
//!    `name` member and states the requirement about the value it refused — with the
//!    value withheld behind the `[redacted]` marker, or the `null` an absent member was.
//! 4. Every `fieldList` element carries the `__type` discriminator — a shape id naming
//!    the field structure — the way the top-level error body does, so a client that
//!    decides how to decode a JSON document from its discriminator can decode the
//!    nested error detail.
//!
//! The rows differ along what the rules generalise over: the constraint class the model
//! declares (`@pattern`, both `@length` bounds, both `@range` bounds, an enum value set,
//! a `@required` member absent), the member's type (string, integer, list, a structure's
//! member, a list element), how many violations one request carries, and which operation
//! refuses it, every refusal being answered in the modelled `ValidationException` with
//! its `fieldList`.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The `ValidationExceptionReason` a rejected member value is stated with.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The shape a `fieldList` element's `__type` discriminator must name
/// (`smithy_query(query="shape", shape="ValidationExceptionFieldList")`: the list's
/// member targets `ValidationExceptionField`).
const FIELD_SHAPE_NAME: &str = "ValidationExceptionField";

/// A router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        engine,
    ));
    router
}

/// What one call answered.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type` on the awsJson protocols), shape name only.
    fn error_code(&self) -> String {
        shape_name(self.body.get("__type").and_then(Value::as_str).unwrap_or_default())
    }

    /// A member of the body, as a string.
    fn member(&self, name: &str) -> Option<&str> {
        self.body.get(name).and_then(Value::as_str)
    }

    /// The whole document as text, for the assertions about what must NOT be in it.
    fn text(&self) -> String {
        self.body.to_string()
    }
}

/// The shape name of a `__type` discriminator: a shape id (`<namespace>#<Shape>`) names
/// the shape after the `#`, and a bare shape name is its own.
fn shape_name(discriminator: &str) -> String {
    discriminator
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One request, returning everything a client can see of the refusal.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("Invoicing.{operation}"))
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// Whether the member was absent from the request, which is the one fact of a request's
/// shape a clause reports where it otherwise reports no value at all.
#[derive(Debug, Clone, Copy)]
enum Subject {
    Present,
    Absent,
}

/// One violation the refusal must state: the member path it names and the requirement
/// it quotes.
#[derive(Debug, Clone)]
struct Expected {
    path: &'static str,
    requirement: String,
    subject: Subject,
}

impl Expected {
    fn present(path: &'static str, requirement: impl Into<String>) -> Self {
        Self {
            path,
            requirement: requirement.into(),
            subject: Subject::Present,
        }
    }

    fn absent(path: &'static str) -> Self {
        Self {
            path,
            // The requirement an absent `@required` member failed.
            requirement: "Member must not be null".to_string(),
            subject: Subject::Absent,
        }
    }

    /// The clause AWS states for this violation inside the envelope: the subject, the
    /// member path and the requirement — and no rendering of the value.
    fn clause(&self) -> String {
        let subject = match self.subject {
            Subject::Present => "Value",
            Subject::Absent => "Value null",
        };
        format!(
            "{subject} at '{}' failed to satisfy constraint: {}",
            self.path, self.requirement
        )
    }

    /// The message the `fieldList` entry for this violation states: the value it refused
    /// — withheld behind the `[redacted]` marker, or the `null` an absent member was —
    /// and the same requirement, with the member left to the entry's own `name`.
    fn field_message(&self) -> String {
        let value = match self.subject {
            Subject::Present => "[redacted]",
            Subject::Absent => "null",
        };
        format!(
            "Value {value} failed to satisfy constraint: {}",
            self.requirement
        )
    }
}

/// `Member must satisfy regular expression pattern: <expression>`, with the expression
/// as the service advertises it — the model's `@pattern` without its anchors.
fn pattern_requirement(unanchored: &str) -> String {
    format!("Member must satisfy regular expression pattern: {unanchored}")
}

/// One refused request and everything the refusal must and must not say.
struct Case {
    operation: &'static str,
    /// What makes this row its own case, for the assertion messages.
    label: String,
    request: Value,
    expected: Vec<Expected>,
    /// Caller-sent values that must appear NOWHERE in the refusal document.
    rejected: Vec<String>,
    /// The anchored model expressions that must appear nowhere in it either.
    anchored: Vec<&'static str>,
}

impl std::fmt::Debug for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.operation, self.label)
    }
}

impl Case {
    fn new(operation: &'static str, label: &str, request: Value, expected: Vec<Expected>) -> Self {
        Self {
            operation,
            label: label.to_string(),
            request,
            expected,
            rejected: Vec::new(),
            anchored: Vec::new(),
        }
    }

    fn rejecting(mut self, value: impl Into<String>) -> Self {
        self.rejected.push(value.into());
        self
    }

    fn anchored(mut self, pattern: &'static str) -> Self {
        self.anchored.push(pattern);
        self
    }

    /// The `message` this refusal must carry: the AWS envelope over its clauses.
    fn envelope(&self) -> String {
        let clauses: Vec<String> = self.expected.iter().map(Expected::clause).collect();
        match clauses.len() {
            1 => format!("1 validation error detected: {}", clauses[0]),
            n => format!("{n} validation errors detected: {}", clauses.join("; ")),
        }
    }
}

/// The 12-digit account id `@pattern` of `AccountIdString`
/// (`smithy_query(query="shape", shape="AccountIdString")`: `^\d{12}$`), and the
/// expression a message advertises for it.
const ACCOUNT_ID_PATTERN: &str = r"^\d{12}$";
const ACCOUNT_ID_EXPRESSION: &str = r"\d{12}";

/// The ARN `@pattern` of `InvoiceUnitArnString` (`smithy_query(query="shape",
/// shape="InvoiceUnitArnString")`), and the expression a message advertises for it.
const INVOICE_UNIT_ARN_PATTERN: &str =
    r"^arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$";
const INVOICE_UNIT_ARN_EXPRESSION: &str =
    r"arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+";

/// The ARN `@pattern` of `TagrisArn` (`smithy_query(query="shape", shape="TagrisArn")`:
/// `@length(20..2048)` beside it), and the expression a message advertises for it.
const TAGRIS_ARN_EXPRESSION: &str = r"arn:aws[-a-z0-9]*:(invoicing)::[0-9]{12}:[-a-zA-Z0-9/:_]+";

/// The ARN `@pattern` of `ProcurementPortalPreferenceArnString`
/// (`smithy_query(query="shape", shape="ProcurementPortalPreferenceArnString")`), and the
/// expression a message advertises for it.
const PREFERENCE_ARN_PATTERN: &str =
    r"^arn:aws:invoicing::[0-9]{12}:procurement-portal-preference/[-a-zA-Z0-9]+$";
const PREFERENCE_ARN_EXPRESSION: &str =
    r"arn:aws:invoicing::[0-9]{12}:procurement-portal-preference/[-a-zA-Z0-9]+";

/// A `CreateInvoiceUnit` body with every `@required` member, which a row then spoils.
fn create_request() -> Value {
    json!({
        "Name": "WordingUnit",
        "InvoiceReceiver": "123456789012",
        "TaxInheritanceDisabled": false,
        "Rule": {"LinkedAccounts": ["210987654321"]},
    })
}

/// Every row of the table: a request, and the clauses its refusal must state.
fn cases() -> Vec<Case> {
    let long_name = "n".repeat(51);
    let long_description = "d".repeat(501);
    vec![
        // `@pattern` on a top-level string member — the shape of the recorded request.
        Case::new(
            "CreateInvoiceUnit",
            "an InvoiceReceiver outside the account-id pattern",
            {
                let mut request = create_request();
                request["InvoiceReceiver"] = json!("not-an-account-id");
                request
            },
            vec![Expected::present(
                "invoiceReceiver",
                pattern_requirement(ACCOUNT_ID_EXPRESSION),
            )],
        )
        .rejecting("not-an-account-id")
        .anchored(ACCOUNT_ID_PATTERN),
        // A different value outside the same pattern: the clause does not vary with it.
        Case::new(
            "CreateInvoiceUnit",
            "an InvoiceReceiver of the wrong digit count",
            {
                let mut request = create_request();
                request["InvoiceReceiver"] = json!("12345");
                request
            },
            vec![Expected::present(
                "invoiceReceiver",
                pattern_requirement(ACCOUNT_ID_EXPRESSION),
            )],
        )
        .rejecting("12345")
        .anchored(ACCOUNT_ID_PATTERN),
        // `@length`'s maximum on a string member (`InvoiceUnitName`, 1..50).
        Case::new(
            "CreateInvoiceUnit",
            "a Name past the length maximum",
            {
                let mut request = create_request();
                request["Name"] = json!(long_name);
                request
            },
            vec![Expected::present(
                "name",
                "Member must have length less than or equal to 50",
            )],
        )
        .rejecting(long_name.clone()),
        // … and its minimum.
        Case::new(
            "CreateInvoiceUnit",
            "a Name below the length minimum",
            {
                let mut request = create_request();
                request["Name"] = json!("");
                request
            },
            vec![Expected::present(
                "name",
                "Member must have length greater than or equal to 1",
            )],
        ),
        // A `@required` member absent: the one clause that reports the request's shape.
        Case::new(
            "CreateInvoiceUnit",
            "no Name at all",
            json!({
                "InvoiceReceiver": "123456789012",
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": ["210987654321"]},
            }),
            vec![Expected::absent("name")],
        ),
        // A violation on a LIST ELEMENT inside a structure member: the path reaches it,
        // and the requirement is the element's own.
        Case::new(
            "CreateInvoiceUnit",
            "a linked account outside the account-id pattern",
            {
                let mut request = create_request();
                request["Rule"] = json!({"LinkedAccounts": ["210987654321", "nope"]});
                request
            },
            vec![Expected::present(
                "rule.linkedAccounts.2.member",
                pattern_requirement(ACCOUNT_ID_EXPRESSION),
            )],
        )
        .rejecting("nope")
        .anchored(ACCOUNT_ID_PATTERN),
        // A violation on a member INSIDE a list element keeps its own full path.
        Case::new(
            "CreateInvoiceUnit",
            "a resource tag with an empty key",
            {
                let mut request = create_request();
                request["ResourceTags"] = json!([{"Key": "", "Value": "v"}]);
                request
            },
            vec![Expected::present(
                "resourceTags.1.member.key",
                "Member must have length greater than or equal to 1",
            )],
        ),
        // TWO violations in one request: the envelope counts them and joins the clauses,
        // in the order the validating walk found them (the input shape's member order).
        Case::new(
            "CreateInvoiceUnit",
            "a bad receiver and an over-long name together",
            {
                let mut request = create_request();
                request["InvoiceReceiver"] = json!("not-an-account-id");
                request["Name"] = json!(long_name.clone());
                request
            },
            vec![
                Expected::present("name", "Member must have length less than or equal to 50"),
                Expected::present(
                    "invoiceReceiver",
                    pattern_requirement(ACCOUNT_ID_EXPRESSION),
                ),
            ],
        )
        .rejecting("not-an-account-id")
        .rejecting(long_name.clone())
        .anchored(ACCOUNT_ID_PATTERN),
        // `@pattern` on another operation's member, a different expression.
        Case::new(
            "GetInvoiceUnit",
            "an ARN outside the invoice-unit ARN pattern",
            json!({"InvoiceUnitArn": "not-an-arn"}),
            vec![Expected::present(
                "invoiceUnitArn",
                pattern_requirement(INVOICE_UNIT_ARN_EXPRESSION),
            )],
        )
        .rejecting("not-an-arn")
        .anchored(INVOICE_UNIT_ARN_PATTERN),
        // `@length`'s maximum on an optional string member.
        Case::new(
            "UpdateInvoiceUnit",
            "a Description past the length maximum",
            json!({
                "InvoiceUnitArn": "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff",
                "Description": long_description.clone(),
            }),
            vec![Expected::present(
                "description",
                "Member must have length less than or equal to 500",
            )],
        )
        .rejecting(long_description),
        // `@range`'s minimum on an INTEGER member (`MaxResultsInteger`, 1..500) …
        Case::new(
            "ListInvoiceUnits",
            "MaxResults below the range minimum",
            json!({"MaxResults": 0}),
            vec![Expected::present(
                "maxResults",
                "Member must have value greater than or equal to 1",
            )],
        ),
        // … and its maximum.
        Case::new(
            "ListInvoiceUnits",
            "MaxResults past the range maximum",
            json!({"MaxResults": 501}),
            vec![Expected::present(
                "maxResults",
                "Member must have value less than or equal to 500",
            )],
        ),
        // One member failing TWO constraints: a clause per requirement.
        Case::new(
            "ListTagsForResource",
            "a ResourceArn too short and outside the pattern",
            json!({"ResourceArn": "arn:aws:s3:::b"}),
            vec![
                Expected::present(
                    "resourceArn",
                    "Member must have length greater than or equal to 20",
                ),
                Expected::present("resourceArn", pattern_requirement(TAGRIS_ARN_EXPRESSION)),
            ],
        )
        .rejecting("arn:aws:s3:::b"),
        // A `@required` LIST member absent, on the tag plane.
        Case::new(
            "UntagResource",
            "no ResourceTagKeys at all",
            json!({"ResourceArn": "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff"}),
            vec![Expected::absent("resourceTagKeys")],
        ),
        // A violation on a LIST ELEMENT at the top level of the input, on the
        // invoice-profile plane.
        Case::new(
            "BatchGetInvoiceProfile",
            "an account id outside the account-id pattern",
            json!({"AccountIds": ["not-an-account"]}),
            vec![Expected::present(
                "accountIds.1.member",
                pattern_requirement(ACCOUNT_ID_EXPRESSION),
            )],
        )
        .rejecting("not-an-account")
        .anchored(ACCOUNT_ID_PATTERN),
        // `@length`'s minimum on a LIST member.
        Case::new(
            "BatchGetInvoiceProfile",
            "an empty AccountIds list",
            json!({"AccountIds": []}),
            vec![Expected::present(
                "accountIds",
                "Member must have length greater than or equal to 1",
            )],
        ),
        // `@pattern` on the procurement-portal plane, whose refusals are worded and
        // carried the same way: the modelled body belongs to the refusal, not the plane.
        Case::new(
            "GetProcurementPortalPreference",
            "a preference ARN outside the preference-ARN pattern",
            json!({"ProcurementPortalPreferenceArn": "not-an-arn"}),
            vec![Expected::present(
                "procurementPortalPreferenceArn",
                pattern_requirement(PREFERENCE_ARN_EXPRESSION),
            )],
        )
        .rejecting("not-an-arn")
        .anchored(PREFERENCE_ARN_PATTERN),
        // A `@required` member absent on a procurement-portal write.
        Case::new(
            "CreateProcurementPortalPreference",
            "no Contacts at all",
            json!({
                "ProcurementPortalName": "COUPA",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "AN01",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "AN02",
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": false,
            }),
            vec![Expected::absent("contacts")],
        ),
        // `@range`'s maximum on the procurement-portal plane's own `MaxResults` (1..100),
        // a different bound from the invoice-unit plane's.
        Case::new(
            "ListProcurementPortalPreferences",
            "MaxResults past the range maximum",
            json!({"MaxResults": 101}),
            vec![Expected::present(
                "maxResults",
                "Member must have value less than or equal to 100",
            )],
        ),
        // The invoice-document reader's own `@required` member, absent: the same wording,
        // in the same modelled shape as every other operation of this service.
        Case::new(
            "GetInvoicePDF",
            "no InvoiceId at all",
            json!({}),
            vec![Expected::absent("invoiceId")],
        ),
    ]
}

/// RULES 1-4 over the table: what a refused request is told, and what it is NOT told.
#[tokio::test]
async fn a_refusal_states_the_member_and_the_constraint_and_nothing_of_the_value() {
    let router = emulator();

    for case in cases() {
        let answer = call(&router, case.operation, &case.request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answer:?}"
        );

        // RULE 1 and RULE 2: the envelope, clause for clause.
        let message = answer.member("message").unwrap_or_default();
        assert_eq!(
            message,
            case.envelope(),
            "case {case:?}: expected the message {:?}, got {answer:?}",
            case.envelope()
        );

        // The value the caller sent appears nowhere in the document, and neither does
        // the anchored expression the model spells internally.
        let document = answer.text();
        for value in &case.rejected {
            assert!(
                !document.contains(value.as_str()),
                "case {case:?}: the refusal must not reproduce the rejected value \
                 {value:?}, got {answer:?}"
            );
        }
        for pattern in &case.anchored {
            // The document is JSON, which escapes a backslash: compare against the
            // expression as it would appear in the body.
            let escaped = pattern.replace('\\', "\\\\");
            assert!(
                !document.contains(pattern) && !document.contains(escaped.as_str()),
                "case {case:?}: the refusal must advertise the pattern without its \
                 anchors, got {answer:?}"
            );
        }

        assert_eq!(
            answer.member("reason"),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {answer:?}"
        );

        // RULE 3: one entry per violation, naming its member in `name` and stating the
        // requirement the envelope states for it.
        let field_list = answer.body["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList, got {answer:?}"));
        let entries: Vec<(String, String)> = field_list
            .iter()
            .map(|entry| {
                (
                    entry["name"].as_str().unwrap_or_default().to_string(),
                    entry["message"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let wanted: Vec<(String, String)> = case
            .expected
            .iter()
            .map(|expected| (expected.path.to_string(), expected.field_message()))
            .collect();
        assert_eq!(
            entries, wanted,
            "case {case:?}: expected the fieldList {wanted:?}, got {answer:?}"
        );

        // RULE 4: every element carries the `__type` discriminator, a shape id naming
        // the field structure.
        for entry in field_list {
            let discriminator = entry["__type"].as_str().unwrap_or_default();
            assert!(
                discriminator.contains('#') && !discriminator.starts_with('#'),
                "case {case:?}: a fieldList element's __type must be a shape id, got \
                 {discriminator:?} in {answer:?}"
            );
            assert_eq!(
                shape_name(discriminator),
                FIELD_SHAPE_NAME,
                "case {case:?}: the discriminator must name the field structure, got \
                 {discriminator:?} in {answer:?}"
            );
        }
    }
}

/// The refusal is of what falls OUTSIDE a constraint: a value ON a published bound, and
/// a request that satisfies every modelled constraint, are not refused at all.
///
/// Without this, every assertion above could be satisfied by refusing everything.
#[tokio::test]
async fn a_request_inside_every_constraint_is_not_refused() {
    let router = emulator();

    // `InvoiceUnitName` is `@length(1..50)` and `MaxResultsInteger` `@range(1..500)`:
    // the ends of both are accepted.
    let accepted: Vec<(&str, Value)> = vec![
        ("CreateInvoiceUnit", {
            let mut request = create_request();
            request["Name"] = json!("a");
            request
        }),
        ("CreateInvoiceUnit", {
            let mut request = create_request();
            request["Name"] = json!("n".repeat(50));
            request["InvoiceReceiver"] = json!("210987654321");
            // An account is linked into at most one invoice unit, and the row above
            // already created one: this unit links its own account.
            request["Rule"] = json!({"LinkedAccounts": ["310987654321"]});
            request
        }),
        ("ListInvoiceUnits", json!({"MaxResults": 1})),
        ("ListInvoiceUnits", json!({"MaxResults": 500})),
    ];

    for (operation, request) in accepted {
        let answer = call(&router, operation, &request).await;
        assert_eq!(
            answer.status, 200,
            "case {operation} {request}: a request inside every constraint is served, \
             got {answer:?}"
        );
    }
}
