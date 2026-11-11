// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that an index update is answered only in the vocabulary its model
//! declares, driven over the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns.
//!
//! The RULE under test: an operation this emulator serves answers every request with one
//! of the answers its model declares — the modelled success status carrying an output
//! member the model allows, or one of the errors the operation lists, at the HTTP status
//! this service answers that error on. No request is answered with a code outside that
//! set, the
//! 501 of an operation nobody has written yet included: a client branches on the error
//! code, so a code the model does not declare for the operation cannot be told apart from
//! a service fault, and an authorization refusal in particular has to arrive as the
//! modelled 403.
//!
//! The sibling suites assert each refusal's own wording and the IAM decision behind the
//! 403 ([`super::index_authorization`], [`super::index_schema_updates`]); this suite
//! asserts the other half — that whatever answer any of those paths reaches belongs to the
//! operation's modelled set, at its modelled status — over a table with one row per class
//! of answer the operation can reach, the same request both refused and served. Its
//! fixture (a domain, an index on it, an assumed-role session and the policies in force on
//! it) comes from [`super::index_authorization`] rather than growing a second copy of it.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::INDEX_STATUS_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::INDEX_STATUS_UPDATED;

use super::domain_lifecycle::{
    error_code, string_member, Answer, DEFAULT_ACCESS_KEY_ID, PREFIX, RECORDED_NOT_FOUND_STATUS,
};
use super::index_authorization::{
    added_property, allow, assert_refused, created_schema, Fixture, Refusal, DOMAIN, INDEX,
    SESSION_KEY_ID,
};
use super::index_registry::get as get_index;

/// Every error `UpdateIndex` declares, with the HTTP status the model gives it: the whole
/// set of codes a client may be refused with
/// (`smithy_query(query="operation",operation="UpdateIndex")` — each `errors[]` entry's
/// `shape` and `httpError`).
const MODELLED_ERRORS: [(&str, u16); 7] = [
    ("AccessDeniedException", 403),
    ("DependencyFailureException", 424),
    ("DisabledOperationException", 409),
    ("InternalException", 500),
    ("ResourceNotFoundException", 409),
    ("ThrottlingException", 429),
    ("ValidationException", 400),
];

/// The status the model gives a served `UpdateIndex` (its `http.code`).
const SUCCESS_STATUS: u16 = 200;

/// The mapping property the update under test adds — absent from [`created_schema`], so
/// its presence afterwards is what tells a served update from a refused one.
const ADDED_PROPERTY: &str = "summary";

/// One row: the request to make, and the answer the model declares for it.
struct Row {
    case: &'static str,
    /// The credential the request is signed with: the long-term key of the account's own
    /// identity, or the assumed-role session the fixture records.
    access_key_id: &'static str,
    domain: &'static str,
    index: &'static str,
    schema: Value,
    /// The modelled error the row must be refused with, or `None` where the operation
    /// serves it.
    refused_with: Option<&'static str>,
    /// The HTTP status the refusal is answered on where that is NOT the status the model
    /// gives the error the row names — the recorded wire answer, which no modelled
    /// annotation can express.
    refused_on: Option<u16>,
}

/// One row per class of answer `UpdateIndex` can reach: the authorization refusal, the
/// two missing-resource refusals, the three malformed-request refusals and the served
/// update. The last two rows carry the SAME document — the one refused for the
/// under-privileged principal is the one the permitted caller lands.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a principal whose policies do not allow this action",
            access_key_id: SESSION_KEY_ID,
            domain: DOMAIN,
            index: INDEX,
            schema: added_property(ADDED_PROPERTY, "text"),
            refused_with: Some("AccessDeniedException"),
            refused_on: None,
        },
        Row {
            case: "a domain the store does not hold",
            access_key_id: DEFAULT_ACCESS_KEY_ID,
            domain: "search-elsewhere",
            index: INDEX,
            schema: added_property(ADDED_PROPERTY, "text"),
            refused_with: Some("ResourceNotFoundException"),
            refused_on: Some(RECORDED_NOT_FOUND_STATUS),
        },
        Row {
            case: "a name that names no index of a domain the store does hold",
            access_key_id: DEFAULT_ACCESS_KEY_ID,
            domain: DOMAIN,
            index: "catalog-absent",
            schema: added_property(ADDED_PROPERTY, "text"),
            refused_with: Some("ResourceNotFoundException"),
            refused_on: Some(RECORDED_NOT_FOUND_STATUS),
        },
        Row {
            case: "an index name no index could carry",
            access_key_id: DEFAULT_ACCESS_KEY_ID,
            domain: DOMAIN,
            // A leading underscore is one of the shapes `IndexName`'s modelled pattern
            // refuses (`smithy_query(query="operation",operation="UpdateIndex")`).
            index: "_catalog",
            schema: added_property(ADDED_PROPERTY, "text"),
            refused_with: Some("ValidationException"),
            refused_on: None,
        },
        Row {
            case: "a schema that is not a set of sections",
            access_key_id: DEFAULT_ACCESS_KEY_ID,
            domain: DOMAIN,
            index: INDEX,
            schema: json!(7),
            refused_with: Some("ValidationException"),
            refused_on: None,
        },
        Row {
            case: "a schema re-typing a field the index already maps",
            access_key_id: DEFAULT_ACCESS_KEY_ID,
            domain: DOMAIN,
            index: INDEX,
            schema: added_property("title", "keyword"),
            refused_with: Some("ValidationException"),
            refused_on: None,
        },
        Row {
            case: "the same document from a principal that may update the index",
            access_key_id: DEFAULT_ACCESS_KEY_ID,
            domain: DOMAIN,
            index: INDEX,
            schema: added_property(ADDED_PROPERTY, "text"),
            refused_with: None,
            refused_on: None,
        },
    ]
}

/// `UpdateIndex` against `domain`/`index`, signed with `access_key_id`.
async fn update_as(
    fixture: &Fixture,
    access_key_id: &str,
    domain: &str,
    index: &str,
    schema: &Value,
) -> Answer {
    fixture
        .emulator
        .call_as(
            access_key_id,
            "PUT",
            &format!("{PREFIX}/opensearch/domain/{domain}/index/{index}"),
            Some(json!({ "IndexSchema": schema })),
        )
        .await
}

/// Assert `answer` is an answer the model declares for `UpdateIndex`: `expected` names the
/// error code the row must be refused with, or is `None` for the row the operation serves;
/// `on` names the status the refusal is answered on where that is not the model's own.
fn assert_modelled_answer(answer: &Answer, case: &str, expected: Option<&str>, on: Option<u16>) {
    let Some(code) = expected else {
        assert_eq!(
            answer.status, SUCCESS_STATUS,
            "case {case:?}: expected the modelled success status {SUCCESS_STATUS}, got {} with \
             {:?}",
            answer.status, answer.body
        );
        let status = string_member(&answer.body, "Status", case);
        assert!(
            INDEX_STATUS_VALUES.contains(&status.as_str()),
            "case {case:?}: expected one of the modelled {INDEX_STATUS_VALUES:?}, got {status:?}"
        );
        return;
    };
    let answered = error_code(answer);
    let Some((_, modelled_status)) = MODELLED_ERRORS
        .iter()
        .find(|(declared, _)| *declared == answered)
    else {
        panic!(
            "case {case:?}: expected one of the errors the model declares for this operation \
             {MODELLED_ERRORS:?}, got {answered:?} with {:?}",
            answer.body
        );
    };
    assert_eq!(
        answered, code,
        "case {case:?}: expected the modelled {code}, got {answered:?} with {:?}",
        answer.body
    );
    assert_eq!(
        answer.status,
        on.unwrap_or(*modelled_status),
        "case {case:?}: expected the status this service answers {answered} on, got {} with {:?}",
        answer.status,
        answer.body
    );
}

#[tokio::test]
async fn every_update_answer_is_one_the_model_declares_for_the_operation() {
    for row in rows() {
        let fixture = Fixture::new().await;
        // The least-privilege shape: the session's role may read the domain and its
        // indexes and nothing else, so a request it signs names no grant of this action
        // while the account's own identity is bounded by no identity policy at all.
        fixture.given_role_policy(&allow(
            json!(["es:DescribeDomain", "es:GetIndex"]),
            json!([
                fixture.domain_arn.clone(),
                format!("{}/*", fixture.domain_arn)
            ]),
        ));

        let answer = update_as(
            &fixture,
            row.access_key_id,
            row.domain,
            row.index,
            &row.schema,
        )
        .await;
        assert_modelled_answer(&answer, row.case, row.refused_with, row.refused_on);

        // The authorization refusal is IAM's own: it names the principal, the action it
        // asked about and the resource the request addressed.
        if row.refused_with == Some("AccessDeniedException") {
            assert_refused(
                &answer,
                row.case,
                &fixture.session_arn(),
                "es:UpdateIndex",
                &fixture.domain_arn,
                Refusal::DefaultDeny,
            );
        }

        // A refused call performs nothing and a served one is the call that lands the
        // change, so the index reads back as the create left it on every refused row and
        // carries the new property on the served one.
        let read = get_index(&fixture.emulator, DOMAIN, INDEX).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: the fixture index must stay readable, got {} with {:?}",
            row.case, read.status, read.body
        );
        let stored = read.body["IndexSchema"].clone();
        match row.refused_with {
            Some(_) => assert_eq!(
                stored,
                created_schema(),
                "case {:?}: a refused update must leave the stored schema untouched, got \
                 {stored:?}",
                row.case
            ),
            None => {
                assert_eq!(
                    string_member(&answer.body, "Status", row.case),
                    INDEX_STATUS_UPDATED,
                    "case {:?}: a served update reports the status of an update",
                    row.case
                );
                assert!(
                    !stored["mappings"]["properties"][ADDED_PROPERTY].is_null(),
                    "case {:?}: a served update must have added {ADDED_PROPERTY:?}, got \
                     {stored:?}",
                    row.case
                );
            }
        }
    }
}
