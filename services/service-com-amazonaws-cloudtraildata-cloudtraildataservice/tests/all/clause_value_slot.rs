// SPDX-License-Identifier: CC0-1.0

//! The VALUE half and the member PATH of every clause a modelled-constraint refusal of
//! `PutAuditEvents` states.
//!
//! One rule: each clause of the refusal envelope states, in its value slot, the value the
//! request carried at the member the validating walk names — a COLLECTION-typed member's
//! own elements rendered in request order, a scalar member's value verbatim, never a
//! summary of a collection's size — with an element of a list named at its 1-BASED
//! position, and the envelope counting one clause per violation found.
//!
//! The axes here are the ones the sibling suites leave open: WHICH position in the batch
//! the offending element sat at (`constraint_refusals` asserts the first element only),
//! how MANY violations one request carried, and a request that breaks the list's own
//! `@length` and an element's constraint together — so one message holds both renderings
//! of the value slot at once.
//!
//! The clause builders, the declared maximum and the client are the suites that own them,
//! reached rather than restated.

use crate::constraint_refusals::{
    at_most_requirement, batch_length_clause, element_id_clause, message_of, pattern_requirement,
    GOOD_EVENT, ID_MAX_LENGTH, ID_PATTERN, MAX_BATCH,
};
use crate::put_audit_events::{channel_arn, ev, Ev, Harness};
use service_com_amazonaws_cloudtraildata_cloudtraildataservice::state::Channel;

/// The envelope read back: the violation count it states, and its clauses.
fn envelope(message: &str) -> (usize, Vec<String>) {
    let (count, clauses) = message
        .split_once(" validation error detected: ")
        .or_else(|| message.split_once(" validation errors detected: "))
        .unwrap_or_else(|| panic!("a refusal envelope, got {message:?}"));
    let count: usize = count
        .parse()
        .unwrap_or_else(|_| panic!("a violation count, got {count:?} in {message:?}"));
    (
        count,
        clauses.split("; ").map(str::to_string).collect::<Vec<_>>(),
    )
}

/// The clause the list member's own `@length` maximum states: the batch the request
/// carried, quoted whole, built from the template the sibling suite owns.
fn batch_clause(events: &[Ev]) -> String {
    batch_length_clause(events, &at_most_requirement(MAX_BATCH))
}

/// A batch of `count` well-formed entries, with the entry at each 1-based position named
/// in `broken` carrying that id instead.
fn batch_breaking(count: usize, broken: &[(usize, &str)]) -> Vec<Ev> {
    (1..=count)
        .map(|position| {
            match broken.iter().find(|(offending, _)| *offending == position) {
                Some((_, id)) => ev(*id, GOOD_EVENT),
                None => ev(format!("evt-{position}"), GOOD_EVENT),
            }
        })
        .collect()
}

/// Every clause of a refusal names the member the walk found the violation at and quotes
/// the value the request carried there: a scalar verbatim at its element's 1-based
/// position, the batch itself for the list member's own bound, and a count summary never.
///
/// Spans where in the batch the offending element sat, how large the batch was, which
/// constraint class the element broke, how many violations one request carried, and a
/// request breaking the list bound and an element constraint together.
#[tokio::test]
async fn every_clause_quotes_the_value_submitted_at_the_member_it_names() {
    let harness = Harness::new();
    let arn = channel_arn("1b2c3d4e-5f60-4712-8823-94a5b6c7d8e9");
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

    let over_long_id = "i".repeat(ID_MAX_LENGTH + 1);
    let past_the_bound = batch_breaking(MAX_BATCH + 1, &[(7, "bad id!")]);
    let id_pattern = pattern_requirement(ID_PATTERN);

    // (case, the batch submitted, the clauses expected, in no particular order)
    let rows: Vec<(&str, Vec<Ev>, Vec<String>)> = vec![
        (
            "the only element breaks the id expression",
            batch_breaking(1, &[(1, "bad id!")]),
            vec![element_id_clause(1, "bad id!", &id_pattern)],
        ),
        (
            "the second of two elements breaks the id expression",
            batch_breaking(2, &[(2, "bad id!")]),
            vec![element_id_clause(2, "bad id!", &id_pattern)],
        ),
        (
            "the last of five elements breaks the id expression",
            batch_breaking(5, &[(5, "bad.id")]),
            vec![element_id_clause(5, "bad.id", &id_pattern)],
        ),
        (
            "the last element of a batch at the declared maximum breaks the id expression",
            batch_breaking(MAX_BATCH, &[(MAX_BATCH, "bad id!")]),
            vec![element_id_clause(MAX_BATCH, "bad id!", &id_pattern)],
        ),
        (
            "the third of four elements carries an id past its declared maximum",
            batch_breaking(4, &[(3, over_long_id.as_str())]),
            vec![element_id_clause(
                3,
                &over_long_id,
                &at_most_requirement(ID_MAX_LENGTH),
            )],
        ),
        (
            "two elements of five break the id expression",
            batch_breaking(5, &[(2, "bad id!"), (4, "worse id!")]),
            vec![
                element_id_clause(2, "bad id!", &id_pattern),
                element_id_clause(4, "worse id!", &id_pattern),
            ],
        ),
        (
            "a batch past the declared maximum one of whose elements breaks the id \
             expression",
            past_the_bound.clone(),
            vec![
                batch_clause(&past_the_bound),
                element_id_clause(7, "bad id!", &id_pattern),
            ],
        ),
    ];

    for (case, events, expected) in &rows {
        let reply = harness.put(&arn, None, events).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} with body {}",
            reply.status,
            reply.body
        );
        let stated = message_of(&reply);
        let (count, clauses) = envelope(&stated);
        assert_eq!(
            count,
            clauses.len(),
            "case {case:?}: the envelope counts {count} violation(s) over {} clause(s), in \
             {stated:?}",
            clauses.len()
        );
        for clause in expected {
            assert!(
                clauses.iter().any(|stated| stated == clause),
                "case {case:?}: expected the clause {clause:?}, got {clauses:?}"
            );
        }
        assert_eq!(
            clauses.len(),
            expected.len(),
            "case {case:?}: expected {} clause(s), got {clauses:?}",
            expected.len()
        );
        // A collection has a count summary in the validating walk — `<n> items` for a
        // list, `<n> entries` for a map — and no caller ever sent one, so no clause of any
        // row may state either in place of the value.
        for summary in [" items'", " entries'"] {
            assert!(
                !stated.contains(summary),
                "case {case:?}: a clause states a count summary ({summary:?}) in place of \
                 the value it rejected, got {stated:?}"
            );
        }
    }
}
