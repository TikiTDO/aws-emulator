// SPDX-License-Identifier: CC0-1.0

//! `StartSearchJob`'s `EncryptionKeyArn`, driven over HTTP exactly as a client drives it.
//!
//! THE RULE UNDER TEST: the member is an `@arnReference` to a KMS key
//! (`smithy_query(query="shape",shape="EncryptionKeyArn")`), so it is a REFERENCE the
//! service resolves rather than a string it stores: a value that resolves to a key of this
//! account at the endpoint the request was signed for is accepted and reported back
//! unchanged, and a value that resolves to none is refused `ResourceNotFoundException`
//! (HTTP 404) with NO job created. A request naming no key at all is unaffected — the
//! member is optional.
//!
//! The rows span what the rule generalises over: the member absent, the ARN of each of two
//! held keys, a held key's ARN at another Region's endpoint, a well-formed key ARN of a key
//! nothing created, an alias ARN naming no alias, another service's ARN, and a string that
//! is no ARN at all. The key a row names is created through KMS's OWN `CreateKey`
//! (`crate::search_jobs::create_kms_key`), so what the rows are judged against is the store
//! KMS's handlers write.
//!
//! The HTTP harness (`emulator`, `call`, `Answer`) is the search-job suite's: every suite
//! drives the same emulator the same way.

use serde_json::{json, Value};

use crate::search_jobs::{call, create_kms_key, emulator, Answer};

/// The Region every call below is signed for, which is the endpoint the key references are
/// resolved at.
const REGION: &str = "us-west-2";

/// Start a search job, optionally under `encryption_key`.
///
/// `pub(crate)` because the relayed-refusal suite (`tests/all/relayed_kms_refusal.rs`)
/// drives the same request: how the refusal of an unresolvable reference is WORDED is
/// judged on exactly the call this builds.
pub(crate) async fn start_under_key(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
    encryption_key: Option<&str>,
) -> Answer {
    let mut body = json!({
        "Name": "encrypted",
        "SearchScope": {"BackupResourceTypes": ["S3"]}
    });
    if let Some(key) = encryption_key {
        body["EncryptionKeyArn"] = json!(key);
    }
    call(emulator, region, "PUT", "/search-jobs", Some(body)).await
}

/// How many search jobs the service reports, read through `ListSearchJobs`, so a refused
/// start is shown to have created nothing by the same surface a client would use.
async fn listed_jobs(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    region: &str,
) -> usize {
    let listed = call(emulator, region, "GET", "/search-jobs", None).await;
    assert_eq!(
        listed.status, 200,
        "ListSearchJobs must succeed, got {} ({:?})",
        listed.status, listed.body
    );
    listed
        .get("SearchJobs")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

#[tokio::test]
async fn an_encryption_key_reference_is_resolved_before_a_job_is_minted() {
    let emulator = emulator();
    let account = engine::emulator_account_id();
    // Two keys, so an accepted row is shown to be resolving the key it NAMED rather than
    // merely finding that the store holds some key.
    let first_key = create_kms_key(&emulator, REGION).await;
    let second_key = create_kms_key(&emulator, REGION).await;
    assert_ne!(
        first_key, second_key,
        "the seed keys must be two different keys, got {first_key:?} twice"
    );

    // (case, the value sent as EncryptionKeyArn, whether it resolves to a key)
    let rows: Vec<(&str, Option<String>, bool)> = vec![
        ("no encryption key at all", None, true),
        ("the key ARN of a held key", Some(first_key.clone()), true),
        (
            "the key ARN of the other held key",
            Some(second_key.clone()),
            true,
        ),
        (
            "a well-formed key ARN of a key nothing created",
            Some(format!(
                "arn:aws:kms:{REGION}:{account}:key/11111111-2222-4333-8444-555555555555"
            )),
            false,
        ),
        (
            // A KMS key is regional: the same key id at another Region's endpoint is a
            // different, absent key.
            "a held key's ARN at another Region",
            Some(first_key.replace(REGION, "eu-west-1")),
            false,
        ),
        (
            "an alias ARN naming no alias",
            Some(format!("arn:aws:kms:{REGION}:{account}:alias/nothing")),
            false,
        ),
        (
            "an ARN of another service's resource",
            Some("arn:aws:s3:::some-bucket".to_string()),
            false,
        ),
        (
            "a string that is not an ARN at all",
            Some("not-a-key".to_string()),
            false,
        ),
    ];

    let mut accepted = 0;
    for (case, key, resolves) in rows {
        let before = listed_jobs(&emulator, REGION).await;
        let started = start_under_key(&emulator, REGION, key.as_deref()).await;

        if !resolves {
            assert_eq!(
                started.status, 404,
                "case {case:?}: a key reference that resolves to nothing must be 404, got {} \
                 ({:?})",
                started.status, started.body
            );
            assert_eq!(
                started.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: the refusal must carry ResourceNotFoundException, got {:?}",
                started.error_code
            );
            assert_eq!(
                started.string("resourceId"),
                key.clone().unwrap_or_default(),
                "case {case:?}: the refusal must name the submitted reference, got {:?}",
                started.get("resourceId")
            );
            assert!(
                !started.string("message").is_empty(),
                "case {case:?}: the refusal must say why the key could not be used, got {:?}",
                started.body
            );
            assert!(
                started.get("SearchJobIdentifier").is_null(),
                "case {case:?}: a refused start must report no job, got {:?}",
                started.body
            );
            assert_eq!(
                listed_jobs(&emulator, REGION).await,
                before,
                "case {case:?}: a refused start must create no job"
            );
            continue;
        }

        assert_eq!(
            started.status, 200,
            "case {case:?}: a resolvable key reference must be accepted, got {} ({:?})",
            started.status, started.body
        );
        accepted += 1;
        assert_eq!(
            listed_jobs(&emulator, REGION).await,
            before + 1,
            "case {case:?}: an accepted start must create exactly one job"
        );

        // Round trip: the stored job reports the reference the request carried, unchanged.
        let identifier = started.string("SearchJobIdentifier");
        let fetched = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetSearchJob must report the minted job, got {} ({:?})",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.get("EncryptionKeyArn"),
            &key.clone().map_or(Value::Null, Value::String),
            "case {case:?}: the stored EncryptionKeyArn must be the submitted one (absent \
             stays absent), got {:?}",
            fetched.get("EncryptionKeyArn")
        );
    }
    assert_eq!(
        accepted, 3,
        "the table must exercise the accepting path as well as the refusing one, accepted \
         {accepted}"
    );
}

/// RULE: the reference is resolved in KMS's OWN store at the instant of the call, so the
/// same ARN is accepted by an emulator whose KMS store holds that key and refused by one
/// whose store never did — the value's TEXT is not what decides.
#[tokio::test]
async fn the_same_key_arn_is_accepted_or_refused_by_what_the_kms_store_holds() {
    let holding = emulator();
    let key = create_kms_key(&holding, REGION).await;

    let accepted = start_under_key(&holding, REGION, Some(&key)).await;
    assert_eq!(
        accepted.status, 200,
        "the key this emulator holds must be accepted, got {} ({:?})",
        accepted.status, accepted.body
    );

    // A second emulator, with its own stores: nothing ever created that key here.
    let empty = emulator();
    let refused = start_under_key(&empty, REGION, Some(&key)).await;
    assert_eq!(
        refused.status, 404,
        "the same ARN must be refused where no key was created, got {} ({:?})",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "the refusal must carry ResourceNotFoundException, got {:?}",
        refused.error_code
    );
    assert_eq!(
        listed_jobs(&empty, REGION).await,
        0,
        "a refused start must create no job"
    );
}
