// SPDX-License-Identifier: CC0-1.0

//! RULE — a `nextToken` is a continuation value the SERVICE cut, so a paginated read
//! honours one only when it can tell the token apart from a value its caller produced:
//! the cursor travels under an authenticator computed with a secret the store drew from
//! the system's randomness and never reveals, so a value a client assembled — however
//! exactly it is spelled like a cursor, and whatever seal it carries that the caller could
//! compute — names no position at all and is refused with the modelled
//! `ValidationException`, while a cursor this store really cut resumes exactly at the
//! position it carries.
//!
//! Two consequences this suite drives, over every seeded paginated listing of the service:
//!
//! 1. A cursor nobody was issued is refused. The rows span the forms a caller can reach
//!    for: the codec's own payload with a digest over it the caller computes itself, the
//!    payload with no seal, the payload behind an authenticator of the right WIDTH but
//!    made up, and a cursor minted by ANOTHER store of the same emulator — which is what
//!    a token from another process, or from this one before a restart, is. For the SCOPED
//!    listing the same holds with the scope field pointed at another resource, so a cursor
//!    cut for one report cannot be re-pointed at another.
//! 2. A cursor is honoured only in the SPELLING the codec writes. The position is
//!    `usize::to_string`, so decimal with no sign and no leading zero; a payload sealed
//!    under the store's OWN key but carrying `+1` or `0001` is a value the minting side
//!    never wrote, and is refused even though `str::parse` reads it as a number.
//!
//! The authentic half is asserted too, at every position of every listing: the fix is a
//! refusal only for what the service did not cut, not a refusal of its own cursors.
//!
//! The payload layout the assembled rows are built from is the one `src/state.rs`'s cursor
//! codec writes (`artifact|<kind>|<scope>|<position>`, with `-` as the scope of a whole
//! collection and the digest of the resource id as the scope of a sub-collection), and the
//! digest a client can compute over it is SHA-256 over that codec's public domain string.
//! Spelled out here rather than imported, so a case stays a value a CLIENT can produce
//! from public knowledge.

use std::sync::Arc;

use service_com_amazonaws_artifact_artifact::state::{
    customer_agreements_page_token, report_versions_token, reports_page_token, service_state,
    PageTokenKey,
};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{emulator_on, get, Answer, CATALOG};

/// The status a refused `nextToken` is answered with.
const REFUSED_STATUS: u16 = 400;

/// The modelled error a refused `nextToken` travels in.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The service's one fixed invalid-token sentence, spelled out here so a changed message
/// fails this suite.
const INVALID_TOKEN: &str = "The provided token is invalid.";

/// The `ValidationExceptionReason` an invalid token is reported under.
const INVALID_TOKEN_REASON: &str = "invalidToken";

/// The service field of a cursor payload, from the codec in `src/state.rs`.
const CURSOR_SERVICE: &str = "artifact";

/// The scope field of a cursor cut for a WHOLE collection, from the same codec.
const UNSCOPED: &str = "-";

/// The separator between a cursor payload's fields, from the same codec.
const SEPARATOR: char = '|';

/// The public domain string the codec's digest is taken over, from the same codec: a
/// constant, so every input to a digest over it is one a caller holds.
const DIGEST_DOMAIN: &str = "com.amazonaws.artifact#pagination-cursor\n";

/// The width of the authenticator a minted token carries ahead of its payload — the one
/// the shared minter puts there, so an assembled value can be given that exact shape.
const AUTHENTICATOR_BYTES: usize = ::service_common::page_tokens::AUTHENTICATOR_BYTES;

/// One paginated listing of this service, served out of seeded state.
struct Listing {
    /// The operation, as a case label.
    case: &'static str,
    /// Its modelled `@http` URI.
    path: &'static str,
    /// The query this operation needs before `maxResults` and `nextToken`.
    scope: &'static str,
    /// The output member whose list the page cuts.
    items: &'static str,
    /// The cursor kind this operation's own tokens carry, from the codec in
    /// `src/state.rs`.
    kind: &'static str,
    /// The resource a SCOPED listing pages the sub-collection of, or `None` for a whole
    /// collection.
    scoped_to: Option<&'static str>,
}

/// The report whose versions the scoped row pages: the catalog row publishing the most
/// versions, so the walk has several positions to resume at.
const SCOPED_REPORT: &str = "report-5eevElBv6MyRmQsD";

/// Another report of the catalog, for the case that points a cursor at a resource it was
/// not cut for.
const OTHER_REPORT: &str = "report-7g5j2DTVeNXWx7St";

/// The listings under test: a service-owned collection, the sub-collection of ONE resource
/// (the scoped case) and the calling account's own set.
const LISTINGS: &[Listing] = &[
    Listing {
        case: "ListReports",
        path: "/v1/report/list",
        scope: "",
        items: "reports",
        kind: "reports",
        scoped_to: None,
    },
    Listing {
        case: "ListReportVersions",
        path: "/v1/report/listVersions",
        scope: "reportId=report-5eevElBv6MyRmQsD",
        items: "reports",
        kind: "reportVersions",
        scoped_to: Some(SCOPED_REPORT),
    },
    Listing {
        case: "ListCustomerAgreements",
        path: "/v1/customer-agreement/list",
        scope: "",
        items: "customerAgreements",
        kind: "customerAgreements",
        scoped_to: None,
    },
];

/// One emulator, with the engine it keeps its store in, so a case can reach the secret
/// that store seals its cursors under.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (emulator_on(engine.clone()), engine)
}

/// The secret one store seals its pagination cursors under.
fn page_token_key(engine: &engine::InMemoryEngine) -> PageTokenKey {
    service_state(engine)
        .expect("the in-memory engine carries this service's store")
        .lock()
        .page_token_key()
}

/// One page of `listing`, with `query` appended after whatever scope the listing needs.
async fn page(router: &MultiServiceRouter, listing: &Listing, query: &str) -> Answer {
    let mut uri = listing.path.to_string();
    let mut separator = '?';
    for part in [listing.scope, query] {
        if part.is_empty() {
            continue;
        }
        uri.push(separator);
        uri.push_str(part);
        separator = '&';
    }
    get(router, &uri).await
}

/// The ARN of every record on the page — the member every summary this service pages
/// carries that names exactly one record.
fn arns(answer: &Answer, items: &str) -> Vec<String> {
    answer.body[items]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|row| row["arn"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The digest a CLIENT can compute over `value`: SHA-256 over the codec's public domain
/// string and the value, which is every input the old seal had.
fn public_digest(value: &str) -> String {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(DIGEST_DOMAIN.as_bytes());
    hasher.update(value.as_bytes());
    ::hex::encode(hasher.finalize())
}

/// The payload a cursor of `kind` cut for `scope` at `resume_at` carries, in the codec's
/// own field order.
fn payload(kind: &str, scope: Option<&str>, resume_at: &str) -> String {
    let scope = scope.map_or_else(|| UNSCOPED.to_string(), public_digest);
    format!("{CURSOR_SERVICE}{SEPARATOR}{kind}{SEPARATOR}{scope}{SEPARATOR}{resume_at}")
}

/// Every value a client can assemble for `listing` at the position spelled `resume_at`,
/// each labelled by what it is — the seal it carries, or the absence of one. `scope` is
/// the resource the assembled cursor claims to be cut for.
fn assembled(kind: &str, scope: Option<&str>, resume_at: &str) -> Vec<(String, String)> {
    let payload = payload(kind, scope, resume_at);
    let bytes = payload.as_bytes();
    vec![
        (
            "sealed with the digest a caller computes".to_string(),
            ::hex::encode(format!("{payload}{SEPARATOR}{}", public_digest(&payload))),
        ),
        ("with no seal at all".to_string(), ::hex::encode(bytes)),
        (
            "behind an all-zero authenticator".to_string(),
            ::hex::encode([&[0_u8; AUTHENTICATOR_BYTES][..], bytes].concat()),
        ),
        (
            "behind an all-ones authenticator".to_string(),
            ::hex::encode([&[0xff_u8; AUTHENTICATOR_BYTES][..], bytes].concat()),
        ),
    ]
}

/// A cursor of `listing` minted under `key` at `position`, through the very codec the
/// handler mints from.
fn minted(listing: &Listing, key: &PageTokenKey, position: usize) -> String {
    match listing.scoped_to {
        Some(report) => report_versions_token(key, report, position),
        None if listing.kind == "reports" => reports_page_token(key, position),
        None => customer_agreements_page_token(key, position),
    }
}

/// A cursor of `listing` SEALED under `key` but carrying `resume_at` as its position — the
/// way a case reaches a spelling the minting side never writes.
fn sealed_payload(listing: &Listing, key: &PageTokenKey, resume_at: &str) -> String {
    ::hex::encode(key.mint(&payload(listing.kind, listing.scoped_to, resume_at)))
}

/// What every refusal of a `nextToken` asserts: the documented status, the modelled error,
/// the service's one fixed invalid-token sentence with its reason, and NO page at all.
fn assert_refused(case: &str, listing: &Listing, answer: &Answer) {
    assert_eq!(
        answer.status, REFUSED_STATUS,
        "case {case:?}: a value this service did not cut must be refused, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        VALIDATION_EXCEPTION,
        "case {case:?}: expected {VALIDATION_EXCEPTION}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(INVALID_TOKEN),
        "case {case:?}: expected the fixed invalid-token sentence, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["reason"].as_str(),
        Some(INVALID_TOKEN_REASON),
        "case {case:?}: expected the {INVALID_TOKEN_REASON} reason, got {:?}",
        answer.body
    );
    assert!(
        arns(answer, listing.items).is_empty(),
        "case {case:?}: a refused token must serve no record, got {:?}",
        answer.body
    );
}

/// The whole reported order of `listing`, from one unpaged call: what every resumed page
/// below is checked against.
async fn whole(router: &MultiServiceRouter, listing: &Listing) -> Vec<String> {
    let answer = page(router, listing, "").await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: an unpaged listing must be served, got {} with {:?}",
        listing.case, answer.status, answer.body
    );
    let records = arns(&answer, listing.items);
    assert!(
        records.len() > 1,
        "case {:?}: the rule needs several records to resume between, got {records:?}",
        listing.case
    );
    records
}

#[tokio::test]
async fn the_scoped_row_names_reports_the_catalog_publishes() {
    for (case, id, least_versions) in [
        ("the scoped report", SCOPED_REPORT, 2),
        ("the other report", OTHER_REPORT, 1),
    ] {
        let row = CATALOG
            .iter()
            .find(|row| row.id == id)
            .unwrap_or_else(|| panic!("case {case:?}: the catalog must publish {id}"));
        assert!(
            row.versions.len() >= least_versions,
            "case {case:?}: expected at least {least_versions} versions, got {:?}",
            row.versions
        );
    }
}

#[tokio::test]
async fn a_cursor_a_client_assembled_is_refused_wherever_it_points() {
    let (router, engine) = emulator_with_engine();
    // Another store of the same emulator: its key is as good as any, and still not this
    // store's — which is what a token from another process or a restarted one is.
    let another_store = PageTokenKey::new();

    for listing in LISTINGS {
        let records = whole(&router, listing).await;
        // The positions a caller would reach for: the start, the second record, the
        // middle, the end of the collection and one past it.
        let positions = [0, 1, records.len() / 2, records.len(), records.len() + 1];
        for position in positions {
            let mut rows = assembled(listing.kind, listing.scoped_to, &position.to_string());
            rows.push((
                "minted by another store at the same position".to_string(),
                minted(listing, &another_store, position),
            ));
            for (what, token) in rows {
                let case = format!("{} given a cursor {what} at {position}", listing.case);
                let answer = page(&router, listing, &format!("nextToken={token}")).await;
                assert_refused(&case, listing, &answer);
            }
        }
    }

    // The SCOPED listing, with the scope field pointed at a resource the cursor was not
    // cut for: neither an assembled scope nor this store's own cursor for another report
    // resumes the report in the request.
    let key = page_token_key(&engine);
    let scoped = LISTINGS
        .iter()
        .find(|listing| listing.scoped_to.is_some())
        .expect("one listing pages the sub-collection of a resource");
    let mut rows = assembled(scoped.kind, Some(OTHER_REPORT), "1");
    rows.push((
        "this store's own cursor cut for another report".to_string(),
        report_versions_token(&key, OTHER_REPORT, 1),
    ));
    for (what, token) in rows {
        let case = format!("{} given a cursor {what}", scoped.case);
        let answer = page(&router, scoped, &format!("nextToken={token}")).await;
        assert_refused(&case, scoped, &answer);
    }
}

#[tokio::test]
async fn a_position_spelled_as_the_codec_never_writes_it_is_refused() {
    let (router, engine) = emulator_with_engine();
    let key = page_token_key(&engine);

    // Sealed under THIS store's key, so nothing but the SPELLING of the position can
    // refuse them. Every row is a spelling `str::parse::<usize>` reads as a number, or as
    // nothing, while `usize::to_string` writes none of them.
    let spellings = [
        "+1", "+0", "01", "0001", "00", " 1", "1 ", "1_0", "0x1", "1.0", "-1", "", "one",
    ];
    for listing in LISTINGS {
        for spelling in spellings {
            let case = format!("{} given the position {spelling:?}", listing.case);
            let token = sealed_payload(listing, &key, spelling);
            let answer = page(&router, listing, &format!("nextToken={token}")).await;
            assert_refused(&case, listing, &answer);
        }
    }
}

#[tokio::test]
async fn a_cursor_this_store_cut_resumes_at_the_position_it_carries() {
    let (router, engine) = emulator_with_engine();
    let key = page_token_key(&engine);

    for listing in LISTINGS {
        let records = whole(&router, listing).await;
        // Every position of the collection, including one past the last record — which
        // serves an empty page rather than a refusal.
        for position in 0..=records.len() {
            let case = format!("{} resumed at {position}", listing.case);
            let token = minted(listing, &key, position);
            let answer = page(&router, listing, &format!("nextToken={token}")).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a cursor this store cut must be served, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                arns(&answer, listing.items),
                records[position..].to_vec(),
                "case {case:?}: expected the records after the cursor, got {:?}",
                answer.body
            );
        }
    }
}
