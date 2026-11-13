// SPDX-License-Identifier: CC0-1.0

//! The LENGTH of the explanatory text this service answers a failure with, driven end to
//! end over its wire protocol.
//!
//! Every call here goes through the same `MultiServiceRouter` the production router uses:
//! `POST /` with `X-Amz-Target: Invoicing.<OperationName>` and an awsJson body, so what
//! these tests see is exactly the error document an SDK client reads.
//!
//! The rule pinned here: every `message` member of every error shape of this model targets
//! `BasicString`, which the model gives `@length(min: 0, max: 1024)`
//! (`smithy_query(query="shape", shape="ValidationException,BasicString")`), so no failure
//! this service answers states a text past that many characters — whatever the request
//! that produced it. Two consequences the rows span:
//!
//! 1. A refusal that states one clause per rejected member states as many WHOLE clauses as
//!    the bound leaves room for, counts the clauses it states, and carries one `fieldList`
//!    entry per clause it states, each entry's own text inside the same bound. The rows
//!    differ along what the rule generalises over: how many members one request has
//!    rejected (one, a few, more than fit, and the most the list constraints admit), the
//!    constraint class the clause states (a `@length` minimum, a `@pattern`, an absent
//!    `@required` member), which member and which list the rejected values sit in, and
//!    which operation refuses the request.
//! 2. A failure whose text NAMES something the caller supplied is held to the same bound.
//!    `ProcurementPortalIdString` is the one input shape of this model with no maximum
//!    length (`smithy_query(query="operation",
//!    operation="ListProcurementPortalSuppliers")`), so the 404 that names the identifier
//!    is the row where a single caller-supplied value can run a message past the bound; it
//!    is driven at the bound itself and past it, and the identifiers that fit are reported
//!    whole.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

/// The longest text a `message` member of this model may carry: every one of them targets
/// `BasicString`, `@length(min: 0, max: 1024)` (`smithy_query(query="shape",
/// shape="ValidationException,BasicString")`), and `@length` on a string counts
/// CHARACTERS.
const MAX_MESSAGE_LENGTH: usize = 1024;

/// The key the invoice-unit, invoice-profile and tag planes carry their explanatory text
/// under: the member name the model spells.
const MODEL_MEMBER_KEY: &str = "message";

/// The key the procurement-portal plane carries it under, as that plane's recorded bodies
/// key it.
const PORTAL_PLANE_KEY: &str = "Message";

/// An ARN of the modelled `TagrisArn` form (`smithy_query(query="operation",
/// operation="TagResource")`: `@length(20..2048)` and its own `@pattern`), so a tag
/// request is refused for its tags rather than for its ARN.
const RESOURCE_ARN: &str = "arn:aws:invoicing::000000000000:invoice-unit/ffffffffffff";

/// `Member must have length greater than or equal to 1` — the requirement an empty tag key
/// fails (`ResourceTagKey` carries `@length(min: 1, max: 128)`).
const MIN_LENGTH_ONE: &str = "Member must have length greater than or equal to 1";

/// The expression the service advertises for `AccountIdString`'s `@pattern` (`^\d{12}$`
/// without the anchors a model spells it with).
const ACCOUNT_ID_EXPRESSION: &str = r"\d{12}";

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
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The explanatory text, under the key the answering plane carries it.
    fn text(&self, key: &str) -> String {
        self.body
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// One awsJson request, returning everything a client can see of the answer.
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

/// The clause a refusal states for one rejected member: the member path and the
/// requirement it failed, and nothing of the value.
fn clause(path: &str, requirement: &str) -> String {
    format!("Value at '{path}' failed to satisfy constraint: {requirement}")
}

/// The message the `fieldList` entry for a rejected member states: the requirement it
/// failed, about the value the entry refused — withheld behind the `[redacted]` marker —
/// with the member left to the entry's own `name`.
///
/// An ABSENT member is the one exception: there was no value to withhold, so the entry
/// keeps the `null` the envelope reports it as (`crate::service::validation`).
fn field_message(clause: &str) -> String {
    let requirement = requirement_of(clause);
    let value = match clause.starts_with(ABSENT_VALUE_PREFIX) {
        true => "null",
        false => "[redacted]",
    };
    format!("Value {value} failed to satisfy constraint: {requirement}")
}

/// How a clause opens when the member it names was ABSENT rather than rejected for its
/// value.
const ABSENT_VALUE_PREFIX: &str = "Value null at '";

/// The member path a clause names, read back out of it.
fn path_of(clause: &str) -> String {
    clause
        .split('\'')
        .nth(1)
        .unwrap_or_else(|| panic!("a clause names a member path, got {clause:?}"))
        .to_string()
}

/// The requirement a clause states, read back out of it.
fn requirement_of(clause: &str) -> String {
    clause
        .split_once("failed to satisfy constraint: ")
        .map(|(_, requirement)| requirement.to_string())
        .unwrap_or_else(|| panic!("a clause states a requirement, got {clause:?}"))
}

/// The AWS envelope over `clauses`, which counts the clauses it states.
fn envelope(clauses: &[String]) -> String {
    match clauses {
        [only] => format!("1 validation error detected: {only}"),
        several => format!(
            "{} validation errors detected: {}",
            several.len(),
            several.join("; ")
        ),
    }
}

/// The count an envelope announces and the clauses it states.
fn stated(message: &str) -> (usize, Vec<&str>) {
    let (count, joined) = message
        .split_once(" validation error detected: ")
        .or_else(|| message.split_once(" validation errors detected: "))
        .unwrap_or_else(|| panic!("an AWS validation envelope, got {message:?}"));
    (
        count
            .parse()
            .unwrap_or_else(|_| panic!("an envelope counts its clauses, got {message:?}")),
        joined.split("; ").collect(),
    )
}

/// One refused request: what it carries, and the clause each rejected member states.
struct Case {
    operation: &'static str,
    label: String,
    request: Value,
    /// Every clause the request earns, in the order the validating walk finds them.
    clauses: Vec<String>,
}

impl std::fmt::Debug for Case {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.operation, self.label)
    }
}

/// `count` tags, every `Key` the empty string: `ResourceTagList` is `@length(0..200)`, so
/// one array carries up to 200 rejected keys.
fn empty_keyed_tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|_| json!({"Key": "", "Value": "v"}))
            .collect(),
    )
}

/// The clauses `count` tags with an empty key earn, under the list member `member`.
fn tag_key_clauses(member: &str, count: usize) -> Vec<String> {
    (1..=count)
        .map(|position| clause(&format!("{member}.{position}.member.key"), MIN_LENGTH_ONE))
        .collect()
}

/// Every row of the table.
fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    // One request, one rejected member per tag: one clause, a few, more than the bound
    // leaves room for, and the most `ResourceTagList`'s `@length(max: 200)` admits.
    for count in [1usize, 2, 5, 11, 100, 200] {
        cases.push(Case {
            operation: "TagResource",
            label: format!("{count} tags with an empty key"),
            request: json!({"ResourceArn": RESOURCE_ARN, "ResourceTags": empty_keyed_tags(count)}),
            clauses: tag_key_clauses("resourceTags", count),
        });
    }
    // The same rejected member on another operation and another plane: the bound belongs
    // to the member the text travels in, not to the operation.
    for count in [3usize, 200] {
        cases.push(Case {
            operation: "CreateInvoiceUnit",
            label: format!("a create carrying {count} tags with an empty key"),
            request: json!({
                "Name": "BoundedUnit",
                "InvoiceReceiver": "123456789012",
                "TaxInheritanceDisabled": false,
                "Rule": {"LinkedAccounts": ["210987654321"]},
                "ResourceTags": empty_keyed_tags(count),
            }),
            clauses: tag_key_clauses("resourceTags", count),
        });
    }
    // A list of STRINGS rather than of structures: `ResourceTagKeyList` is `@length(0..200)`
    // and its member carries `@length(min: 1)`.
    cases.push(Case {
        operation: "UntagResource",
        label: "200 empty tag keys".to_string(),
        request: json!({
            "ResourceArn": RESOURCE_ARN,
            "ResourceTagKeys": Value::Array((0..200).map(|_| json!("")).collect()),
        }),
        clauses: (1..=200)
            .map(|position| clause(&format!("resourceTagKeys.{position}.member"), MIN_LENGTH_ONE))
            .collect(),
    });
    // A `@pattern` clause, which is longer than a `@length` one — so fewer of them fit —
    // on the longest list this model admits (`AccountIdList` is `@length(1..1000)`).
    for count in [4usize, 1000] {
        cases.push(Case {
            operation: "BatchGetInvoiceProfile",
            label: format!("{count} account ids outside the account-id pattern"),
            request: json!({
                "AccountIds": Value::Array((0..count).map(|_| json!("nope")).collect()),
            }),
            clauses: (1..=count)
                .map(|position| {
                    clause(
                        &format!("accountIds.{position}.member"),
                        &format!("Member must satisfy regular expression pattern: {ACCOUNT_ID_EXPRESSION}"),
                    )
                })
                .collect(),
        });
    }
    // The invoice-document reader's own `@required` member, absent: the same wording from
    // the same formatter, held to the same bound, in the same modelled shape.
    cases.push(Case {
        operation: "GetInvoicePDF",
        label: "no InvoiceId at all".to_string(),
        request: json!({}),
        clauses: vec![
            "Value null at 'invoiceId' failed to satisfy constraint: Member must not be null"
                .to_string(),
        ],
    });
    cases
}

/// RULE 1: a refusal fits the bound the model gives the member its text travels in, by
/// stating whole clauses and counting the ones it states — and both halves of the body
/// state the same ones.
#[tokio::test]
async fn a_refusal_states_as_many_whole_clauses_as_the_modelled_bound_admits() {
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

        let message = answer.text(MODEL_MEMBER_KEY);
        let length = message.chars().count();
        assert!(
            length <= MAX_MESSAGE_LENGTH,
            "case {case:?}: the message must fit the modelled {MAX_MESSAGE_LENGTH} \
             characters, got {length}: {message:?}"
        );

        // The count the envelope announces is the number of clauses it states, and each
        // one is a WHOLE clause of the walk, in the order the walk found them.
        let (count, clauses) = stated(&message);
        assert_eq!(
            count,
            clauses.len(),
            "case {case:?}: the envelope counts the clauses it states, got {message:?}"
        );
        assert!(
            count >= 1 && count <= case.clauses.len(),
            "case {case:?}: expected between 1 and {} clauses, got {count}",
            case.clauses.len()
        );
        for (position, stated_clause) in clauses.iter().enumerate() {
            assert_eq!(
                *stated_clause,
                case.clauses[position].as_str(),
                "case {case:?}: clause {position} must be stated whole and in the walk's \
                 order, got {message:?}"
            );
        }

        if count == case.clauses.len() {
            // Nothing was dropped: the refusal states every rejected member.
            assert_eq!(
                message,
                envelope(&case.clauses),
                "case {case:?}: a refusal that fits states every clause, got {message:?}"
            );
        } else {
            // Clauses were dropped, so the message must be full: one more clause could
            // not have fitted. The widest a further clause and its separator could be is
            // the longest clause stated, the `; ` that would join it, and the digit the
            // count could grow by.
            let widest = clauses
                .iter()
                .map(|stated_clause| stated_clause.chars().count())
                .max()
                .unwrap_or_default();
            let room_left = MAX_MESSAGE_LENGTH - length;
            assert!(
                room_left < widest + 3,
                "case {case:?}: {count} of {} clauses were stated while {room_left} \
                 characters were left for another clause of at most {widest}",
                case.clauses.len()
            );
        }

        // One entry per clause the envelope states — the two halves state the same
        // violations — and every text of an entry inside the same modelled bound.
        let field_list = answer.body["fieldList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a fieldList, got {answer:?}"));
        assert_eq!(
            field_list.len(),
            count,
            "case {case:?}: expected one fieldList entry per clause stated, got {answer:?}"
        );
        for (position, entry) in field_list.iter().enumerate() {
            let name = entry["name"].as_str().unwrap_or_default();
            let entry_message = entry["message"].as_str().unwrap_or_default();
            assert_eq!(
                name,
                path_of(clauses[position]),
                "case {case:?}: entry {position} must name the member its clause names, \
                 got {answer:?}"
            );
            assert_eq!(
                entry_message,
                field_message(clauses[position]),
                "case {case:?}: entry {position} must state the requirement the envelope \
                 states for that member, got {answer:?}"
            );
            for (member, text) in [("name", name), ("message", entry_message)] {
                let text_length = text.chars().count();
                assert!(
                    text_length <= MAX_MESSAGE_LENGTH,
                    "case {case:?}: entry {position}'s {member} must fit the modelled \
                     {MAX_MESSAGE_LENGTH} characters, got {text_length}"
                );
            }
        }
    }
}

/// The sentence the procurement-portal plane's 404 states for an identifier no portal of
/// this service answers to.
fn no_such_portal_sentence(identifier: &str) -> String {
    format!("Procurement portal with identifier {identifier} does not exist")
}

/// RULE 2: a failure whose text NAMES what the caller supplied is held to the same bound,
/// including for the one input shape of this model that has no maximum length of its own.
///
/// The rows are the identifier lengths around the bound: short, at the longest identifier
/// the whole sentence fits, one past it, and far past it.
#[tokio::test]
async fn a_not_found_naming_an_unbounded_identifier_fits_the_modelled_bound() {
    let router = emulator();

    // The longest identifier the whole sentence still fits: the bound less the sentence's
    // own fixed text.
    let fixed = no_such_portal_sentence("").chars().count();
    let fitting = MAX_MESSAGE_LENGTH - fixed;

    for length in [1usize, 40, fitting, fitting + 1, 4 * MAX_MESSAGE_LENGTH] {
        // An identifier of `@length(min: 1)` and no maximum that names no portal: the
        // catalog's identifiers are lowercase portal names, so a run of one letter is
        // none of them at any length.
        let identifier = "z".repeat(length);
        let answer = call(
            &router,
            "ListProcurementPortalSuppliers",
            &json!({"PortalIdentifier": identifier}),
        )
        .await;
        assert_eq!(
            answer.status, 404,
            "case an identifier of {length} characters: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case an identifier of {length} characters: expected a \
             ResourceNotFoundException, got {answer:?}"
        );

        let message = answer.text(PORTAL_PLANE_KEY);
        let stated_length = message.chars().count();
        assert!(
            stated_length <= MAX_MESSAGE_LENGTH,
            "case an identifier of {length} characters: the message must fit the modelled \
             {MAX_MESSAGE_LENGTH} characters, got {stated_length}"
        );

        let sentence = no_such_portal_sentence(&identifier);
        if sentence.chars().count() <= MAX_MESSAGE_LENGTH {
            // It fits, so it is stated whole — the bound truncates nothing a client could
            // otherwise have read.
            assert_eq!(
                message, sentence,
                "case an identifier of {length} characters: expected the whole sentence"
            );
        } else {
            // Past the bound, what is stated is that same sentence as far as the member
            // reaches: its own prefix, filled to the bound.
            let expected: String = sentence.chars().take(MAX_MESSAGE_LENGTH).collect();
            assert_eq!(
                message, expected,
                "case an identifier of {length} characters: expected the sentence as far \
                 as the member reaches"
            );
        }
    }
}
