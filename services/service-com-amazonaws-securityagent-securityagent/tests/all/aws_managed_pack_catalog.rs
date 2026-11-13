// SPDX-License-Identifier: CC0-1.0

//! The AWS managed security requirement pack catalog of the SecurityAgent emulator, end
//! to end over the wire: what `ListSecurityRequirementPacks` and
//! `GetSecurityRequirementPack` report for the packs a fresh account already holds.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name (source: this crate's `OPERATIONS.md` "Wire protocol" section), so a
//! value counts only once it has come back out of the shared store.
//!
//! RULE under test: the managed catalog is STATIC service content — each shipped pack
//! reports the fixed `description` it is shipped with, and both of its required
//! `date-time` stamps are the catalog's single publication instant, the same for every
//! managed pack and the same on every boot, rather than a value read from the clock
//! while the account's state was seeded. The table is `AWS_MANAGED_PACKS`, the catalog
//! itself, so an entry added to it is covered without these cases changing; the rows of
//! the other cases differ along the reader (listing against read), the boot the answer
//! came from, and whether the pack is one AWS ships or one a caller created.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    AwsManagedPack, AWS_MANAGED_PACKS, AWS_MANAGED_PACK_PUBLISHED_AT_EPOCH_SECONDS,
    MANAGEMENT_TYPE_AWS_MANAGED, MAX_PAGE_SIZE, PACK_STATUS_DISABLED, PACK_STATUS_ENABLED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member};

/// `call`, requiring `expected` as the HTTP status and naming the case when it is not.
async fn expect_status(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    expected: u16,
    case: &str,
) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, expected,
        "case {case:?}: expected {operation} to answer {expected}, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// Every summary `ListSecurityRequirementPacks` reports, paged to the end.
async fn list_summaries(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let payload =
            expect_status(router, "ListSecurityRequirementPacks", &request, 200, case).await;
        let page = payload
            .get("securityRequirementPackSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: ListSecurityRequirementPacks answered no \
                     securityRequirementPackSummaries list: {payload}"
                )
            })
            .clone();
        summaries.extend(page);
        token = member(&payload, "nextToken");
        if token.is_none() {
            break;
        }
        assert!(
            summaries.len() <= MAX_PAGE_SIZE as usize * 10,
            "case {case:?}: pagination did not terminate"
        );
    }
    summaries
}

/// The summary the listing reports for `pack_id`.
async fn summary_of(router: &MultiServiceRouter, pack_id: &str, case: &str) -> Value {
    let summaries = list_summaries(router, case).await;
    summaries
        .iter()
        .find(|summary| member(summary, "packId").as_deref() == Some(pack_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing does not report the pack {pack_id:?}")
        })
        .clone()
}

/// `GetSecurityRequirementPack` on `pack_id`, requiring HTTP 200.
async fn get_pack(router: &MultiServiceRouter, pack_id: &str, case: &str) -> Value {
    expect_status(
        router,
        "GetSecurityRequirementPack",
        &json!({"packId": pack_id}),
        200,
        case,
    )
    .await
}

/// A modelled `date-time` member as epoch seconds.
///
/// `SecurityRequirementPackSummary$createdAt`/`$updatedAt` and the same members of
/// `GetSecurityRequirementPackOutput` carry `timestampFormat: date-time`
/// (`smithy_query(query="shape", shape="SecurityRequirementPackSummary")`), which
/// restJson1 renders as an ISO-8601 string; a number is read as epoch seconds too, so
/// the case asserts the VALUE rather than one rendering of it.
fn date_time(payload: &Value, name: &str, case: &str) -> f64 {
    let value = payload
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} was reported: {payload}"));
    match value {
        Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|e| {
            panic!("case {case:?}: {name} {text:?} is not an RFC 3339 date-time: {e}")
        })
        .as_secs_f64(),
        Value::Number(number) => number.as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: {name} {number} is not a finite epoch timestamp")
        }),
        other => panic!("case {case:?}: {name} is not a modelled timestamp: {other}"),
    }
}

/// The catalog's publication instant as the epoch seconds every managed pack's stamps
/// must carry.
fn published_at() -> f64 {
    AWS_MANAGED_PACK_PUBLISHED_AT_EPOCH_SECONDS as f64
}

/// RULE: every pack AWS ships reports the fixed description it is shipped with, through
/// the listing and through the read alike.
///
/// The table is `AWS_MANAGED_PACKS` — every catalog entry, including the three the issue
/// did not cite — crossed with the two readers that project a stored pack.
#[tokio::test]
async fn every_managed_pack_reports_its_shipped_description() {
    let router = emulator();
    let summaries = list_summaries(&router, "the seeded catalog").await;
    let mut descriptions: BTreeSet<String> = BTreeSet::new();

    for entry in AWS_MANAGED_PACKS {
        let case = entry.pack_id;
        assert!(
            !entry.description.is_empty(),
            "case {case:?}: the catalog entry carries no description text"
        );
        let summary = summaries
            .iter()
            .find(|summary| member(summary, "packId").as_deref() == Some(entry.pack_id))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing does not report the seeded pack")
            });
        let read = get_pack(&router, entry.pack_id, case).await;

        for (reader, payload) in [("the listing", summary), ("the read", &read)] {
            assert_eq!(
                member(payload, "description").as_deref(),
                Some(entry.description),
                "case {case:?} through {reader}: expected the shipped description {:?}, \
                 got {:?} in {payload}",
                entry.description,
                member(payload, "description")
            );
            assert_eq!(
                member(payload, "managementType").as_deref(),
                Some(MANAGEMENT_TYPE_AWS_MANAGED),
                "case {case:?} through {reader}: expected an \
                 {MANAGEMENT_TYPE_AWS_MANAGED} pack, got {payload}"
            );
        }
        descriptions.insert(entry.description.to_string());
    }

    // Each pack describes ITS OWN content, so no two shipped packs share one text: a
    // single placeholder repeated across the catalog would collapse this set.
    assert_eq!(
        descriptions.len(),
        AWS_MANAGED_PACKS.len(),
        "case the whole catalog: expected {} distinct descriptions, got {descriptions:?}",
        AWS_MANAGED_PACKS.len()
    );
}

/// RULE: both required stamps of every managed pack are the catalog's ONE publication
/// instant — identical across the packs, across the two readers, and across boots,
/// because when AWS published the catalog is not when this account's state was seeded.
///
/// The rows are the catalog entries crossed with three independently booted emulators
/// (each boot seeds a fresh account's state, which is what a restart does) and with both
/// readers.
#[tokio::test]
async fn managed_pack_stamps_are_the_catalogs_publication_instant_on_every_boot() {
    let wanted = published_at();
    let mut seen: BTreeSet<u64> = BTreeSet::new();

    for boot in 0..3 {
        let router = emulator();
        let summaries = list_summaries(&router, &format!("boot {boot}")).await;

        for entry in AWS_MANAGED_PACKS {
            let case = format!("boot {boot} pack {}", entry.pack_id);
            let summary = summaries
                .iter()
                .find(|summary| member(summary, "packId").as_deref() == Some(entry.pack_id))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the listing does not report the seeded pack")
                });
            let read = get_pack(&router, entry.pack_id, &case).await;

            for (reader, payload) in [("the listing", summary), ("the read", &read)] {
                for name in ["createdAt", "updatedAt"] {
                    let got = date_time(payload, name, &case);
                    assert!(
                        (got - wanted).abs() < f64::EPSILON,
                        "case {case:?} through {reader}: expected {name} to be the \
                         catalog's publication instant {wanted}, got {got}"
                    );
                    seen.insert(got as u64);
                }
            }
        }
    }

    // One instant for the whole catalog, on every boot: twelve readings of a per-boot or
    // per-pack clock would be more than one value.
    assert_eq!(
        seen.len(),
        1,
        "case the whole catalog over three boots: expected one shared instant, got {seen:?}"
    );
}

/// RULE: the publication instant belongs to the catalog, not to this run — it is in the
/// past relative to the account's seeding, so a pack the account was created with never
/// looks newly published, and a pack a CALLER creates still carries the clock.
#[tokio::test]
async fn a_created_pack_carries_the_clock_and_a_shipped_one_does_not() {
    let router = emulator();

    let created = expect_status(
        &router,
        "CreateSecurityRequirementPack",
        &json!({"name": "customer-pack", "description": "the caller's own text"}),
        201,
        "a created pack",
    )
    .await;
    let pack_id = member(&created, "packId")
        .unwrap_or_else(|| panic!("case a created pack: no packId was minted: {created}"));
    let summary = summary_of(&router, &pack_id, "a created pack").await;
    let created_at = date_time(&summary, "createdAt", "a created pack");
    assert!(
        created_at > published_at(),
        "case a created pack: expected a pack created now to be stamped after the \
         catalog's publication instant {}, got {created_at}",
        published_at()
    );

    // And the shipped packs, read in the same account at the same moment, are still
    // stamped with the catalog's instant rather than with the create's.
    for entry in AWS_MANAGED_PACKS {
        let case = format!("shipped pack {} beside a created one", entry.pack_id);
        let shipped = get_pack(&router, entry.pack_id, &case).await;
        let got = date_time(&shipped, "createdAt", &case);
        assert!(
            (got - published_at()).abs() < f64::EPSILON,
            "case {case:?}: expected the publication instant {}, got {got}",
            published_at()
        );
    }
}

/// RULE: the publication instant is what a shipped pack carries until something writes
/// the record — a status update (the one write an AWS managed pack takes) moves
/// `updatedAt` off it and leaves `createdAt` where the catalog put it.
///
/// The rows are each catalog entry crossed with the status it is NOT shipped in, so
/// every entry takes a real change.
#[tokio::test]
async fn a_status_update_moves_only_the_updated_stamp_of_a_shipped_pack() {
    let router = emulator();

    for entry in AWS_MANAGED_PACKS {
        let other_status = if entry.status == PACK_STATUS_ENABLED {
            PACK_STATUS_DISABLED
        } else {
            PACK_STATUS_ENABLED
        };
        let case = format!("{} to {other_status}", entry.pack_id);
        expect_status(
            &router,
            "UpdateSecurityRequirementPack",
            &json!({"packId": entry.pack_id, "status": other_status}),
            200,
            &case,
        )
        .await;

        let read = get_pack(&router, entry.pack_id, &case).await;
        let created_at = date_time(&read, "createdAt", &case);
        let updated_at = date_time(&read, "updatedAt", &case);
        assert!(
            (created_at - published_at()).abs() < f64::EPSILON,
            "case {case:?}: an update must leave createdAt at the publication instant \
             {}, got {created_at}",
            published_at()
        );
        assert!(
            updated_at > created_at,
            "case {case:?}: expected updatedAt to move past the publication instant \
             {created_at}, got {updated_at}"
        );
        // The write touched the stamps, not the shipped description.
        assert_eq!(
            member(&read, "description").as_deref(),
            Some(entry.description),
            "case {case:?}: expected the shipped description {:?}, got {read}",
            entry.description
        );
        assert_eq!(
            member(&read, "status").as_deref(),
            Some(other_status),
            "case {case:?}: expected the updated status, got {read}"
        );
    }
}

/// RULE: a listing reports the same catalog content a read does, and reading it changes
/// nothing — two listings of an untouched account agree member for member.
#[tokio::test]
async fn reading_the_catalog_does_not_change_it() {
    let router = emulator();
    let first = list_summaries(&router, "the first listing").await;
    for entry in AWS_MANAGED_PACKS {
        get_pack(&router, entry.pack_id, entry.pack_id).await;
    }
    let again = list_summaries(&router, "a repeated listing").await;
    assert_eq!(
        again, first,
        "case a repeated listing: two reads of the seeded catalog disagreed"
    );

    // The catalog type itself is what the cases above are driven from, so it must name
    // every pack exactly once.
    let ids: BTreeSet<&str> = AWS_MANAGED_PACKS
        .iter()
        .map(|entry: &AwsManagedPack| entry.pack_id)
        .collect();
    assert_eq!(
        ids.len(),
        AWS_MANAGED_PACKS.len(),
        "case the catalog table: expected {} distinct packIds, got {ids:?}",
        AWS_MANAGED_PACKS.len()
    );
}
