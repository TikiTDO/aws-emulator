// SPDX-License-Identifier: CC0-1.0

//! The revision chain an `UpdateConfiguration` grows belongs to the configuration its
//! id names — and to no other configuration the store holds.
//!
//! RULE: `UpdateConfiguration` appends the next revision OF THE CONFIGURATION its
//! `ConfigurationId` names, so every number, description and document a later read
//! reports for a configuration is computed from that configuration's own record: a
//! configuration's latest revision counts the updates IT was sent, whatever updates
//! the store's other configurations were sent in between.
//!
//! The table spans what the rule generalises over — how many updates one configuration
//! receives (none, one, and several) and in what order those updates interleave with
//! another configuration's — with the rows driven round-robin so a chain shared across
//! records, or a revision number taken from a store-wide count, shows up as a wrong
//! number or a foreign document rather than as a passing row.
//!
//! `FIRST_CONFIGURATION_REVISION` is the service's own published first revision
//! number, not a constant invented here.

use serde_json::Value;

use service_com_amazonaws_mq_mq::state::FIRST_CONFIGURATION_REVISION;

use crate::broker_lifecycle::{string_at, Mq};
use crate::configuration_wire_form::base64_data;

/// One configuration of the table: the name it is created under and the documents the
/// updates it receives send, one per update. A row with no document receives no update
/// at all, which is the case a store-wide revision count gets wrong first.
struct IsolationCase {
    name: &'static str,
    documents: &'static [&'static str],
}

/// The rows differ in how many updates the configuration receives (0, 1, 2, 3) and in
/// the documents those updates carry, so no two configurations can be confused by their
/// revision count or by the document a revision stores.
const ISOLATION_CASES: &[IsolationCase] = &[
    IsolationCase {
        name: "isolation-never-updated",
        documents: &[],
    },
    IsolationCase {
        name: "isolation-updated-once",
        documents: &["<broker><plugins/></broker>"],
    },
    IsolationCase {
        name: "isolation-updated-twice",
        documents: &[
            "[{rabbit,[{vm_memory_high_watermark,0.4}]}].",
            "[{rabbit,[{vm_memory_high_watermark,0.6}]}].",
        ],
    },
    IsolationCase {
        name: "isolation-updated-thrice",
        documents: &[
            "<broker><destinationPolicy/></broker>",
            "<broker xmlns=\"http://activemq.apache.org/schema/core\"/>",
            "<broker><networkConnectors/></broker>",
        ],
    },
];

/// RULE: each update grows the chain of the configuration it names only, so after the
/// rows' updates are interleaved every configuration reports its OWN revision count,
/// its own numbering from the published first revision, and the document each of its
/// own updates sent.
#[tokio::test]
async fn an_update_grows_only_the_chain_of_the_configuration_it_names() {
    let mq = Mq::new("us-east-1");

    // Every row's configuration exists before any update is sent, so the updates below
    // really do run against a store holding several revision chains at once.
    let mut ids: Vec<String> = Vec::new();
    for case in ISOLATION_CASES {
        let (status, code, created) = mq.create_configuration(case.name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the configuration to be created, got {status} {code:?} \
             {created:?}",
            case.name
        );
        assert_eq!(
            created
                .pointer("/latestRevision/revision")
                .and_then(Value::as_i64),
            Some(i64::from(FIRST_CONFIGURATION_REVISION)),
            "case {:?}: a created configuration starts at the published first revision, \
             got {created:?}",
            case.name
        );
        ids.push(string_at(&created, "/id"));
    }

    // Round-robin: the Nth update of every row is sent before any row's (N+1)th, so the
    // calls for one configuration are separated by calls for the others.
    let rounds = ISOLATION_CASES
        .iter()
        .map(|case| case.documents.len())
        .max()
        .unwrap_or_default();
    for round in 0..rounds {
        for (case, id) in ISOLATION_CASES.iter().zip(&ids) {
            let Some(document) = case.documents.get(round) else {
                continue;
            };
            let label = format!("{:?} update {}", case.name, round + 1);
            let data = base64_data(document);
            let description = format!("{} revision {}", case.name, round + 1);
            let (status, code, updated) = mq
                .update_configuration(id, &data, Some(&description))
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} {code:?} {updated:?}"
            );
            assert_eq!(
                string_at(&updated, "/id"),
                *id,
                "case {label:?}: the answer must name the configuration updated, got {updated:?}"
            );
            assert_eq!(
                string_at(&updated, "/name"),
                case.name,
                "case {label:?}: the answer must name the configuration updated, got {updated:?}"
            );
            assert_eq!(
                updated
                    .pointer("/latestRevision/revision")
                    .and_then(Value::as_i64),
                Some(i64::from(FIRST_CONFIGURATION_REVISION) + round as i64 + 1),
                "case {label:?}: the revision counts this configuration's own updates, \
                 got {updated:?}"
            );
        }
    }

    for (case, id) in ISOLATION_CASES.iter().zip(&ids) {
        let expected_latest =
            i64::from(FIRST_CONFIGURATION_REVISION) + case.documents.len() as i64;

        // The configuration's own latest revision: its create plus the updates IT was
        // sent, never the store's total.
        let (status, code, described) = mq.describe_configuration(id).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the configuration to read back, got {status} {code:?} \
             {described:?}",
            case.name
        );
        assert_eq!(
            described
                .pointer("/latestRevision/revision")
                .and_then(Value::as_i64),
            Some(expected_latest),
            "case {:?}: expected latest revision {expected_latest}, got {described:?}",
            case.name
        );

        // The whole chain, in one page past anything this test makes: exactly the
        // create's revision followed by one per update of THIS configuration.
        let (status, code, listed) = mq
            .list_configuration_revisions(id, "maxResults=100")
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} {code:?} {listed:?}",
            case.name
        );
        let numbers: Vec<i64> = listed
            .pointer("/revisions")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry
                            .pointer("/revision")
                            .and_then(Value::as_i64)
                            .unwrap_or_default()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let expected_numbers: Vec<i64> =
            (i64::from(FIRST_CONFIGURATION_REVISION)..=expected_latest).collect();
        assert_eq!(
            numbers, expected_numbers,
            "case {:?}: expected the chain {expected_numbers:?}, got {numbers:?} from \
             {listed:?}",
            case.name
        );

        // And each revision carries the document THIS configuration's update sent: the
        // create-time revision carries none, and the update-made ones carry their own.
        for (index, document) in case.documents.iter().enumerate() {
            let revision = i64::from(FIRST_CONFIGURATION_REVISION) + index as i64 + 1;
            let label = format!("{:?} revision {revision}", case.name);
            let (status, code, answer) = mq
                .describe_configuration_revision(id, &revision.to_string())
                .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected 200, got {status} {code:?} {answer:?}"
            );
            let expected_data = base64_data(document);
            assert_eq!(
                answer.pointer("/data").and_then(Value::as_str),
                Some(expected_data.as_str()),
                "case {label:?}: expected this configuration's own document, got {answer:?}"
            );
            let expected_description = format!("{} revision {}", case.name, index + 1);
            assert_eq!(
                answer.pointer("/description").and_then(Value::as_str),
                Some(expected_description.as_str()),
                "case {label:?}: expected this configuration's own description, got {answer:?}"
            );
            assert_eq!(
                string_at(&answer, "/configurationId"),
                *id,
                "case {label:?}: the answer must name the configuration it describes, \
                 got {answer:?}"
            );
        }

        // A revision number one past this configuration's own chain names nothing, even
        // where another configuration in the store does hold that number.
        let past_the_chain = expected_latest + 1;
        let label = format!("{:?} revision {past_the_chain}", case.name);
        let (status, code, answer) = mq
            .describe_configuration_revision(id, &past_the_chain.to_string())
            .await;
        assert_eq!(
            status, 404,
            "case {label:?}: a revision this configuration never made must not be served, \
             got {status} {answer:?}"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {label:?}: expected NotFoundException, got {code:?} {answer:?}"
        );
    }
}
