// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `InsightFeedback`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`], the insight builder from [`super::insight_details`]
//! and the refusal assertion from [`super::insights`]; this suite adds no second copy
//! of any of them.
//!
//! The rule under test: a submission records the caller's rating — its thumbs value
//! and the optional text that came with it — on the ONE insight the domain the request
//! names holds under the requested id, replacing whatever rating an earlier submission
//! left there, changes nothing else about that insight, and answers with the status of
//! the submission; an address that resolves to no such insight, because the account
//! holds no such domain or that domain holds no insight under the id, is refused as a
//! resource this service cannot resolve, and a request outside the modelled values —
//! an entity type feedback does not accept, a rating outside the enum, text past the
//! published maximum — is refused before any address is resolved and records nothing.
//!
//! So the assertions span a SET of inputs rather than one call: a table of submissions
//! covering BOTH values of `InsightFeedbackThumbs` against five shapes of feedback
//! text (none at all, the empty string, one line, non-ASCII content and the published
//! maximum length), then the same table resubmitted onto one insight, four addresses
//! that resolve to no insight of the named domain, and seven malformed requests
//! including the length bound and one character past it.
//!
//! The insights themselves are written into the store directly (through
//! [`super::domain_lifecycle::Emulator::store`]): the service RAISES an insight from
//! what it observes of a domain, so no wire call leaves one. The model declares no
//! operation that REPORTS the feedback an insight carries, so the rating is read back
//! from the store the handler wrote it to, while everything a client can see — the
//! answered status, the listing and the detailed view — is asserted over the wire.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    INSIGHT_ENTITY_TYPE_VALUES, INSIGHT_FEEDBACK_ENTITY_TYPE_VALUES,
    INSIGHT_FEEDBACK_THUMBS_VALUES, INSIGHT_RESPONSE_STATUS_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    InsightDetailField, InsightFeedbackRecord,
};

use super::domain_lifecycle::{
    create_baseline, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};
use super::insight_details::{raise_on_domain, record};
use super::insights::refused;

/// The longest `FeedbackText` a submission may carry: the member is
/// `@length(min: 0, max: 1000)`, as `smithy_query(query="operation",
/// operation="InsightFeedback")` reports it — "maximum length is 1000 characters".
const MAX_FEEDBACK_TEXT: usize = 1000;

/// The entity type a submission names its domain with: the ONLY value
/// `InsightFeedbackEntityType` admits (asserted against the model's own catalog
/// below).
const FEEDBACK_ENTITY_TYPE: &str = "DomainName";

/// One row of the submission table: what it is called, the insight it rates, the
/// rating it gives and the text it carries.
struct Row {
    label: &'static str,
    id_suffix: &'static str,
    thumbs: &'static str,
    text: Option<String>,
}

/// The `GUID` an insight of this suite is addressed by: `InsightId` is
/// `@length(36, 36)` with pattern
/// `^\p{XDigit}{8}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{4}-\p{XDigit}{12}$`, so an
/// id is a hyphenated UUID whose last field is what tells it apart.
fn guid(suffix: &str) -> String {
    format!("3a7f41c8-9d26-4b5e-8f17-{suffix}")
}

/// The submissions every case below is asserted over.
///
/// The rows differ along what the rule generalises over: the rating given (between
/// them every value of `InsightFeedbackThumbs`, the catalog the model declares, as
/// `ops::types`' `INSIGHT_FEEDBACK_THUMBS_VALUES`, asserted below) and the text that
/// came with it — none at all, the empty string (the published minimum length), one
/// line, content outside ASCII, and text exactly at the published maximum.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a thumbs up with no text",
            id_suffix: "000000000001",
            thumbs: "Up",
            text: None,
        },
        Row {
            label: "a thumbs down with no text",
            id_suffix: "000000000002",
            thumbs: "Down",
            text: None,
        },
        Row {
            label: "a thumbs up with empty text",
            id_suffix: "000000000003",
            thumbs: "Up",
            text: Some(String::new()),
        },
        Row {
            label: "a thumbs down with one line of text",
            id_suffix: "000000000004",
            thumbs: "Down",
            text: Some("This recommendation does not apply to this domain.".to_string()),
        },
        Row {
            label: "a thumbs down with text outside ASCII",
            id_suffix: "000000000005",
            thumbs: "Down",
            text: Some("Trop de partitions par nœud — 提案は役に立ちました".to_string()),
        },
        Row {
            label: "a thumbs up with text at the published maximum",
            id_suffix: "000000000006",
            thumbs: "Up",
            text: Some("y".repeat(MAX_FEEDBACK_TEXT)),
        },
    ]
}

impl Row {
    /// The id of the insight this row rates.
    fn insight_id(&self) -> String {
        guid(self.id_suffix)
    }
}

/// An `InsightFeedbackEntity` naming the domain `name`.
fn domain_entity(name: &str) -> Value {
    json!({"Type": FEEDBACK_ENTITY_TYPE, "Value": name})
}

/// `POST /2021-01-01/opensearch/insight-feedback` with `body`.
async fn feedback(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/insight-feedback"),
            Some(body),
        )
        .await
}

/// The request body of a submission: the members a caller sends, with `FeedbackText`
/// present only where the submission carries text.
fn submission(entity: Value, insight_id: &str, thumbs: &str, text: Option<&str>) -> Value {
    let mut body = json!({"Entity": entity, "InsightId": insight_id, "Thumbs": thumbs});
    if let Some(text) = text {
        body["FeedbackText"] = json!(text);
    }
    body
}

/// Submit `row`'s rating of its own insight to the domain `name`.
async fn submit_row(emulator: &Emulator, name: &str, row: &Row) -> Answer {
    feedback(
        emulator,
        submission(
            domain_entity(name),
            &row.insight_id(),
            row.thumbs,
            row.text.as_deref(),
        ),
    )
    .await
}

/// The `Status` of an accepted submission, asserted to be one of the modelled values.
fn status(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let reported = answer.body["Status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a Status, got {:?}", answer.body))
        .to_owned();
    // The status catalog is the model's own enum, as generated into `ops::types`.
    assert!(
        INSIGHT_RESPONSE_STATUS_VALUES.contains(&reported.as_str()),
        "case {case}: expected a modelled status, got {reported:?}"
    );
    reported
}

/// The feedback the store holds on the insight `insight_id` of the domain `name` —
/// the record the handler wrote, read the way a handler reaches it.
fn stored_feedback(emulator: &Emulator, name: &str, insight_id: &str) -> Option<InsightFeedbackRecord> {
    let store = emulator.store();
    let guard = store.lock();
    guard
        .domains
        .get(name)
        .unwrap_or_else(|| panic!("case stored_feedback({name:?}): the domain must exist"))
        .insights
        .get(insight_id)
        .unwrap_or_else(|| {
            panic!("case stored_feedback({insight_id:?}): the insight must have been raised")
        })
        .feedback
        .clone()
}

/// The rating `row` submits, as the store must hold it afterwards, asserted against
/// the record found there.
fn assert_recorded(
    emulator: &Emulator,
    name: &str,
    row: &Row,
    before: SystemTime,
    after: SystemTime,
) {
    let held = stored_feedback(emulator, name, &row.insight_id()).unwrap_or_else(|| {
        panic!(
            "case {}: expected the rating to be recorded on the insight",
            row.label
        )
    });
    assert_eq!(
        held.thumbs, row.thumbs,
        "case {}: expected the submitted rating, got {:?}",
        row.label, held.thumbs
    );
    assert_eq!(
        held.feedback_text.as_deref(),
        row.text.as_deref(),
        "case {}: expected the submitted text, got {:?}",
        row.label,
        held.feedback_text
    );
    assert!(
        held.submitted_at >= before && held.submitted_at <= after,
        "case {}: expected the submission instant to be the instant of the call, got {:?}",
        row.label,
        held.submitted_at
    );
}

/// A domain holding one insight per [`rows`] row, each rated by nobody yet.
async fn seeded(emulator: &Emulator, name: &str) -> Vec<Row> {
    create_baseline(emulator, name).await;
    let raised = SystemTime::now();
    for row in rows() {
        raise_on_domain(
            emulator,
            name,
            record(&row.insight_id(), raised, Vec::new()),
        );
    }
    rows()
}

/// `POST /2021-01-01/opensearch/insights` for the domain `name`.
async fn list(emulator: &Emulator, name: &str) -> Value {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/insights"),
            Some(json!({"Entity": domain_entity(name)})),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case list({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Insights"].clone()
}

/// `POST /2021-01-01/opensearch/insight-details` for one insight of the domain `name`.
async fn detail_fields(emulator: &Emulator, name: &str, insight_id: &str) -> Value {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/insight-details"),
            Some(json!({"Entity": domain_entity(name), "InsightId": insight_id})),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case detail_fields({insight_id:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Fields"].clone()
}

#[tokio::test]
async fn every_rating_and_text_is_recorded_on_the_insight_it_rates() {
    let emulator = Emulator::new();
    let table = seeded(&emulator, "insight-feedback").await;

    // The ratings this table submits are the model's own enum, as generated into
    // `ops::types`: a row naming a value outside it would assert nothing about this
    // service's wire surface, and a table missing one would leave a rating untested.
    for value in INSIGHT_FEEDBACK_THUMBS_VALUES {
        assert!(
            table.iter().any(|row| &row.thumbs == value),
            "case the thumbs catalog: expected a row rating an insight {value:?}"
        );
    }
    // Feedback accepts exactly one entity type, which is what every row names.
    assert_eq!(
        INSIGHT_FEEDBACK_ENTITY_TYPE_VALUES,
        [FEEDBACK_ENTITY_TYPE],
        "case the entity catalog: expected feedback to accept one entity type, got {:?}",
        INSIGHT_FEEDBACK_ENTITY_TYPE_VALUES
    );

    for row in &table {
        assert!(
            INSIGHT_FEEDBACK_THUMBS_VALUES.contains(&row.thumbs),
            "case {}: the row must rate with a modelled value, got {:?}",
            row.label,
            row.thumbs
        );
        let before = SystemTime::now();
        let answer = submit_row(&emulator, "insight-feedback", row).await;
        let after = SystemTime::now();
        assert_eq!(
            status(&answer, row.label),
            "SUCCESS",
            "case {}: expected the submission to succeed, got {:?}",
            row.label,
            answer.body
        );
        assert_recorded(&emulator, "insight-feedback", row, before, after);
    }

    // Each submission rated ITS insight and no other: the whole table is held side by
    // side once every row has been submitted.
    for row in &table {
        let held = stored_feedback(&emulator, "insight-feedback", &row.insight_id())
            .unwrap_or_else(|| panic!("case {}: expected the rating to survive", row.label));
        assert_eq!(
            held.thumbs, row.thumbs,
            "case {}: expected each insight to keep its own rating, got {:?}",
            row.label, held.thumbs
        );
        assert_eq!(
            held.feedback_text.as_deref(),
            row.text.as_deref(),
            "case {}: expected each insight to keep its own text, got {:?}",
            row.label,
            held.feedback_text
        );
    }
}

#[tokio::test]
async fn a_later_submission_replaces_the_rating_an_earlier_one_left() {
    let emulator = Emulator::new();
    create_baseline(&emulator, "insight-reverdict").await;
    let rated = guid("0000000000aa");
    raise_on_domain(
        &emulator,
        "insight-reverdict",
        record(&rated, SystemTime::now(), Vec::new()),
    );

    // One caller holds ONE verdict on an insight, so the whole table submitted onto the
    // SAME insight leaves exactly the last row's rating — never a tally of all of them.
    for row in rows() {
        let case = format!("resubmitted {}", row.label);
        let answer = feedback(
            &emulator,
            submission(
                domain_entity("insight-reverdict"),
                &rated,
                row.thumbs,
                row.text.as_deref(),
            ),
        )
        .await;
        assert_eq!(
            status(&answer, &case),
            "SUCCESS",
            "case {case}: expected the resubmission to succeed, got {:?}",
            answer.body
        );
        let held = stored_feedback(&emulator, "insight-reverdict", &rated)
            .unwrap_or_else(|| panic!("case {case}: expected a recorded rating"));
        assert_eq!(
            held.thumbs, row.thumbs,
            "case {case}: expected the latest rating, got {:?}",
            held.thumbs
        );
        assert_eq!(
            held.feedback_text.as_deref(),
            row.text.as_deref(),
            "case {case}: expected the latest text, got {:?}",
            held.feedback_text
        );
    }
}

#[tokio::test]
async fn rating_an_insight_changes_nothing_else_about_it() {
    let emulator = Emulator::new();
    create_baseline(&emulator, "insight-untouched").await;
    let rated = guid("0000000000bb");
    raise_on_domain(
        &emulator,
        "insight-untouched",
        record(
            &rated,
            SystemTime::now(),
            vec![InsightDetailField {
                name: "Summary".to_string(),
                field_type: "text".to_string(),
                value: "Shard count on the hot data nodes is above the ceiling.".to_string(),
                html_value: None,
            }],
        ),
    );

    let listing_before = list(&emulator, "insight-untouched").await;
    let details_before = detail_fields(&emulator, "insight-untouched", &rated).await;

    for row in rows() {
        let case = format!("unchanged after {}", row.label);
        let answer = feedback(
            &emulator,
            submission(
                domain_entity("insight-untouched"),
                &rated,
                row.thumbs,
                row.text.as_deref(),
            ),
        )
        .await;
        assert_eq!(
            status(&answer, &case),
            "SUCCESS",
            "case {case}: expected the submission to succeed, got {:?}",
            answer.body
        );
        // The rating is feedback ABOUT the insight: every member the service reports of
        // it — including the instant it was last changed — reads exactly as before.
        assert_eq!(
            list(&emulator, "insight-untouched").await,
            listing_before,
            "case {case}: expected the listing to be unchanged, got {:?}",
            list(&emulator, "insight-untouched").await
        );
        assert_eq!(
            detail_fields(&emulator, "insight-untouched", &rated).await,
            details_before,
            "case {case}: expected the detailed view to be unchanged, got {:?}",
            detail_fields(&emulator, "insight-untouched", &rated).await
        );
    }
}

#[tokio::test]
async fn a_submission_naming_no_insight_of_the_domain_is_refused() {
    let emulator = Emulator::new();
    let table = seeded(&emulator, "insight-missing").await;
    create_baseline(&emulator, "insight-sibling").await;
    let elsewhere = guid("0000000000cc");
    raise_on_domain(
        &emulator,
        "insight-sibling",
        record(&elsewhere, SystemTime::now(), Vec::new()),
    );
    let account_wide = guid("0000000000dd");
    {
        let store = emulator.store();
        let mut guard = store.lock();
        let held = record(&account_wide, SystemTime::now(), Vec::new());
        guard.account_insights.insert(account_wide.clone(), held);
    }
    let rated = table[0].insight_id();

    // An address resolves only where the domain holds the insight: a domain this
    // account never created, an id the domain holds no insight under, an insight held
    // by a SIBLING domain, and one held for the account itself rather than for any
    // domain.
    for (entity, insight_id, case) in [
        (
            domain_entity("absent-domain"),
            rated.clone(),
            "a domain that was never created",
        ),
        (
            domain_entity("insight-missing"),
            guid("00000000ffff"),
            "an id the domain holds no insight under",
        ),
        (
            domain_entity("insight-missing"),
            elsewhere.clone(),
            "an insight of a sibling domain",
        ),
        (
            domain_entity("insight-missing"),
            account_wide.clone(),
            "an insight held for the account itself",
        ),
    ] {
        let answer = feedback(
            &emulator,
            submission(entity, &insight_id, "Up", Some("Not applicable.")),
        )
        .await;
        refused(
            &answer,
            case,
            RECORDED_NOT_FOUND_STATUS,
            "ResourceNotFoundException",
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some("Resource not found"),
            "case {case}: expected the unresolvable address to be reported as such, got {:?}",
            answer.body
        );
    }

    // Nothing was recorded by any of them: the insight the first case named, the one
    // the sibling domain holds and the one the account holds are all still unrated.
    assert!(
        stored_feedback(&emulator, "insight-missing", &rated).is_none(),
        "case a refused submission: expected no rating on the insight it named"
    );
    assert!(
        stored_feedback(&emulator, "insight-sibling", &elsewhere).is_none(),
        "case a refused submission: expected no rating on the sibling domain's insight"
    );
    let store = emulator.store();
    let held = store
        .lock()
        .account_insights
        .get(&account_wide)
        .and_then(|insight| insight.feedback.clone());
    assert!(
        held.is_none(),
        "case a refused submission: expected no rating on the account's own insight, got \
         {held:?}"
    );
}

#[tokio::test]
async fn a_submission_outside_the_modelled_values_is_refused_and_records_nothing() {
    let emulator = Emulator::new();
    let table = seeded(&emulator, "insight-malformed").await;
    let rated = table[0].insight_id();
    let past_the_bound = "y".repeat(MAX_FEEDBACK_TEXT + 1);

    // `Account` is a value `InsightEntityType` admits and
    // `InsightFeedbackEntityType` does not, so feedback refuses the entity a listing
    // would have resolved.
    assert!(
        INSIGHT_ENTITY_TYPE_VALUES.contains(&"Account")
            && !INSIGHT_FEEDBACK_ENTITY_TYPE_VALUES.contains(&"Account"),
        "case the entity catalogs: expected Account to be a listing entity alone"
    );

    for (entity, thumbs, text, case) in [
        (
            domain_entity("insight-malformed"),
            "UP",
            None,
            "a rating in another casing",
        ),
        (
            domain_entity("insight-malformed"),
            "up",
            None,
            "a rating in lower case",
        ),
        (
            domain_entity("insight-malformed"),
            "Maybe",
            None,
            "a rating outside the enum",
        ),
        (
            domain_entity("insight-malformed"),
            "",
            None,
            "no rating at all",
        ),
        (
            json!({"Type": "Account", "Value": "insight-malformed"}),
            "Up",
            None,
            "an entity type feedback does not accept",
        ),
        (
            domain_entity("insight-malformed"),
            "Up",
            Some(past_the_bound.as_str()),
            "text one character past the published maximum",
        ),
        // A request that cannot be served as asked is refused whether or not the
        // resource it names exists, so the malformed request is refused and not the
        // missing domain.
        (
            domain_entity("absent-domain"),
            "Maybe",
            None,
            "a rating outside the enum against a domain that was never created",
        ),
    ] {
        let answer = feedback(&emulator, submission(entity, &rated, thumbs, text)).await;
        refused(&answer, case, 400, "ValidationException");
        assert!(
            stored_feedback(&emulator, "insight-malformed", &rated).is_none(),
            "case {case}: expected a refused submission to record nothing"
        );
    }

    // The same text one character shorter is exactly at the bound, and is recorded:
    // the refusal above is the bound's, not a refusal of long text as such.
    let answer = feedback(
        &emulator,
        submission(
            domain_entity("insight-malformed"),
            &rated,
            "Up",
            Some(&past_the_bound[..MAX_FEEDBACK_TEXT]),
        ),
    )
    .await;
    assert_eq!(
        status(&answer, "text at the published maximum"),
        "SUCCESS",
        "case text at the published maximum: expected the submission to succeed, got {:?}",
        answer.body
    );
}
