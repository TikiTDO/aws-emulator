// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT boundary of the tag-store projection the account-wide tagging API reads
//! (`ComAmazonawsGlueAwsglueState::tagged_resources`).
//!
//! RULE: a tag-store row belongs to the account that holds the resource, so the projection
//! for one account reports that account's resources and no other's — the same boundary the
//! single-resource tag accessors enforce, which resolve a database and a connection through
//! the catalog of the account the request addressed and refuse a crawler or a job definition
//! of an account this emulator does not serve.
//!
//! The resources are created over the wire through the shared [`crate::wire`] harness, so
//! what is projected is what a real create stored; the projection itself is read from the
//! store, because it is the record another service's API reads rather than an operation of
//! this one.
//!
//! The rows differ along the resource kind created and along the account asked for: the one
//! the resources were created in, and other accounts of the published form.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState;

use crate::resource_tagging::create_request;
use crate::wire::{emulator_on, ok, ACCOUNT_ID, REGION};

/// Accounts other than the one the harness serves. A Glue resource of another account is
/// reachable through none of this endpoint's reads, so none of its rows may be projected for
/// one of these either.
const OTHER_ACCOUNTS: [&str; 2] = ["111122223333", "000000000001"];

/// The four kinds whose create models a `Tags` map, as [`create_request`] spells them.
const TAGGABLE_KINDS: [&str; 4] = ["database", "connection", "crawler", "job"];

/// The ARNs the projection reports for `account_id`, each with its tag set.
fn projected(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: &str,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    store
        .tagged_resources(account_id, REGION)
        .into_iter()
        .map(|(arn, tags)| (arn, tags.into_iter().collect()))
        .collect()
}

/// An emulator and the engine it runs on, so the projection another service's API reads can
/// be read from the store the creates landed in.
fn emulator_and_engine() -> (
    smithy_server_runtime::MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// Every taggable kind is created in the served account; the projection for that account
/// reports all four with the tags their creates recorded, and the projection for any other
/// account reports nothing at all.
#[tokio::test]
async fn the_tag_store_projection_reports_one_accounts_resources_only() {
    let (router, engine) = emulator_and_engine();

    for kind in TAGGABLE_KINDS {
        let name = format!("scoped-{kind}");
        let (operation, body) = create_request(kind, &name, &json!({ "owner": kind }));
        ok(&router, operation, &body).await;
    }

    let served = projected(&engine, ACCOUNT_ID);
    for kind in TAGGABLE_KINDS {
        let arn = format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:{kind}/scoped-{kind}");
        assert_eq!(
            served
                .get(&arn)
                .and_then(|tags| tags.get("owner"))
                .map(String::as_str),
            Some(kind),
            "case {kind:?}: the serving account's projection must report {arn} with the tags \
             its create recorded, got {served:?}"
        );
    }

    for account in OTHER_ACCOUNTS {
        let other = projected(&engine, account);
        assert!(
            other.is_empty(),
            "case {account:?}: that account holds no Glue resource, so its projection must be \
             empty, got {other:?}"
        );
    }
}

/// The tags a create recorded are what the projection carries: a resource created with no
/// tags is a row with an empty set, and a resource created with tags keeps its own.
#[tokio::test]
async fn a_projected_row_carries_the_tags_its_own_create_recorded() {
    let (router, engine) = emulator_and_engine();

    for (kind, name, tags) in [
        ("database", "untagged-db", json!({})),
        ("job", "tagged-job", json!({ "owner": "etl" })),
    ] {
        let (operation, body) = create_request(kind, name, &tags);
        ok(&router, operation, &body).await;
    }

    let served = projected(&engine, ACCOUNT_ID);
    let untagged = format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:database/untagged-db");
    assert_eq!(
        served.get(&untagged),
        Some(&BTreeMap::new()),
        "an untagged resource is a row with no tags, got {served:?}"
    );
    let tagged = format!("arn:aws:glue:{REGION}:{ACCOUNT_ID}:job/tagged-job");
    assert_eq!(
        served
            .get(&tagged)
            .and_then(|tags| tags.get("owner"))
            .map(String::as_str),
        Some("etl"),
        "the tagged resource keeps the tags its own create recorded, got {served:?}"
    );
}
