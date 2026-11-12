// SPDX-License-Identifier: CC0-1.0

//! The Firehose-to-Amazon-OpenSearch-Service delivery into a domain whose authorization
//! boundary is FINE-GRAINED ACCESS CONTROL, driven end to end over the wire: the stream and
//! the put as Firehose calls, the domain and its security configuration as Amazon
//! OpenSearch Service calls, the delivered documents read back through the domain's OWN REST
//! endpoint, and the backup read as the path-style Amazon S3 requests a client sends.
//!
//! The rule under test: on a domain with fine-grained access control switched on, whether a
//! delivery's documents are indexed is the CLUSTER's decision about the destination's role
//! — the role mappings the domain holds, not IAM alone — and every document the cluster
//! does not take is written to the destination's Amazon S3 backup under the failed-document
//! folder instead of being dropped ("Firehose writes any documents that could not be
//! indexed to the configured Amazon S3 destination, with `AmazonOpenSearchService-failed/`
//! appended to the key prefix", `S3BackupMode`).
//!
//! Two rows of that rule, which is what an estate needs to be able to tell apart: a role a
//! role mapping BINDS indexes what the index's mappings accept and parks only what they
//! refuse, and a role NOTHING binds indexes nothing at all and parks every record intact —
//! and adding the mapping makes the very same stream deliver.
//!
//! The harness comes from [`super::destination_delivery`]; this suite adds no second copy
//! of it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state as search_store;

use crate::delivery_stream_lifecycle::ROLE_ARN;
use crate::destination_delivery::{base64, Answer, Emulator};

/// How long a delivery is waited for before the case fails: far longer than the emulator's
/// delivery span, so a slow machine does not fail a case the clock would have served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How often the destination is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The folder a document the domain did not index is written under, when the backup
/// configures no `ErrorOutputPrefix` of its own.
const FAILED_FOLDER: &str = "AmazonOpenSearchService-failed/";

/// The bucket every case's backup is configured with.
const BACKUP_BUCKET: &str = "fgac-search-backup";

/// That bucket as a `BucketARN`.
const BACKUP_BUCKET_ARN: &str = "arn:aws:s3:::fgac-search-backup";

/// The delivery role NOTHING binds — the position the recorded scenario's unmapped stream
/// is in: allowed this service's data-plane actions by IAM, and held by no role mapping.
const UNMAPPED_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/firehose-unmapped";

/// The index the destinations below are configured with, before rotation.
const INDEX: &str = "shipments";

/// The principal the harness's requests speak for: the IAM user this emulator's own
/// long-term key belongs to, which is what a domain naming it as its master user grants
/// everything to.
fn master_user_arn() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// One request to a domain's REST data plane, under the addressing a client that cannot
/// resolve the published endpoint host uses.
async fn rest(
    emulator: &Emulator,
    domain: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    emulator
        .signed(
            "es",
            method,
            &format!("/_aws/es/{domain}{path}"),
            payload.as_bytes(),
        )
        .await
}

/// A REST answer's JSON body, with the case named on a body that is not JSON.
fn json_body(case: &str, answer: &Answer) -> Value {
    serde_json::from_slice(&answer.body).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: expected a JSON answer, got {error} in {:?}",
            String::from_utf8_lossy(&answer.body)
        )
    })
}

/// Create a domain whose authorization boundary is fine-grained access control, with the
/// harness's own principal as its master user, answering the `ARN` its status published.
///
/// The three prerequisites of that boundary are carried too — the domain's data encrypted
/// at rest, its node-to-node traffic encrypted and HTTPS-only traffic — because the
/// OpenSearch control plane refuses a create that would switch fine-grained access
/// control on without them.
async fn create_fgac_domain(emulator: &Emulator, name: &str) -> String {
    let request = json!({
        "DomainName": name,
        "EngineVersion": "OpenSearch_2.11",
        "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
        "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
        "EncryptionAtRestOptions": { "Enabled": true },
        "NodeToNodeEncryptionOptions": { "Enabled": true },
        "DomainEndpointOptions": { "EnforceHTTPS": true },
        "AdvancedSecurityOptions": {
            "Enabled": true,
            "InternalUserDatabaseEnabled": false,
            "MasterUserOptions": { "MasterUserARN": master_user_arn() }
        }
    });
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
    let answered = json_body("create domain", &answer);
    answered["DomainStatus"]["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("create domain {name} must publish an ARN, got {answered:?}"))
        .to_string()
}

/// An Amazon OpenSearch Service destination configuration naming `domain_arn`, delivering
/// as `role_arn`, with its Amazon S3 backup under `prefix` and the one-day index rotation
/// the recorded scenario configures.
fn destination(domain_arn: &str, role_arn: &str, prefix: &str) -> Value {
    json!({
        "RoleARN": role_arn,
        "DomainARN": domain_arn,
        "IndexName": INDEX,
        "IndexRotationPeriod": "OneDay",
        "S3BackupMode": "FailedDocumentsOnly",
        // The shortest wait this destination's `IntervalInSeconds` admits (its published
        // range is 0..900): these cases are about which identity indexes a document, not
        // about how long a buffer is held first.
        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        "S3Configuration": {
            "RoleARN": role_arn,
            "BucketARN": BACKUP_BUCKET_ARN,
            "CompressionFormat": "UNCOMPRESSED",
            "Prefix": prefix,
        },
    })
}

/// Every index name one domain holds, read out of the Amazon OpenSearch Service store the
/// delivery wrote them into.
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

/// The error-output documents the backup holds under `prefix` once it holds at least `want`
/// of them, or a failure naming `case` once [`DELIVERY_DEADLINE`] has passed.
async fn parked_documents(emulator: &Emulator, case: &str, prefix: &str, want: usize) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let mut documents = Vec::new();
        for key in emulator.keys_under(BACKUP_BUCKET, prefix).await {
            let body = emulator.object(BACKUP_BUCKET, &key).await;
            for line in String::from_utf8_lossy(&body).lines() {
                documents.push(serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("case {case:?}: an error-output document must be JSON: {error}")
                }));
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

/// The index name the delivery created once the domain holds one, or a failure naming
/// `case` once [`DELIVERY_DEADLINE`] has passed.
async fn rotated_index(emulator: &Emulator, case: &str, domain: &str) -> String {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let names = index_names(emulator, domain);
        if let [one] = names.as_slice() {
            return one.clone();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the delivery to create exactly one index, got {names:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The documents `index` holds, counted through the domain's OWN REST endpoint — the read a
/// client performs, rather than a read of the store.
async fn counted(emulator: &Emulator, case: &str, domain: &str, index: &str) -> i64 {
    let answer = rest(emulator, domain, "GET", &format!("/{index}/_count"), None).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected the count to be served, got {} with {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    json_body(case, &answer)["count"].as_i64().unwrap_or(-1)
}

/// `-####-##-##`: the shape of the suffix `OneDay` rotation appends, with `#` standing for
/// a digit. Asserted as a shape so the case states the calendar fields the period names
/// without recomputing the calendar.
fn has_day_suffix(index: &str) -> bool {
    let Some(suffix) = index.strip_prefix(INDEX) else {
        return false;
    };
    suffix.len() == "-0000-00-00".len()
        && suffix.chars().zip("-####-##-##".chars()).all(|(got, want)| {
            if want == '#' {
                got.is_ascii_digit()
            } else {
                got == want
            }
        })
}

/// Write the strict index template the recorded scenario writes: `dynamic: "strict"` and a
/// non-coercing integer field, so a document carrying an unmapped field or an unparsable
/// value is a mapping conflict the cluster refuses.
async fn given_strict_template(emulator: &Emulator, domain: &str) {
    let answer = rest(
        emulator,
        domain,
        "PUT",
        "/_index_template/shipments",
        Some(json!({
            "index_patterns": [format!("{INDEX}-*")],
            "priority": 100,
            "template": {
                "mappings": {
                    "dynamic": "strict",
                    "properties": {
                        "sku": { "type": "keyword" },
                        "qty": { "type": "integer", "coerce": false }
                    }
                }
            }
        })),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "the strict index template must be written, got {} with {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// Bind `role_arn` as a `backend_role` of a fine-grained-access-control role — the one
/// change that lets a principal that is not the master user reach the domain.
async fn given_role_mapping(emulator: &Emulator, domain: &str, role_arn: &str) {
    let answer = rest(
        emulator,
        domain,
        "PUT",
        "/_plugins/_security/api/rolesmapping/firehose_writer",
        Some(json!({ "backend_roles": [role_arn], "users": [], "hosts": [] })),
    )
    .await;
    assert!(
        (200..300).contains(&answer.status),
        "the role mapping must be written, got {} with {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// The five records the strict template accepts, and the two it refuses: one carrying a
/// field the mappings do not map, one carrying a value the mapped type cannot parse.
fn records() -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let conforming: Vec<Vec<u8>> = (0..5)
        .map(|ordinal| format!(r#"{{"sku":"sku-{ordinal}","qty":{ordinal}}}"#).into_bytes())
        .collect();
    let conflicting: Vec<Vec<u8>> = vec![
        br#"{"sku":"sku-x","qty":1,"unmapped":"x"}"#.to_vec(),
        br#"{"sku":"sku-y","qty":"many"}"#.to_vec(),
    ];
    (conforming, conflicting)
}

/// RULE: a role a role mapping BINDS may write, so the documents the index's mappings
/// accept are indexed into the rotated index and only the ones they refuse are parked
/// under the failed-document folder, each carrying the engine's reason and the record's own
/// bytes.
#[tokio::test]
async fn a_mapped_role_indexes_what_fits_and_parks_only_the_conflicts() {
    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    let domain = "search-fgac-mapped";
    let domain_arn = create_fgac_domain(&emulator, domain).await;
    given_strict_template(&emulator, domain).await;
    given_role_mapping(&emulator, domain, ROLE_ARN).await;

    emulator
        .create_active_stream(
            "fgac-mapped-stream",
            "AmazonopensearchserviceDestinationConfiguration",
            destination(&domain_arn, ROLE_ARN, "mapped/"),
        )
        .await;

    let (conforming, conflicting) = records();
    let mut batch = conforming.clone();
    batch.extend(conflicting.clone());
    let (status, answered) = emulator.put_batch("fgac-mapped-stream", &batch).await;
    assert_eq!(
        status, 200,
        "case \"a mapped role\": the put must be accepted, got {answered:?}"
    );

    let index = rotated_index(&emulator, "a mapped role", domain).await;
    assert!(
        has_day_suffix(&index),
        "case \"a mapped role\": expected the index named for the day of the delivery, got \
         {index:?}"
    );
    let count = counted(&emulator, "a mapped role", domain, &index).await;
    assert_eq!(
        count,
        i64::try_from(conforming.len()).unwrap_or_default(),
        "case \"a mapped role\": expected only the conforming records indexed, got {count}"
    );

    let parked = parked_documents(
        &emulator,
        "a mapped role",
        &format!("mapped/{FAILED_FOLDER}"),
        conflicting.len(),
    )
    .await;
    assert_eq!(
        parked.len(),
        conflicting.len(),
        "case \"a mapped role\": expected exactly the conflicts parked, got {parked:?}"
    );
    let parked_raw: Vec<String> = parked
        .iter()
        .map(|document| document["rawData"].as_str().unwrap_or_default().to_string())
        .collect();
    for record in &conflicting {
        assert!(
            parked_raw.contains(&base64(record)),
            "case \"a mapped role\": expected the conflicting record's own bytes parked, got \
             {parked_raw:?}"
        );
    }
    for document in &parked {
        assert_eq!(
            document["esIndexName"].as_str(),
            Some(index.as_str()),
            "case \"a mapped role\": expected the index the delivery used, got {document:?}"
        );
        assert!(
            document["errorCode"]
                .as_str()
                .is_some_and(|code| !code.is_empty()),
            "case \"a mapped role\": expected the engine's error code, got {document:?}"
        );
        assert!(
            document["errorMessage"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case \"a mapped role\": expected a reason beside the code, got {document:?}"
        );
    }
    // Nothing of this delivery was written anywhere but the failed-document folder.
    let keys = emulator.keys_under(BACKUP_BUCKET, "mapped/").await;
    assert!(
        keys.iter()
            .all(|key| key.starts_with(&format!("mapped/{FAILED_FOLDER}"))),
        "case \"a mapped role\": expected the backup to hold only failed documents, got {keys:?}"
    );
}

/// RULE: a role NOTHING binds may write nothing, so EVERY record of its delivery is parked
/// intact and no index is created at all — and adding the role mapping makes the same
/// stream deliver.
#[tokio::test]
async fn an_unmapped_role_indexes_nothing_until_a_role_mapping_binds_it() {
    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    let domain = "search-fgac-unmapped";
    let domain_arn = create_fgac_domain(&emulator, domain).await;
    given_strict_template(&emulator, domain).await;

    emulator
        .create_active_stream(
            "fgac-unmapped-stream",
            "AmazonopensearchserviceDestinationConfiguration",
            destination(&domain_arn, UNMAPPED_ROLE_ARN, "unmapped/"),
        )
        .await;

    let (conforming, _) = records();
    let (status, answered) = emulator
        .put_batch("fgac-unmapped-stream", &conforming)
        .await;
    assert_eq!(
        status, 200,
        "case \"an unmapped role\": the put must be accepted, got {answered:?}"
    );

    let parked = parked_documents(
        &emulator,
        "an unmapped role",
        &format!("unmapped/{FAILED_FOLDER}"),
        conforming.len(),
    )
    .await;
    assert_eq!(
        parked.len(),
        conforming.len(),
        "case \"an unmapped role\": expected every record parked, got {parked:?}"
    );
    let parked_raw: Vec<String> = parked
        .iter()
        .map(|document| document["rawData"].as_str().unwrap_or_default().to_string())
        .collect();
    for record in &conforming {
        assert!(
            parked_raw.contains(&base64(record)),
            "case \"an unmapped role\": expected the record parked intact, got {parked_raw:?}"
        );
    }
    for document in &parked {
        assert_eq!(
            document["errorCode"].as_str(),
            Some("security_exception"),
            "case \"an unmapped role\": expected the cluster's authorization error type, got \
             {document:?}"
        );
    }
    assert!(
        index_names(&emulator, domain).is_empty(),
        "case \"an unmapped role\": expected no index to have been created, got {:?}",
        index_names(&emulator, domain)
    );

    // Binding the role is the ONE change that makes the same stream deliver.
    given_role_mapping(&emulator, domain, UNMAPPED_ROLE_ARN).await;
    let (status, answered) = emulator
        .put_batch("fgac-unmapped-stream", &conforming)
        .await;
    assert_eq!(
        status, 200,
        "case \"the role bound by a mapping\": the put must be accepted, got {answered:?}"
    );
    let index = rotated_index(&emulator, "the role bound by a mapping", domain).await;
    let count = counted(&emulator, "the role bound by a mapping", domain, &index).await;
    assert_eq!(
        count,
        i64::try_from(conforming.len()).unwrap_or_default(),
        "case \"the role bound by a mapping\": expected every record indexed, got {count}"
    );
}
