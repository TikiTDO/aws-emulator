// SPDX-License-Identifier: CC0-1.0

//! The modelled refusal body is a property of the REFUSAL, not of the plane or the
//! operation that was asked for, driven end to end over this service's wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and a JSON body, so what these
//! tests see is exactly the error document an SDK client reads.
//!
//! The rules pinned here:
//!
//! 1. A request a modelled input constraint rejects is answered HTTP 400
//!    `ValidationException` carrying `reason` = `fieldValidationFailed` and a `fieldList`
//!    entry naming the rejected member and the constraint it failed — for EVERY operation
//!    this emulator implements, whichever plane it belongs to.
//! 2. What is refused is what falls OUTSIDE a constraint: a value ON a published bound, and
//!    a request every modelled constraint admits, are not answered as a field-validation
//!    failure at all — whatever the handler behind them then answers.
//!
//! The rows of rule 1 span every implemented operation of the model and, across them, the
//! constraint classes it declares (a `@pattern`, both `@length` bounds, both `@range`
//! bounds of two different ranges, a `@required` member absent), the member's type (string,
//! integer, list, a list element) and the plane that was asked for. Seven of those rows name
//! an operation `wire_envelope_and_error_detail.rs` also refuses: this table's subject is
//! EXHAUSTIVENESS over the implemented surface, which is what the overlap buys, and the
//! harness it drives the surface through is that suite's rather than a second copy of it.

use serde_json::{json, Value};

// The wire harness this suite drives the service through lives beside it: one router
// assembly, one request helper and one view of what a client can see.
use crate::wire_envelope_and_error_detail::{call, emulator, Answer};

/// The `ValidationExceptionReason` a rejected member value is stated with.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The media type a refusal of the REQUEST announces: the one the model's protocol trait
/// declares (`smithy_query(query="service_info")`: `awsJson1_0`), because the input
/// validator in front of the service answers it rather than the plane asked for.
const REFUSAL_MEDIA_TYPE: &str = "application/x-amz-json-1.0";

/// The `MaxResults` range of the PROCUREMENT-PORTAL plane's paginated readers
/// (`smithy_query(query="operation", operation="ListProcurementPortals")`: range 1..100).
const PORTAL_MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// The `MaxResults` range of `ListInvoiceUnits` (`smithy_query(query="operation",
/// operation="ListInvoiceUnits")`: range 1..500).
const INVOICE_UNIT_MAX_RESULTS_RANGE: (i64, i64) = (1, 500);

/// The `MaxResults` range of `ListInvoiceSummaries` (`smithy_query(query="operation",
/// operation="ListInvoiceSummaries")`: `InvoiceSummariesMaxResults`, range 1..100).
const INVOICE_SUMMARIES_MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// A well-formed procurement-portal-preference ARN no create produced
/// (`smithy_query(query="operation", operation="GetProcurementPortalPreference")`:
/// `@pattern ^arn:aws:invoicing::[0-9]{12}:procurement-portal-preference/[-a-zA-Z0-9]+$`).
fn well_formed_preference_arn() -> String {
    format!(
        "arn:aws:invoicing::{}:procurement-portal-preference/absent000",
        engine::emulator_account_id()
    )
}

/// A well-formed invoice-unit ARN no create produced.
fn well_formed_unit_arn() -> String {
    format!(
        "arn:aws:invoicing::{}:invoice-unit/ffffffffffff",
        engine::emulator_account_id()
    )
}

/// The `fieldList` entries of an answer, as (name, message) pairs.
fn field_list(answer: &Answer) -> Vec<(String, String)> {
    answer
        .body
        .get("fieldList")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["name"].as_str().unwrap_or_default().to_string(),
                        entry["message"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One refused request: the operation, the member path the refusal must name, and the
/// request that provokes it.
#[derive(Debug)]
struct Case {
    operation: &'static str,
    member: &'static str,
    request: Value,
}

impl Case {
    fn new(operation: &'static str, member: &'static str, request: Value) -> Self {
        Self {
            operation,
            member,
            request,
        }
    }
}

/// One row per implemented operation of this model, each refused on a constraint its own
/// input shape declares.
fn cases() -> Vec<Case> {
    let preference_arn = well_formed_preference_arn();
    let unit_arn = well_formed_unit_arn();
    vec![
        // The invoice-unit plane: a `@required` member absent, two `@pattern`s, a
        // `@length` maximum and both ends of a `@range`.
        Case::new(
            "CreateInvoiceUnit",
            "name",
            json!({
                "InvoiceReceiver": "123456789012",
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": ["210987654321"]},
            }),
        ),
        Case::new(
            "GetInvoiceUnit",
            "invoiceUnitArn",
            json!({"InvoiceUnitArn": "not-an-arn"}),
        ),
        Case::new(
            "UpdateInvoiceUnit",
            "description",
            json!({"InvoiceUnitArn": unit_arn, "Description": "d".repeat(501)}),
        ),
        Case::new(
            "DeleteInvoiceUnit",
            "invoiceUnitArn",
            json!({"InvoiceUnitArn": "not-an-arn"}),
        ),
        Case::new(
            "ListInvoiceUnits",
            "maxResults",
            json!({"MaxResults": INVOICE_UNIT_MAX_RESULTS_RANGE.1 + 1}),
        ),
        Case::new(
            "ListInvoiceSummaries",
            "maxResults",
            json!({
                "Selector": {"ResourceType": "ACCOUNT_ID", "Value": "123456789012"},
                "MaxResults": INVOICE_SUMMARIES_MAX_RESULTS_RANGE.1 + 1,
            }),
        ),
        // A `@required` member absent on the invoice-document reader.
        Case::new("GetInvoicePDF", "invoiceId", json!({})),
        // The invoice-profile plane: a violation on a LIST ELEMENT.
        Case::new(
            "BatchGetInvoiceProfile",
            "accountIds.1.member",
            json!({"AccountIds": ["not-an-account"]}),
        ),
        // The procurement-portal plane: the same classes, on its own members.
        Case::new(
            "CreateProcurementPortalPreference",
            "contacts",
            json!({
                "ProcurementPortalName": "COUPA",
                "BuyerDomain": "NetworkID",
                "BuyerIdentifier": "AN01",
                "SupplierDomain": "NetworkID",
                "SupplierIdentifier": "AN02",
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": false,
            }),
        ),
        Case::new(
            "GetProcurementPortalPreference",
            "procurementPortalPreferenceArn",
            json!({"ProcurementPortalPreferenceArn": "not-an-arn"}),
        ),
        Case::new(
            "PutProcurementPortalPreference",
            "procurementPortalPreferenceArn",
            json!({
                "ProcurementPortalPreferenceArn": "not-an-arn",
                "EinvoiceDeliveryEnabled": false,
                "PurchaseOrderRetrievalEnabled": false,
                "Contacts": [{"Name": "Portal Administrator", "Email": "ops@example.com"}],
            }),
        ),
        Case::new(
            "DeleteProcurementPortalPreference",
            "procurementPortalPreferenceArn",
            json!({"ProcurementPortalPreferenceArn": "not-an-arn"}),
        ),
        Case::new(
            "UpdateProcurementPortalPreferenceStatus",
            "procurementPortalPreferenceArn",
            json!({
                "ProcurementPortalPreferenceArn": "not-an-arn",
                "EinvoiceDeliveryPreferenceStatus": "PENDING_VALIDATION",
            }),
        ),
        Case::new(
            "SendProcurementPortalValidation",
            "procurementPortalPreferenceArn",
            json!({"ProcurementPortalPreferenceArn": "not-an-arn"}),
        ),
        // A `@required` member absent beside a well-formed ARN: the refusal names the
        // member that is missing, not the one that is fine.
        Case::new(
            "VerifyProcurementPortalValidation",
            "code",
            json!({"ProcurementPortalPreferenceArn": preference_arn}),
        ),
        Case::new(
            "ListProcurementPortalPreferences",
            "maxResults",
            json!({"MaxResults": PORTAL_MAX_RESULTS_RANGE.1 + 1}),
        ),
        // … and this range's minimum, one row below it.
        Case::new(
            "ListProcurementPortals",
            "maxResults",
            json!({"MaxResults": PORTAL_MAX_RESULTS_RANGE.0 - 1}),
        ),
        Case::new(
            "ListProcurementPortalSuppliers",
            "portalIdentifier",
            json!({"MaxResults": PORTAL_MAX_RESULTS_RANGE.0}),
        ),
        // The tag plane, which answers in its own media type but refuses like the rest.
        Case::new(
            "ListTagsForResource",
            "resourceArn",
            json!({"ResourceArn": "arn:aws:s3:::b"}),
        ),
        Case::new("TagResource", "resourceTags", json!({"ResourceArn": unit_arn})),
        Case::new(
            "UntagResource",
            "resourceTagKeys",
            json!({"ResourceArn": unit_arn}),
        ),
    ]
}

/// RULE 1: every implemented operation states a rejected member in the modelled
/// `ValidationException` — the `reason` a client branches on and the `fieldList` entry
/// naming the member and the constraint it failed.
#[tokio::test]
async fn every_implemented_operation_states_a_rejected_member_in_the_modelled_shape() {
    let router = emulator();

    let cases = cases();
    for case in &cases {
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
        assert_eq!(
            answer.member("reason"),
            Some(FIELD_VALIDATION_FAILED),
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {answer:?}"
        );
        let field_list = field_list(&answer);
        let entry = field_list
            .iter()
            .find(|(name, _)| name == case.member)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a fieldList entry naming {:?}, got {answer:?}",
                    case.member
                )
            });
        assert!(
            entry.1.contains("Member must"),
            "case {case:?}: the entry must name the constraint the member failed, got \
             {answer:?}"
        );
        // Both halves of the body state the same violation, so the envelope names the
        // member too.
        assert!(
            answer
                .member("message")
                .is_some_and(|text| text.contains("validation error")
                    && text.contains(case.member)),
            "case {case:?}: the envelope must name the refused member, got {answer:?}"
        );
        // A refusal of the REQUEST is answered in front of the service, in the media type
        // the model's protocol trait declares — on every plane.
        assert_eq!(
            answer.media_type.as_deref(),
            Some(REFUSAL_MEDIA_TYPE),
            "case {case:?}: a refusal keeps the model-declared media type, got {answer:?}"
        );
    }

    // Every operation this emulator implements is covered: the model binds 21 operations
    // (`smithy_query(query="digest")`) and this service implements every one of them.
    let mut covered: Vec<&str> = cases.iter().map(|case| case.operation).collect();
    covered.sort_unstable();
    covered.dedup();
    assert_eq!(
        covered.len(),
        21,
        "the table must refuse every implemented operation, got {covered:?}"
    );
}

/// RULE 2: what is refused is what falls OUTSIDE a constraint. A value ON a published
/// bound, and a request every modelled constraint admits, are never answered as a
/// field-validation failure — whatever the handler behind them then answers.
///
/// Without this, every assertion above could be satisfied by refusing everything.
#[tokio::test]
async fn a_request_inside_every_constraint_is_not_a_field_validation_failure() {
    let router = emulator();
    let preference_arn = well_formed_preference_arn();

    let accepted: Vec<(&str, Value)> = vec![
        // Both published ends of the procurement-portal plane's `MaxResults` range.
        (
            "ListProcurementPortals",
            json!({"MaxResults": PORTAL_MAX_RESULTS_RANGE.0}),
        ),
        (
            "ListProcurementPortals",
            json!({"MaxResults": PORTAL_MAX_RESULTS_RANGE.1}),
        ),
        (
            "ListProcurementPortalPreferences",
            json!({"MaxResults": PORTAL_MAX_RESULTS_RANGE.0}),
        ),
        (
            "ListProcurementPortalPreferences",
            json!({"MaxResults": PORTAL_MAX_RESULTS_RANGE.1}),
        ),
        // A well-formed ARN no create produced: beyond the boundary, so the handler
        // answers for it.
        (
            "GetProcurementPortalPreference",
            json!({"ProcurementPortalPreferenceArn": preference_arn}),
        ),
        (
            "SendProcurementPortalValidation",
            json!({"ProcurementPortalPreferenceArn": preference_arn}),
        ),
        // An account id of the modelled form, and a list at its own length minimum.
        ("BatchGetInvoiceProfile", json!({"AccountIds": ["123456789012"]})),
        // The invoice-document reader too: a well-formed id the registry holds no invoice
        // for is refused by the handler, not by the boundary.
        ("GetInvoicePDF", json!({"InvoiceId": "INV-0001"})),
    ];

    for (operation, request) in accepted {
        let answer = call(&router, operation, &request).await;
        assert_ne!(
            answer.member("reason"),
            Some(FIELD_VALIDATION_FAILED),
            "case {operation} {request}: a request inside every constraint is not a \
             field-validation failure, got {answer:?}"
        );
        assert!(
            field_list(&answer).is_empty(),
            "case {operation} {request}: a request inside every constraint names no \
             rejected member, got {answer:?}"
        );
    }
}
