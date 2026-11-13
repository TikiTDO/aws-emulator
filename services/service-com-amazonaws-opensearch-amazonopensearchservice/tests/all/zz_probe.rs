// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the value set an enumerated member's refusal states when that
//! set is too large to spell out in an expectation: the instance types
//! `OpenSearchPartitionInstanceType` declares
//! (`smithy_query(query="shape",shape="OpenSearchPartitionInstanceType")` — over a
//! hundred values).
//!
//! The sibling suite [`super::constraint_refusal_messages`] asserts the whole message for
//! the enums small enough to write out; this one asserts the same rule over the LARGEST
//! enum of this model by reading the set back out of the clause: it is the member's whole
//! declared set, stated in the order this front door walks its own hash table over those
//! values, in the envelope and clause every other constraint refusal of this service
//! states.

use service_common::validation_message::enum_value_bucket;

use super::domain_lifecycle::{error_code, Emulator, PREFIX};

/// A value no instance type of the model's enumerated type spells.
const NOT_AN_INSTANCE_TYPE: &str = "not.an.instance";

/// Instance types the model declares, spread across the families it carries, so the set
/// read back is checked for membership and not only for order.
///
/// From the model (`smithy_query(query="shape",shape="OpenSearchPartitionInstanceType")`),
/// and deliberately not the one the issue that prompted this suite cited: the first
/// family it declares, a current one, a burstable one and the warm tier's.
const DECLARED_INSTANCE_TYPES: [&str; 4] = [
    "m3.medium.search",
    "or1.2xlarge.search",
    "t3.small.search",
    "ultrawarm1.large.search",
];

/// The value set a clause states, read back out of `message`.
fn stated_set(message: &str, case: &str) -> Vec<String> {
    let opened = message
        .split_once("Member must satisfy enum value set: [")
        .unwrap_or_else(|| panic!("case {case:?}: expected an enum value set, got {message:?}"))
        .1;
    let set = opened
        .strip_suffix(']')
        .unwrap_or_else(|| panic!("case {case:?}: expected a closed value set, got {message:?}"));
    set.split(", ").map(str::to_owned).collect()
}

/// RULE: a value outside an enumerated member's declared set is refused with the standard
/// constraint report, over that member's WHOLE declared set, stated in the order this
/// front door walks its own hash table over those values.
#[tokio::test]
async fn a_large_enum_states_its_whole_set_in_the_order_the_front_door_walks_it() {
    let emulator = Emulator::new();

    // (case, uri) — the member travels as a URI LABEL on one operation and as a query
    // parameter on the other, so the rule is asserted over both bindings of one set.
    let rows = [
        (
            "an instance type bound to a URI label",
            format!(
                "{PREFIX}/opensearch/instanceTypeLimits/OpenSearch_2.11/{NOT_AN_INSTANCE_TYPE}"
            ),
        ),
        (
            "the same set behind another operation's label",
            format!(
                "{PREFIX}/opensearch/instanceTypeLimits/Elasticsearch_7.10/\
                 {NOT_AN_INSTANCE_TYPE}"
            ),
        ),
    ];

    for (case, uri) in rows {
        let answer = emulator.call("GET", &uri, None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        let message = answer.body["message"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a refusal message, got {:?}",
                    answer.body
                )
            })
            .to_owned();
        let opening = format!(
            "1 validation error detected: Value '{NOT_AN_INSTANCE_TYPE}' at 'instanceType' \
             failed to satisfy constraint: Member must satisfy enum value set: ["
        );
        assert!(
            message.starts_with(&opening),
            "case {case:?}: expected the clause to open {opening:?}, got {message:?}"
        );
        let stated = stated_set(&message, case);
        // The ORDER is the hash table's: each value sits in the bucket its own text hashes
        // to ([`enum_value_bucket`], the rule's one home), and the set is listed bucket by
        // bucket — so the buckets of the stated set never decrease. Asserted as that
        // property rather than against a rebuilt list, because two values of a set this
        // large share a bucket and the chain they are read off is the front door's.
        let buckets: Vec<u32> = stated
            .iter()
            .map(|value| enum_value_bucket(value))
            .collect();
        assert!(
            buckets.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case:?}: expected the set listed bucket by bucket, got {:?}",
            stated.iter().zip(&buckets).collect::<Vec<_>>()
        );
        // And that order is NOT the sort, which a set listed alphabetically would also
        // satisfy above were the two the same.
        let mut sorted = stated.clone();
        sorted.sort_unstable();
        assert_ne!(
            stated, sorted,
            "case {case:?}: the order stated must differ from the sort, got {stated:?}"
        );
        for declared in DECLARED_INSTANCE_TYPES {
            assert!(
                stated.iter().any(|value| value == declared),
                "case {case:?}: expected the member's whole declared set, which {declared:?} is \
                 in, got {stated:?}"
            );
        }
    }
}
