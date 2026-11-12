// SPDX-License-Identifier: CC0-1.0

//! The Firehose-to-Amazon-OpenSearch-Service data plane, driven over the wire.
//!
//! The rule under test: a stream whose destination is an Amazon OpenSearch Service domain
//! DELIVERS each buffered record to that domain as one document of the destination's index
//! — "Firehose buffers records before delivering them to the destination" (`PutRecord`) —
//! and a document the domain does not take is written to the destination's Amazon S3
//! backup instead of being dropped: "Firehose writes any documents that could not be
//! indexed to the configured Amazon S3 destination, with `AmazonOpenSearchService-failed/`
//! appended to the key prefix" (`S3BackupMode`). Whether the domain takes a document is
//! the DOMAIN's decision — its access policy on the delivery role, and its index's
//! mappings on the document — so the same stream both indexes and parks, record by record.
//!
//! Every service is mounted on ONE shared engine, exactly as the router mounts them, so
//! what is exercised is the hand-off between three of them: the Firehose calls as `POST /`
//! with `X-Amz-Target`, the domain and index creates as the Amazon OpenSearch Service
//! `restJson1` calls a client makes, and the backup reads as the path-style Amazon S3
//! requests a client sends.
//!
//! The documents a delivery INDEXED are read back out of the Amazon OpenSearch Service
//! store they were written into — the one store that service's own reads report from. A
//! sibling suite (`opensearch_fgac_destination_delivery`) reads them back through the
//! domain's own REST data plane, which is how a client would search them.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state as search_store;

use crate::delivery_stream_lifecycle::{REGION, ROLE_ARN};
use crate::destination_delivery::{base64, Emulator};

/// How long a delivery is waited for before the case fails: far longer than the
/// emulator's delivery span, so a slow machine does not fail a case the clock would have
/// served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How often the destination is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The folder a document the domain did not index is written under, when the backup
/// configures no `ErrorOutputPrefix` of its own: "with `AmazonOpenSearchService-failed/`
/// appended to the key prefix" (`S3BackupMode`).
const FAILED_FOLDER: &str = "AmazonOpenSearchService-failed/";

/// The bucket every case's backup is configured with.
const BACKUP_BUCKET: &str = "search-backup";

/// That bucket as a `BucketARN`.
const BACKUP_BUCKET_ARN: &str = "arn:aws:s3:::search-backup";

/// Create a domain, answering the `ARN` its status published — the value a destination
/// configuration names it by.
async fn create_domain(emulator: &Emulator, name: &str, access_policies: Option<Value>) -> String {
    let mut request = json!({ "DomainName": name });
    if let Some(policy) = access_policies {
        // `AccessPolicies` is the policy DOCUMENT as text, which is how a client sends it.
        request["AccessPolicies"] = json!(policy.to_string());
    }
    let answer = emulator
        .signed(
            "es",
            "POST",
            "/2021-01-01/opensearch/domain",
            &serde_json::to_vec(&request).expect("the request body serializes"),
        )
        .await;
    assert_eq!(
        answer.status,
        200,
        "create domain {name}: got {}",
        String::from_utf8_lossy(&answer.body)
    );
    let answered: Value = serde_json::from_slice(&answer.body).expect("the answer is JSON");
    answered["DomainStatus"]["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("create domain {name} must publish an ARN, got {answered:?}"))
        .to_string()
}

/// Create an index on `domain` carrying `schema`.
async fn create_index(emulator: &Emulator, domain: &str, index: &str, schema: Value) {
    let answer = emulator
        .signed(
            "es",
            "POST",
            &format!("/2021-01-01/opensearch/domain/{domain}/index"),
            &serde_json::to_vec(&json!({ "IndexName": index, "IndexSchema": schema }))
                .expect("the request body serializes"),
        )
        .await;
    assert_eq!(
        answer.status,
        200,
        "create index {index} on {domain}: got {}",
        String::from_utf8_lossy(&answer.body)
    );
}

/// The documents one index of one domain holds, keyed by their id, read out of the Amazon
/// OpenSearch Service store the delivery wrote them into.
fn indexed(emulator: &Emulator, domain: &str, index: &str) -> BTreeMap<String, Value> {
    let state = search_store::service_state(&**emulator.engine())
        .expect("the Amazon OpenSearch Service store is reachable from the shared engine");
    let store = state.lock();
    store
        .domains
        .get(domain)
        .and_then(|domain| domain.indexes.get(index))
        .map(|index| {
            index
                .documents
                .values()
                .map(|document| (document.id.clone(), document.source.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Every index name one domain holds.
fn index_names(emulator: &Emulator, domain: &str) -> Vec<String> {
    let state = search_store::service_state(&**emulator.engine())
        .expect("the Amazon OpenSearch Service store is reachable from the shared engine");
    let store = state.lock();
    store
        .domains
        .get(domain)
        .map(|domain| domain.indexes.keys().cloned().collect())
        .unwrap_or_default()
}

/// The documents `index` holds once it holds at least `want` of them, or a failure naming
/// `case` once [`DELIVERY_DEADLINE`] has passed.
async fn indexed_documents(
    emulator: &Emulator,
    case: &str,
    domain: &str,
    index: &str,
    want: usize,
) -> BTreeMap<String, Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let documents = indexed(emulator, domain, index);
        if documents.len() >= want {
            return documents;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected at least {want} indexed document(s) in {index}, got \
             {documents:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The error-output documents the backup holds under `prefix` once it holds at least
/// `want` of them, or a failure naming `case` once [`DELIVERY_DEADLINE`] has passed.
///
/// One object may carry several documents, one per line, which is how a flush writes the
/// documents the domain refused.
async fn parked_documents(
    emulator: &Emulator,
    case: &str,
    prefix: &str,
    want: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let mut documents = Vec::new();
        for key in emulator.keys_under(BACKUP_BUCKET, prefix).await {
            let body = emulator.object(BACKUP_BUCKET, &key).await;
            for line in String::from_utf8_lossy(&body).lines() {
                documents.push(
                    serde_json::from_str(line)
                        .unwrap_or_else(|error| panic!("case {case:?}: an error-output document must be JSON: {error}")),
                );
            }
        }
        if documents.len() >= want {
            return documents;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected at least {want} parked document(s) under {prefix}, got \
             {documents:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// An Amazon OpenSearch Service destination configuration naming `domain_arn` and `index`,
/// with its Amazon S3 backup under `prefix`.
fn destination(domain_arn: &str, index: &str, prefix: &str) -> Value {
    json!({
        "RoleARN": ROLE_ARN,
        "DomainARN": domain_arn,
        "IndexName": index,
        "S3BackupMode": "FailedDocumentsOnly",
        // The shortest wait this destination's `IntervalInSeconds` admits (its published
        // range is 0..900): these cases are about WHICH documents the index takes, not
        // about how long a buffer is held before they are offered to it.
        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        "S3Configuration": {
            "RoleARN": ROLE_ARN,
            "BucketARN": BACKUP_BUCKET_ARN,
            "Prefix": prefix,
        },
    })
}

/// A strict mapping over two fields, which is what makes a document carrying anything else
/// a mapping conflict.
fn strict_schema() -> Value {
    json!({
        "mappings": {
            "dynamic": "strict",
            "properties": { "level": { "type": "keyword" }, "count": { "type": "long" } },
        }
    })
}

/// RULE: every record of a delivered buffer is offered to the destination's index as one
/// document; the ones the index's mappings accept are indexed, and each one it refuses is
/// written to the destination's Amazon S3 backup under the failed-document folder, carrying
/// the engine's own error type and the record's own bytes.
///
/// The rows span what the index can say about a document: a mapping that accepts it, a
/// strict mapping that has no field for it, a mapped type that cannot parse its value,
/// bytes that are not a document at all, a dynamic mapping that takes an unmapped field,
/// and an index the delivery itself creates.
#[tokio::test]
async fn documents_the_index_refuses_are_parked_in_the_backup() {
    /// What one case delivers, and what the domain is expected to do with it.
    struct Case {
        case: &'static str,
        /// The index's schema, or `None` to leave the index to the delivery to create.
        schema: Option<Value>,
        record: &'static [u8],
        /// The engine error type the backup must report, or `None` when the document is
        /// expected to be indexed.
        refused_with: Option<&'static str>,
    }

    let cases = [
        Case {
            case: "a document the strict mapping accepts",
            schema: Some(strict_schema()),
            record: br#"{"level":"info","count":1}"#,
            refused_with: None,
        },
        Case {
            case: "a field the strict mapping does not carry",
            schema: Some(strict_schema()),
            record: br#"{"level":"info","surprise":"x"}"#,
            refused_with: Some("strict_dynamic_mapping_exception"),
        },
        Case {
            case: "a value the mapped type cannot parse",
            schema: Some(strict_schema()),
            record: br#"{"level":"info","count":"many"}"#,
            refused_with: Some("mapper_parsing_exception"),
        },
        Case {
            case: "bytes that are not a document",
            schema: Some(strict_schema()),
            record: b"not a document at all",
            refused_with: Some("mapper_parsing_exception"),
        },
        Case {
            case: "an unmapped field under a dynamic mapping",
            schema: Some(json!({
                "mappings": { "properties": { "level": { "type": "keyword" } } }
            })),
            record: br#"{"level":"info","surprise":"x"}"#,
            refused_with: None,
        },
        Case {
            case: "an index the delivery creates on its first write",
            schema: None,
            record: br#"{"level":"info","count":2}"#,
            refused_with: None,
        },
    ];

    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    let domain_arn = create_domain(&emulator, "search-mappings", None).await;
    for (ordinal, case) in cases.iter().enumerate() {
        let index = format!("logs-{ordinal}");
        let stream = format!("stream-{ordinal}");
        let prefix = format!("case-{ordinal}/");
        if let Some(schema) = case.schema.clone() {
            create_index(&emulator, "search-mappings", &index, schema).await;
        }
        emulator
            .create_active_stream(
                &stream,
                "AmazonopensearchserviceDestinationConfiguration",
                destination(&domain_arn, &index, &prefix),
            )
            .await;
        let (status, answered) = emulator.put_batch(&stream, &[case.record.to_vec()]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put must be accepted, got {answered:?}",
            case.case
        );
        let record_id = answered["RequestResponses"][0]["RecordId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the put must answer a RecordId", case.case))
            .to_string();

        match case.refused_with {
            None => {
                let documents =
                    indexed_documents(&emulator, case.case, "search-mappings", &index, 1).await;
                let expected: Value =
                    serde_json::from_slice(case.record).expect("the case's record is a document");
                assert_eq!(
                    documents.get(&record_id),
                    Some(&expected),
                    "case {:?}: expected the record indexed under its own id, got {documents:?}",
                    case.case
                );
                assert!(
                    emulator
                        .keys_under(BACKUP_BUCKET, &format!("{prefix}{FAILED_FOLDER}"))
                        .await
                        .is_empty(),
                    "case {:?}: expected nothing parked in the backup",
                    case.case
                );
            }
            Some(error_type) => {
                let parked = parked_documents(
                    &emulator,
                    case.case,
                    &format!("{prefix}{FAILED_FOLDER}"),
                    1,
                )
                .await;
                assert_eq!(
                    parked.len(),
                    1,
                    "case {:?}: expected exactly one parked document, got {parked:?}",
                    case.case
                );
                let document = &parked[0];
                assert_eq!(
                    document["errorCode"].as_str(),
                    Some(error_type),
                    "case {:?}: expected the engine's error type, got {document:?}",
                    case.case
                );
                assert_eq!(
                    document["esIndexName"].as_str(),
                    Some(index.as_str()),
                    "case {:?}: expected the index the delivery used, got {document:?}",
                    case.case
                );
                assert_eq!(
                    document["esDocumentId"].as_str(),
                    Some(record_id.as_str()),
                    "case {:?}: expected the document id the delivery used, got {document:?}",
                    case.case
                );
                assert_eq!(
                    document["rawData"].as_str(),
                    Some(base64(case.record).as_str()),
                    "case {:?}: expected the record's own bytes, got {document:?}",
                    case.case
                );
                assert!(
                    document["errorMessage"]
                        .as_str()
                        .is_some_and(|message| !message.is_empty()),
                    "case {:?}: expected a reason beside the code, got {document:?}",
                    case.case
                );
                assert!(
                    indexed(&emulator, "search-mappings", &index).is_empty(),
                    "case {:?}: expected the refused document not to be indexed",
                    case.case
                );
            }
        }
    }
}

/// RULE: whether a delivery may write to the domain is the DOMAIN's decision about the
/// destination's role — the access policy it carries, read as IAM reads a resource-based
/// policy — and every document of a delivery the domain does not authorize is written to
/// the Amazon S3 backup rather than indexed.
///
/// The rows span the positions an access policy can put the delivery role in: no policy at
/// all, a policy allowing it, a policy allowing only another principal, and a policy
/// denying it outright.
#[tokio::test]
async fn a_domain_that_does_not_authorize_the_role_parks_every_document() {
    /// What one case's domain carries, and whether the delivery is expected through.
    struct Case {
        case: &'static str,
        access_policies: Option<Value>,
        authorized: bool,
    }

    let allow_every_action = |principal: &str| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": principal },
                "Action": "es:*",
                "Resource": "*",
            }],
        })
    };
    let another_role = "arn:aws:iam::000000000000:role/someone-else";

    let cases = [
        Case {
            case: "a domain carrying no access policy",
            access_policies: None,
            authorized: true,
        },
        Case {
            case: "an access policy allowing the delivery role",
            access_policies: Some(allow_every_action(ROLE_ARN)),
            authorized: true,
        },
        Case {
            case: "an access policy allowing only another principal",
            access_policies: Some(allow_every_action(another_role)),
            authorized: false,
        },
        Case {
            case: "an access policy denying the delivery role",
            access_policies: Some(json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": { "AWS": "*" },
                        "Action": "es:*",
                        "Resource": "*",
                    },
                    {
                        "Effect": "Deny",
                        "Principal": { "AWS": ROLE_ARN },
                        "Action": "es:*",
                        "Resource": "*",
                    },
                ],
            })),
            authorized: false,
        },
    ];

    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    for (ordinal, case) in cases.iter().enumerate() {
        let domain = format!("search-policy-{ordinal}");
        let stream = format!("policy-stream-{ordinal}");
        let prefix = format!("policy-{ordinal}/");
        let domain_arn = create_domain(&emulator, &domain, case.access_policies.clone()).await;
        emulator
            .create_active_stream(
                &stream,
                "AmazonopensearchserviceDestinationConfiguration",
                destination(&domain_arn, "events", &prefix),
            )
            .await;
        let records = [
            br#"{"event":"one"}"#.to_vec(),
            br#"{"event":"two"}"#.to_vec(),
        ];
        let (status, answered) = emulator.put_batch(&stream, &records).await;
        assert_eq!(
            status, 200,
            "case {:?}: the put must be accepted, got {answered:?}",
            case.case
        );

        if case.authorized {
            let documents = indexed_documents(&emulator, case.case, &domain, "events", 2).await;
            assert_eq!(
                documents.len(),
                2,
                "case {:?}: expected both records indexed, got {documents:?}",
                case.case
            );
            continue;
        }
        let parked = parked_documents(
            &emulator,
            case.case,
            &format!("{prefix}{FAILED_FOLDER}"),
            2,
        )
        .await;
        assert_eq!(
            parked.len(),
            2,
            "case {:?}: expected both documents parked, got {parked:?}",
            case.case
        );
        for document in &parked {
            assert_eq!(
                document["errorCode"].as_str(),
                Some("security_exception"),
                "case {:?}: expected the engine's authorization error type, got {document:?}",
                case.case
            );
        }
        assert!(
            indexed(&emulator, &domain, "events").is_empty(),
            "case {:?}: expected nothing indexed on a domain that refused the role",
            case.case
        );
    }
}

/// RULE: "index rotation appends a timestamp to the IndexName"
/// (`IndexRotationPeriod`), so a delivery writes into the configured index named for the
/// period the rotation is over — and into the configured name itself where no rotation is
/// configured.
///
/// The rows are every `AmazonopensearchserviceIndexRotationPeriod` value
/// (`smithy_query(query="shape", shape="AmazonopensearchserviceIndexRotationPeriod")`:
/// `NoRotation`, `OneHour`, `OneDay`, `OneWeek`, `OneMonth`), plus a configuration naming
/// none. The expected suffix is asserted as a SHAPE — `#` standing for a digit — so the
/// case states the calendar fields the period names without recomputing the calendar.
#[tokio::test]
async fn an_index_rotation_period_names_the_index_of_the_period() {
    // (the case, the `IndexRotationPeriod` value, the shape of the suffix it appends)
    let rows: &[(&str, Option<&str>, &str)] = &[
        ("no rotation configured", None, ""),
        ("NoRotation", Some("NoRotation"), ""),
        ("OneHour", Some("OneHour"), "-####-##-##-##"),
        ("OneDay", Some("OneDay"), "-####-##-##"),
        ("OneWeek", Some("OneWeek"), "-####-w##"),
        ("OneMonth", Some("OneMonth"), "-####-##"),
    ];

    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    for (ordinal, (case, period, shape)) in rows.iter().enumerate() {
        let domain = format!("search-rotation-{ordinal}");
        let stream = format!("rotation-stream-{ordinal}");
        let domain_arn = create_domain(&emulator, &domain, None).await;
        let mut config = destination(&domain_arn, "rotated", &format!("rotation-{ordinal}/"));
        if let Some(period) = period {
            config["IndexRotationPeriod"] = json!(period);
        }
        emulator
            .create_active_stream(
                &stream,
                "AmazonopensearchserviceDestinationConfiguration",
                config,
            )
            .await;
        let (status, answered) = emulator
            .put_batch(&stream, &[br#"{"event":"rotated"}"#.to_vec()])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the put must be accepted, got {answered:?}"
        );
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        let indexes = loop {
            let indexes = index_names(&emulator, &domain);
            if !indexes.is_empty() {
                break indexes;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected the delivery to write an index on {domain}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert_eq!(
            indexes.len(),
            1,
            "case {case:?}: expected the delivery to write one index, got {indexes:?}"
        );
        let expected = format!("rotated{shape}");
        assert!(
            matches_shape(&indexes[0], &expected),
            "case {case:?}: expected an index of the shape {expected:?}, got {:?}",
            indexes[0]
        );
    }
}

/// RULE: `DocumentIdOptions` decides who names a delivered document — Firehose, with an
/// id "stable across multiple delivery attempts" (the record's own id), or the domain,
/// when the delivery carries none.
#[tokio::test]
async fn document_id_options_decide_who_names_the_document() {
    // (the case, the `DefaultDocumentIdFormat` value, whether the id is the record's own)
    let rows: &[(&str, Option<&str>, bool)] = &[
        ("no DocumentIdOptions configured", None, true),
        ("FIREHOSE_DEFAULT", Some("FIREHOSE_DEFAULT"), true),
        ("NO_DOCUMENT_ID", Some("NO_DOCUMENT_ID"), false),
    ];

    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    for (ordinal, (case, format, firehose_names_it)) in rows.iter().enumerate() {
        let domain = format!("search-docid-{ordinal}");
        let stream = format!("docid-stream-{ordinal}");
        let domain_arn = create_domain(&emulator, &domain, None).await;
        let mut config = destination(&domain_arn, "ids", &format!("docid-{ordinal}/"));
        if let Some(format) = format {
            config["DocumentIdOptions"] = json!({ "DefaultDocumentIdFormat": format });
        }
        emulator
            .create_active_stream(
                &stream,
                "AmazonopensearchserviceDestinationConfiguration",
                config,
            )
            .await;
        let (status, answered) = emulator
            .put_batch(&stream, &[br#"{"event":"named"}"#.to_vec()])
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the put must be accepted, got {answered:?}"
        );
        let record_id = answered["RequestResponses"][0]["RecordId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the put must answer a RecordId"))
            .to_string();
        let documents = indexed_documents(&emulator, case, &domain, "ids", 1).await;
        let ids: Vec<&String> = documents.keys().collect();
        assert_eq!(
            ids.contains(&&record_id),
            *firehose_names_it,
            "case {case:?}: expected the record's own id to be used: {firehose_names_it}, got \
             {ids:?} for record {record_id:?}"
        );
    }
}

/// RULE: a destination whose domain this emulator holds none of is a delivery that has NOT
/// happened — nothing is indexed and nothing is parked while the destination's retry window
/// is open — and the records stay buffered, so a put that was accepted is not lost.
#[tokio::test]
async fn a_destination_no_domain_answers_leaves_the_records_buffered() {
    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    // An ARN of the documented form that no domain of this emulator carries.
    let absent = format!("arn:aws:es:{REGION}:000000000000:domain/never-created");
    emulator
        .create_active_stream(
            "absent-domain-stream",
            "AmazonopensearchserviceDestinationConfiguration",
            destination(&absent, "events", "absent/"),
        )
        .await;
    let (status, answered) = emulator
        .put_batch("absent-domain-stream", &[br#"{"event":"one"}"#.to_vec()])
        .await;
    assert_eq!(
        status, 200,
        "the put must be accepted, got {answered:?}"
    );
    // Long enough for several delivery windows to have come and gone.
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    let parked = emulator
        .keys_under(BACKUP_BUCKET, &format!("absent/{FAILED_FOLDER}"))
        .await;
    assert!(
        parked.is_empty(),
        "expected nothing parked while the retry window is open, got {parked:?}"
    );
    let described = emulator
        .firehose_ok(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": "absent-domain-stream" }),
        )
        .await;
    assert_eq!(
        described["DeliveryStreamDescription"]["DeliveryStreamStatus"].as_str(),
        Some("ACTIVE"),
        "the stream must still be ACTIVE, got {described:?}"
    );
}

/// Whether `text` has the shape `shape`: a `#` stands for one ASCII digit and every other
/// character for itself.
fn matches_shape(text: &str, shape: &str) -> bool {
    if text.chars().count() != shape.chars().count() {
        return false;
    }
    text.chars().zip(shape.chars()).all(|(character, expected)| {
        if expected == '#' {
            character.is_ascii_digit()
        } else {
            character == expected
        }
    })
}
