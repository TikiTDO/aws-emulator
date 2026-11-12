// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WORDING of the refusals this service states in AWS's own
//! words rather than in its own: the validation clause a violated `@pattern` is reported
//! with, the sentence a `profileId` that resolves to nothing is reported with, the fixed
//! sentence a duplicate of an in-flight idempotent create is refused with, the fixed
//! sentence a transformer configured in both styles at once is refused with, and the JSON
//! parser's own diagnostic of a submitted document that does not parse.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one recorded request:
//!
//! * a violated `@pattern` states the expression WITHOUT the `^`/`$` anchors that bind it
//!   to the whole value, and withholds the rejected value for a `@sensitive` member while
//!   echoing it for every other member;
//! * a `profileId` the store does not hold is refused with the template AWS records for
//!   the path that resolved it: the read and the delete of the record state one, and the
//!   update, the partnership create and the narrowed listing state the other;
//! * a create duplicating one still in flight under the same `@idempotencyToken` is
//!   refused with one FIXED sentence that carries neither the token nor the resource type;
//! * a transformer configured with both the deprecated members and the ones that replace
//!   them is refused with one FIXED sentence that reports none of the members the request
//!   supplied;
//! * a submitted JSON document that does not parse is refused with the parser's own
//!   diagnostic — what the parse stopped on, what belonged there, and the position — with
//!   the document withheld and no input member named.
//!
//! Every row is driven over the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`)
//! through the same router the emulator boots, so each assertion is about what a real SDK
//! client observes.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;

use crate::onboarding_loop::{
    b2bi_router, call, error_code, given_capability, given_profile, given_transformer, text,
};

/// The envelope AWS states a refused input in: one clause per violation, counted.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The refusal sentence a response carries.
///
/// `ValidationException$Message` has a capital M in this service's model
/// (`smithy_query(query="shape",shape="ValidationException")`), while a refusal the
/// framework answers before any handler ran renders the sentence in the lower-cased
/// `message` member every other modelled error of this service uses — so a test that is
/// about the SENTENCE reads whichever of the two the response carries.
fn message_of(body: &Value) -> String {
    for member in ["Message", "message"] {
        let stated = text(body, member);
        if !stated.is_empty() {
            return stated;
        }
    }
    String::new()
}

/// One row of the pattern table: what a failing assertion calls it, the operation, the
/// request, the member path the clause names, the pattern as the clause states it, and the
/// value the clause echoes (`None` for a `@sensitive` member, whose value it withholds).
type PatternRow = (
    &'static str,
    &'static str,
    Value,
    &'static str,
    &'static str,
    Option<&'static str>,
);

/// RULE: a value outside a modelled `@pattern` is refused with `ValidationException`
/// (HTTP 400) whose clause states the regular expression WITHOUT the `^` and `$` anchors
/// that bind it to the whole value — and states the rejected value only where the member
/// is not `@sensitive`, because a `@sensitive` member's value may not reach a response.
///
/// The rows differ along what the rule generalises over: whether the member is
/// `@sensitive` (so whether the value is echoed), which operation and member carry the
/// pattern, and the shape of the pattern itself (a bare character class, one ending in a
/// quantifier, and one ending in an optional group) — every one of which the service
/// reports unanchored.
#[tokio::test]
async fn a_pattern_violation_states_the_expression_without_its_anchors() {
    let router = b2bi_router();
    let profile_id = given_profile(&router, "anchors").await;

    // (case, operation, request, the member path, the pattern as the clause states it,
    //  the value the clause echoes — `None` for a `@sensitive` member)
    //
    // Every pattern below is the one the model puts on that member
    // (`smithy_query(query="shape",shape="Email,Phone,ProfileId,TransformerId")`), stated
    // here with its anchors so the test shows exactly which two characters the service
    // drops.
    let rows: Vec<PatternRow> = vec![
        (
            "a @sensitive email, whose value the clause must withhold",
            "CreatePartnership",
            json!({
                "profileId": profile_id.clone(),
                "name": "anchored",
                "email": "not-an-email-address",
                "capabilities": []
            }),
            "email",
            r"[\w\.\-]+@[\w\.\-]+",
            None,
        ),
        (
            "a @sensitive phone, whose pattern ends in an optional group",
            "CreateProfile",
            json!({
                "name": "anchored",
                "phone": "not a phone",
                "businessName": "Anchored Corp",
                "logging": "DISABLED"
            }),
            "phone",
            r"\+?([0-9 \t\-()\/]{7,})(?:\s*(?:#|x\.?|ext\.?|extension) \t*(\d+))?",
            None,
        ),
        (
            "a profileId, which is not @sensitive, on a read",
            "GetProfile",
            json!({ "profileId": "not a profile id" }),
            "profileId",
            "[a-zA-Z0-9_-]+",
            Some("not a profile id"),
        ),
        (
            "a profileId, which is not @sensitive, on a delete",
            "DeleteProfile",
            json!({ "profileId": "not a profile id" }),
            "profileId",
            "[a-zA-Z0-9_-]+",
            Some("not a profile id"),
        ),
        (
            "a transformerId on a read",
            "GetTransformer",
            json!({ "transformerId": "not a transformer id" }),
            "transformerId",
            "[a-zA-Z0-9_-]+",
            Some("not a transformer id"),
        ),
        (
            "a transformer name, whose pattern ends in a quantifier",
            "CreateTransformer",
            json!({
                "name": "a name with spaces",
                "inputConversion": {
                    "fromFormat": "X12",
                    "formatOptions": {
                        "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                    }
                }
            }),
            "name",
            "[a-zA-Z0-9_-]{1,512}",
            Some("a name with spaces"),
        ),
    ];

    for (case, operation, request, member, pattern, echoed) in rows {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let subject = match echoed {
            Some(value) => format!("Value '{value}'"),
            None => "Value".to_string(),
        };
        let expected = one_violation(&format!(
            "{subject} at '{member}' failed to satisfy constraint: Member must satisfy \
             regular expression pattern: {pattern}"
        ));
        // `ValidationException$Message` has a capital M in this service's model
        // (`smithy_query(query="shape",shape="ValidationException")`); a refusal the
        // framework answers before the handler runs states it in `message`.
        let message = message_of(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains('^') && !message.contains('$'),
            "case {case:?}: the clause must state the expression unanchored, got {message:?}"
        );
    }
}

/// RULE: a `profileId` that resolves to no profile of this account is refused with
/// `ResourceNotFoundException` whose message is built from the REQUESTED id in the
/// template AWS records for that path: the two operations that address the profile record
/// itself (the read and the delete) state `Profile ID <id> not found.`, and every path
/// that resolves a `profileId` on the way to doing something else — the update, the
/// partnership create and the listing narrowed to one profile — states `Profile with
/// profileId <id> not found.`.
///
/// The rows differ along what the rule generalises over: which operation resolves the id
/// (both templates, over a read, a mutation, a delete, a create that references a profile
/// and a narrowed listing), and the shape of the unknown id (one no profile was ever
/// minted under, and one this account minted and deleted).
#[tokio::test]
async fn an_unresolved_profile_id_is_refused_with_the_template_recorded_for_its_path() {
    use service_com_amazonaws_b2bi_b2bi::state::{profile_not_found, profile_reference_not_found};

    let router = b2bi_router();
    let capability_id = given_capability(&router, "profile-not-found").await;

    // A profile that WAS minted and is now gone resolves to nothing exactly as one that
    // never existed does, so both id shapes are rows of the same table.
    let deleted = given_profile(&router, "deleted").await;
    let (status, body) = call(
        &router,
        "DeleteProfile",
        json!({ "profileId": deleted.clone() }),
    )
    .await;
    assert_eq!(status, 200, "DeleteProfile failed: {status} {body}");

    let unknown_ids = [
        ("an id no profile was ever minted under", "p-nonexistent000000".to_string()),
        ("a profile this account deleted", deleted.clone()),
    ];

    for (id_case, unknown) in unknown_ids {
        // (case, operation, request naming `unknown`, the template that path states)
        let rows: Vec<(&str, &str, Value, String)> = vec![
            (
                "a read of the record",
                "GetProfile",
                json!({ "profileId": unknown }),
                profile_not_found(&unknown),
            ),
            (
                "a delete of the record",
                "DeleteProfile",
                json!({ "profileId": unknown }),
                profile_not_found(&unknown),
            ),
            (
                "a mutation, which resolves the record before writing",
                "UpdateProfile",
                json!({ "profileId": unknown, "name": "renamed" }),
                profile_reference_not_found(&unknown),
            ),
            (
                "a create that references the profile",
                "CreatePartnership",
                json!({
                    "profileId": unknown,
                    "name": "partner",
                    "email": "partner@example.com",
                    "capabilities": [capability_id.clone()]
                }),
                profile_reference_not_found(&unknown),
            ),
            (
                "a listing narrowed to the profile",
                "ListPartnerships",
                json!({ "profileId": unknown }),
                profile_reference_not_found(&unknown),
            ),
        ];

        for (case, operation, request, expected) in rows {
            let case = format!("{case} with {id_case}");
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status,
                NOT_FOUND_STATUS,
                "case {case:?}: expected HTTP {NOT_FOUND_STATUS}, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {body}"
            );
            // `ResourceNotFoundException$message` is lower-cased in this service's model.
            let message = text(&body, "message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected message {expected:?}, got {message:?}"
            );
            assert!(
                message.contains(&unknown),
                "case {case:?}: the sentence is built from the requested id, got {message:?}"
            );
        }
    }
}

/// RULE: a create that duplicates one still in flight under the same
/// `@idempotencyToken` is refused with `ConflictException` (HTTP 409) and ONE FIXED
/// sentence — it carries neither the token the caller generated nor the kind of resource
/// being created, so a retry wrapper recognises the refusal by matching the sentence.
///
/// The rows differ along what the rule generalises over: which create is duplicated (each
/// of the three this service offers carries an `@idempotencyToken`), and whether the
/// duplicate is one call or two calls racing each other.
///
/// The window the refusal belongs to is the processing of the request that holds the token,
/// so it is held open here through the claim that request takes — and released again —
/// rather than by hoping a call is still running: which answer a duplicate gets must not
/// depend on how loaded the host is.
#[tokio::test]
async fn a_duplicate_of_an_in_flight_idempotent_create_is_refused_with_one_sentence() {
    use crate::idempotent_create_in_flight_window::{hold_token_in_flight, release_token};

    let expected = service_com_amazonaws_b2bi_b2bi::state::CLIENT_TOKEN_IN_FLIGHT;

    for racers in [1usize, 2] {
        let (router, engine) = crate::onboarding_loop::b2bi_router_with_engine();
        let profile_id = given_profile(&router, "in-flight").await;
        let capability_id = given_capability(&router, "in-flight").await;
        let transformer = json!({
            "name": "in-flight-transformer",
            "inputConversion": {
                "fromFormat": "X12",
                "formatOptions": {
                    "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                }
            }
        });

        // (case, operation, the resource type the token is scoped by, the request, minus
        //  the token every row adds)
        let rows: Vec<(&str, &str, &str, Value)> = vec![
            (
                "a profile",
                "CreateProfile",
                "profile",
                json!({
                    "name": "in-flight profile",
                    "phone": "555-555-0199",
                    "businessName": "In Flight Corp",
                    "logging": "DISABLED"
                }),
            ),
            (
                "a partnership",
                "CreatePartnership",
                "partnership",
                json!({
                    "profileId": profile_id.clone(),
                    "name": "in-flight partner",
                    "email": "partner@example.com",
                    "capabilities": [capability_id.clone()]
                }),
            ),
            (
                "a transformer",
                "CreateTransformer",
                "transformer",
                transformer,
            ),
        ];

        for (case, operation, resource_type, request) in rows {
            let case = format!("{case} duplicated by {racers} racer(s)");
            let token = format!("token-for-{operation}-{racers}");
            let with_token = |request: &Value| {
                let mut request = request.clone();
                request["clientToken"] = json!(token);
                request
            };

            // The request that holds the token is in flight: its claim stands, exactly as
            // it does while that request is being processed.
            hold_token_in_flight(&engine, resource_type, &token, &case);

            // Every duplicate that arrives while the claim stands is a racer that lost.
            let answers = match racers {
                1 => vec![call(&router, operation, with_token(&request)).await],
                _ => {
                    let (first, second) = ::tokio::join!(
                        call(&router, operation, with_token(&request)),
                        call(&router, operation, with_token(&request))
                    );
                    vec![first, second]
                }
            };
            for (status, body) in &answers {
                assert_eq!(
                    *status, 409,
                    "case {case:?}: a duplicate of an in-flight create must be refused, got \
                     {status} {body}"
                );
                assert_eq!(
                    error_code(body),
                    "ConflictException",
                    "case {case:?}: expected a ConflictException, got {body}"
                );
                let message = text(body, "message");
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected message {expected:?}, got {message:?}"
                );
                assert!(
                    !message.contains(&token),
                    "case {case:?}: the sentence must not echo the caller's token, got \
                     {message:?}"
                );
            }

            // The fixed sentence is for a duplicate that RACED the original: once the
            // request holding the token has answered, the same token answers with the
            // resource it minted, which is the other half of the recorded idempotency
            // behaviour.
            release_token(&engine, resource_type, &token);
            let (status, created) = call(&router, operation, with_token(&request)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: with no request holding the token the create must be \
                 served, got {status} {created}"
            );
            let (status, replayed) = call(&router, operation, with_token(&request)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeat issued after the create answered must replay it, \
                 got {status} {replayed}"
            );
        }
    }
}

/// RULE: a transformer configured with BOTH the deprecated members and the ones that
/// replace them is refused with `ValidationException` (HTTP 400) and one FIXED sentence,
/// which states the two sets of fields a transformer may be configured with and reports
/// NOTHING of which members this particular request supplied.
///
/// The rows differ along what the rule generalises over: which operation configures the
/// transformer (the create and the update enforce the same rule), which member of each
/// style is mixed, and how many of them the request carries.
#[tokio::test]
async fn mixing_the_two_transformer_configuration_styles_is_refused_with_one_sentence() {
    use service_com_amazonaws_b2bi_b2bi::ops::create_transformer::MIXED_CONFIGURATION_STYLES;

    let router = b2bi_router();
    let transformer_id = given_transformer(&router, "mixed").await;

    let input_conversion = json!({
        "fromFormat": "X12",
        "formatOptions": { "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" } }
    });
    let edi_type = json!({ "x12Details": { "transactionSet": "X12_850", "version": "VERSION_4010" } });

    // (case, the members of each style the request mixes)
    let mixes: Vec<(&str, Value)> = vec![
        (
            "inputConversion beside fileFormat",
            json!({ "inputConversion": input_conversion.clone(), "fileFormat": "JSON" }),
        ),
        (
            "mapping beside mappingTemplate",
            json!({
                "mapping": { "templateLanguage": "JSONATA", "template": "$" },
                "mappingTemplate": "$"
            }),
        ),
        (
            "outputConversion beside ediType",
            json!({
                "outputConversion": {
                    "toFormat": "X12",
                    "formatOptions": {
                        "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                    }
                },
                "ediType": edi_type.clone()
            }),
        ),
        (
            "sampleDocuments beside mappingTemplate",
            json!({
                "sampleDocuments": {
                    "bucketName": "b2bi-samples",
                    "keys": [{ "input": "in/850.edi", "output": "out/850.json" }]
                },
                "mappingTemplate": "$"
            }),
        ),
        (
            "every member of both styles at once",
            json!({
                "inputConversion": input_conversion,
                "mapping": { "templateLanguage": "JSONATA", "template": "$" },
                "fileFormat": "JSON",
                "mappingTemplate": "$",
                "ediType": edi_type
            }),
        ),
    ];

    for (case, mix) in mixes {
        // The same mix, through the create and through the update.
        let mut create = json!({ "name": "mixed-styles" });
        let mut update = json!({ "transformerId": transformer_id.clone() });
        for (member, value) in mix.as_object().expect("the mix is an object") {
            create[member] = value.clone();
            update[member] = value.clone();
        }

        for (operation, request) in [
            ("CreateTransformer", create),
            ("UpdateTransformer", update),
        ] {
            let case = format!("{case} on {operation}");
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            let message = message_of(&body);
            assert_eq!(
                message, MIXED_CONFIGURATION_STYLES,
                "case {case:?}: expected message {MIXED_CONFIGURATION_STYLES:?}, got \
                 {message:?} in {body}"
            );
        }
    }

    // The rule refuses the OVERLAP, so each style on its own is still served.
    let (status, body) = call(
        &router,
        "UpdateTransformer",
        json!({ "transformerId": transformer_id, "name": "only-current-members" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a request in one style only must be served, got {status} {body}"
    );
}

/// RULE: a submitted JSON document that does not parse is refused with the JSON parser's
/// own diagnostic — what the parse stopped ON (the offending character and its character
/// code, the unquoted word where a value belongs, or the end of the input), what belonged
/// there instead, and the line and column it stopped at — with the document reported as
/// REDACTED and no input member named.
///
/// The rows differ along what the rule generalises over: which fault the document carries
/// (a field name that is not quoted, a bare word where a value belongs, a missing comma, a
/// document that ends inside a collection), where in the document it sits (the first line
/// and a later one), and the offending character itself, whose code travels in the
/// sentence.
#[tokio::test]
async fn an_unparsable_json_document_is_refused_with_the_parsers_own_diagnostic() {
    let router = b2bi_router();

    // (case, the submitted document, the diagnosis, the position it is reported at)
    let rows: Vec<(&str, &str, String, (usize, usize))> = vec![
        (
            "an unquoted field name",
            "{ this is not json }",
            "Unexpected character ('t' (code 116)): was expecting double-quote to start \
             field name"
                .to_string(),
            (1, 3),
        ),
        (
            "an unquoted field name on the second line",
            "{\n  total: 1\n}",
            "Unexpected character ('t' (code 116)): was expecting double-quote to \
             start field name"
                .to_string(),
            (2, 3),
        ),
        (
            "a bare word where a value belongs",
            "not a document",
            format!(
                "Unrecognized token 'not': was expecting ({})",
                ::service_common::json_parse_fault::VALUE_CLASSES
            ),
            (1, 1),
        ),
        (
            "a missing comma between members",
            r#"{"interchanges": 1 "more": 2}"#,
            "Unexpected character ('\"' (code 34)): was expecting comma to separate Object \
             entries"
                .to_string(),
            (1, 20),
        ),
        (
            "a document that ends inside an object",
            r#"{"interchanges": ["#,
            "Unexpected end-of-input: expected close marker for Array".to_string(),
            (1, 19),
        ),
    ];

    for (case, document, diagnosis, (line, column)) in rows {
        let (status, body) = call(
            &router,
            "TestConversion",
            json!({
                "source": {
                    "fileFormat": "JSON",
                    "inputFile": { "fileContent": document },
                },
                "target": { "fileFormat": "X12" },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let expected = format!(
            "{diagnosis} at [Source: {}; line: {line}, column: {column}]",
            ::service_common::json_parse_fault::REDACTED_SOURCE
        );
        let message = message_of(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected message {expected:?}, got {message:?} in {body}"
        );
        assert!(
            !message.contains("fileContent") && !message.contains("source"),
            "case {case:?}: the parser's diagnostic names no input member, got {message:?}"
        );
    }
}
