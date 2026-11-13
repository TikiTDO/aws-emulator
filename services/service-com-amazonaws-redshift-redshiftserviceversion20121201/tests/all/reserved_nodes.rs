// SPDX-License-Identifier: CC0-1.0

//! The reservations `DescribeReservedNodes` reports, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns rather than keeping a second copy of it.
//!
//! The suite asserts the RULE rather than one recorded request: this operation
//! reports the reservations the account HOLDS — the ones
//! `PurchaseReservedNodeOffering` bought, in purchase order — each carrying its
//! offering's dimensions and per-node charges verbatim plus what belongs to the
//! reservation alone (its minted id, when its term started, how many nodes it
//! holds and the state of its payment); `ReservedNodeId` selects the one
//! reservation it names and an id the account holds none under is refused; the
//! shared `MaxRecords` window and `Marker` cursor paginate the list; and the only
//! change a read makes is the payment confirmation the clock already owed.
//!
//! Each table row differs along what the rule generalises over: which offering is
//! bought (node type, term, payment option), how many nodes the purchase reserves,
//! how many reservations the account holds, and how the page is asked for.
//!
//! The helpers this suite owns are `pub(crate)` so the sibling reserved-node
//! EXCHANGE suite reads the offering catalog, buys a reservation and polls a
//! clock-owed state change through this one copy of them.

use std::time::Instant;

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, element_blocks, Answer, Emulator};

/// The members a reservation carries VERBATIM from the offering it was bought
/// from, as the awsQuery XML names them.
///
/// Source: the members `ReservedNode` and `ReservedNodeOffering` have in common
/// (`smithy_query(query="shape", shape="ReservedNode,ReservedNodeOffering")`),
/// the recurring-charge entries among them. Everything else a reservation
/// reports — its id, start time, node count and payment state — belongs to the
/// reservation alone and is asserted separately.
const OFFERING_MEMBERS: [&str; 10] = [
    "ReservedNodeOfferingId",
    "NodeType",
    "Duration",
    "FixedPrice",
    "UsagePrice",
    "CurrencyCode",
    "OfferingType",
    "RecurringChargeAmount",
    "RecurringChargeFrequency",
    "ReservedNodeOfferingType",
];

/// The shorter published term, in seconds: a one-year commitment of 365-day
/// years, as the offering catalog publishes it.
pub(crate) const ONE_YEAR: &str = "31536000";

/// The longer published term, in seconds: a three-year commitment of the same
/// years.
pub(crate) const THREE_YEARS: &str = "94608000";

/// How long a poll loop is given before the suite calls the clock-owed payment
/// confirmation unobservable, and the gap between two polls of one loop: the
/// harness's, so every suite of this crate waits on one set of bounds.
///
/// Re-exported for the sibling exchange suite, which polls through the helper
/// below and reports the same bounds in its own messages.
pub(crate) use crate::cluster_lifecycle::{POLL_DEADLINE, POLL_INTERVAL};

/// The value of `<tag>` in one served record, asserting the record carries
/// exactly one.
pub(crate) fn one(case: &str, block: &str, tag: &str) -> String {
    let found = all_text(block, tag);
    assert_eq!(
        found.len(),
        1,
        "case {case}: expected exactly one <{tag}> per record, got {found:?} in {block}"
    );
    found.into_iter().next().unwrap_or_default()
}

/// Every offering the catalog publishes, as the inner XML of its element.
pub(crate) async fn catalog(emulator: &Emulator) -> Vec<String> {
    let answer = emulator.call("DescribeReservedNodeOfferings", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case the offering catalog: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let offerings = element_blocks(&answer.body, "ReservedNodeOffering");
    assert!(
        !offerings.is_empty(),
        "case the offering catalog: expected published offerings, got body {}",
        answer.body
    );
    offerings
}

/// The one published offering of this node type, term and offering type.
pub(crate) fn offering_of(
    case: &str,
    catalog: &[String],
    node_type: &str,
    duration: &str,
    offering_type: &str,
) -> String {
    let found: Vec<&String> = catalog
        .iter()
        .filter(|block| {
            one(case, block, "NodeType") == node_type
                && one(case, block, "Duration") == duration
                && one(case, block, "OfferingType") == offering_type
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case}: expected one {node_type} / {duration}s / {offering_type} offering, got {found:?}"
    );
    found[0].clone()
}

/// Buy `node_count` nodes of the offering this block describes, answering the
/// response as served.
pub(crate) async fn purchase(
    emulator: &Emulator,
    offering_block: &str,
    node_count: Option<i32>,
    case: &str,
) -> Answer {
    let offering_id = one(case, offering_block, "ReservedNodeOfferingId");
    let count = node_count.map(|count| count.to_string());
    let mut params: Vec<(&str, &str)> = vec![("ReservedNodeOfferingId", offering_id.as_str())];
    if let Some(count) = count.as_deref() {
        params.push(("NodeCount", count));
    }
    emulator
        .call("PurchaseReservedNodeOffering", &params)
        .await
}

/// The reservations one `DescribeReservedNodes` call reports, asserting the call
/// succeeded.
async fn described(emulator: &Emulator, case: &str, params: &[(&str, &str)]) -> (Answer, Vec<String>) {
    let answer = emulator.call("DescribeReservedNodes", params).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} / {:?} with body {}",
        answer.status,
        answer.error_code(),
        answer.body
    );
    let blocks = element_blocks(&answer.body, "ReservedNode");
    (answer, blocks)
}

/// The one reservation the id names, asserting the read reports exactly one.
pub(crate) async fn reservation(emulator: &Emulator, case: &str, reserved_node_id: &str) -> String {
    let (_, blocks) = described(emulator, case, &[("ReservedNodeId", reserved_node_id)]).await;
    assert_eq!(
        blocks.len(),
        1,
        "case {case}: expected the one reservation {reserved_node_id} names, got {blocks:?}"
    );
    blocks[0].clone()
}

/// Poll `DescribeReservedNodes` for this reservation until its `State` is
/// `wanted`, answering the block it held on and how many polls that took; panics
/// quoting the last block when the deadline passes first.
///
/// This loop makes NO mutating call, so only a transition the clock owes can end
/// it.
pub(crate) async fn poll_until_state(
    emulator: &Emulator,
    case: &str,
    reserved_node_id: &str,
    wanted: &str,
) -> (String, usize) {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut polls = 0usize;
    let mut last = String::new();
    while Instant::now() < deadline {
        let block = reservation(emulator, case, reserved_node_id).await;
        polls += 1;
        if one(case, &block, "State") == wanted {
            return (block, polls);
        }
        last = block;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case}: the reservation never reached {wanted} within {POLL_DEADLINE:?} \
         ({polls} polls); last read: {last}"
    );
}

/// What a purchase records is what a later read reports, for every shape of
/// purchase.
#[tokio::test]
async fn a_purchase_is_reported_by_a_later_describe() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;

    // Before anything is bought the account holds no reservation: this operation
    // reports what was PURCHASED, never the offerings that are purchasable.
    let (_, none_held) = described(&emulator, "before any purchase", &[]).await;
    assert!(
        none_held.is_empty(),
        "case before any purchase: expected no reservation, got {none_held:?}"
    );

    // Rows differ along which offering is bought — node type, term and payment
    // option — and along how many nodes the purchase reserves, including the
    // request that names no `NodeCount` at all.
    let rows: [(&str, &str, &str, &str, Option<i32>); 5] = [
        (
            "the smallest node, one year, nothing upfront, no NodeCount",
            "dc2.large",
            ONE_YEAR,
            "No Upfront",
            None,
        ),
        (
            "a fully-upfront offering of the largest dense-compute node",
            "dc2.8xlarge",
            ONE_YEAR,
            "All Upfront",
            Some(3),
        ),
        (
            "the largest node for three years",
            "ra3.16xlarge",
            THREE_YEARS,
            "Partial Upfront",
            Some(10),
        ),
        (
            "a mid-sized node for three years",
            "ra3.xlplus",
            THREE_YEARS,
            "No Upfront",
            Some(1),
        ),
        (
            "a reserve-group node, one year, part upfront",
            "rg.large",
            ONE_YEAR,
            "Partial Upfront",
            Some(2),
        ),
    ];

    let mut bought: Vec<(String, i32)> = Vec::new();
    for (case, node_type, duration, offering_type, node_count) in rows {
        let offering = offering_of(case, &offerings, node_type, duration, offering_type);
        let answer = purchase(&emulator, &offering, node_count, case).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected PurchaseReservedNodeOffering 200, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
        let purchased = element_blocks(&answer.body, "ReservedNode");
        assert_eq!(
            purchased.len(),
            1,
            "case {case}: expected the one reservation the purchase recorded, got {purchased:?}"
        );
        let purchased = purchased[0].clone();

        // The reservation carries its offering's dimensions and charges verbatim:
        // this is what makes a reservation and the offering it was bought from
        // agree about the same purchase.
        for member in OFFERING_MEMBERS {
            assert_eq!(
                all_text(&purchased, member),
                all_text(&offering, member),
                "case {case}: expected <{member}> taken from the offering, got {:?} against {:?}",
                all_text(&purchased, member),
                all_text(&offering, member)
            );
        }

        // What belongs to the reservation alone.
        let expected_count = node_count.unwrap_or(state::DEFAULT_RESERVED_NODE_COUNT);
        assert_eq!(
            one(case, &purchased, "NodeCount"),
            expected_count.to_string(),
            "case {case}: expected {expected_count} reserved node(s), got {purchased}"
        );
        assert_eq!(
            one(case, &purchased, "State"),
            state::RESERVED_NODE_STATE_PENDING_PAYMENT,
            "case {case}: expected a purchase to leave the payment unconfirmed, got {purchased}"
        );
        let start_time = one(case, &purchased, "StartTime");
        assert!(
            !start_time.is_empty(),
            "case {case}: expected the term's start time, got {purchased}"
        );
        // The id the service minted for the reservation: the 8-4-4-4-12
        // lowercase-hex form, and one id per reservation.
        let reserved_node_id = one(case, &purchased, "ReservedNodeId");
        let groups: Vec<usize> = reserved_node_id.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "case {case}: expected an 8-4-4-4-12 ReservedNodeId, got {reserved_node_id:?}"
        );
        assert!(
            reserved_node_id
                .chars()
                .all(|c| c == '-' || c.is_ascii_hexdigit()),
            "case {case}: expected a hexadecimal ReservedNodeId, got {reserved_node_id:?}"
        );

        // The round trip: the record the purchase returned is the record a read
        // reports, down to the start time — only the payment state moves, and
        // only the way the clock owes.
        let read = reservation(&emulator, case, &reserved_node_id).await;
        for member in OFFERING_MEMBERS
            .into_iter()
            .chain(["ReservedNodeId", "StartTime", "NodeCount"])
        {
            assert_eq!(
                all_text(&read, member),
                all_text(&purchased, member),
                "case {case}: expected <{member}> unchanged by a read, got {:?} against {:?}",
                all_text(&read, member),
                all_text(&purchased, member)
            );
        }
        let (settled, polls) = poll_until_state(
            &emulator,
            case,
            &reserved_node_id,
            state::RESERVED_NODE_STATE_ACTIVE,
        )
        .await;
        for member in OFFERING_MEMBERS
            .into_iter()
            .chain(["ReservedNodeId", "StartTime", "NodeCount"])
        {
            assert_eq!(
                all_text(&settled, member),
                all_text(&purchased, member),
                "case {case}: expected <{member}> unchanged after {polls} poll(s), got {:?} \
                 against {:?}",
                all_text(&settled, member),
                all_text(&purchased, member)
            );
        }

        bought.push((reserved_node_id, expected_count));
    }

    // Every reservation the account holds, in purchase order, each exactly once.
    let (_, held) = described(&emulator, "every reservation", &[]).await;
    let ids: Vec<String> = held
        .iter()
        .map(|block| one("every reservation", block, "ReservedNodeId"))
        .collect();
    let expected_ids: Vec<String> = bought.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(
        ids, expected_ids,
        "case every reservation: expected the purchases in order, got {ids:?}"
    );
    let counts: Vec<String> = held
        .iter()
        .map(|block| one("every reservation", block, "NodeCount"))
        .collect();
    let expected_counts: Vec<String> = bought
        .iter()
        .map(|(_, count)| count.to_string())
        .collect();
    assert_eq!(
        counts, expected_counts,
        "case every reservation: expected each purchase's node count, got {counts:?}"
    );
}

/// `ReservedNodeId` names ONE reservation, and an id the account holds none
/// under is a refusal rather than an empty page.
#[tokio::test]
async fn an_id_selects_the_one_reservation_it_names() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;

    // An id is refused the same way whether or not the account holds anything at
    // all, so the first row runs against an empty account.
    let before = emulator
        .call("DescribeReservedNodes", &[("ReservedNodeId", "reservation-1")])
        .await;
    assert_eq!(
        (before.status, before.error_code()),
        (404, Some("ReservedNodeNotFound".to_string())),
        "case an id before any purchase: expected a 404 refusal, got {} / {:?} with body {}",
        before.status,
        before.error_code(),
        before.body
    );

    // Two reservations of DIFFERENT offerings and two of the SAME one: buying an
    // offering twice is two reservations, so an id still names exactly one.
    let first = offering_of(
        "the first offering",
        &offerings,
        "dc2.large",
        ONE_YEAR,
        "No Upfront",
    );
    let second = offering_of(
        "a three-year offering",
        &offerings,
        "ra3.4xlarge",
        THREE_YEARS,
        "All Upfront",
    );
    let mut held: Vec<String> = Vec::new();
    for (case, offering, node_count) in [
        ("one node of the first offering", &first, Some(1)),
        ("four nodes of the first offering again", &first, Some(4)),
        ("two nodes of a three-year offering", &second, Some(2)),
    ] {
        let answer = purchase(&emulator, offering, node_count, case).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected PurchaseReservedNodeOffering 200, got {} with body {}",
            answer.status, answer.body
        );
        held.push(
            answer
                .text("ReservedNodeId")
                .unwrap_or_else(|| panic!("case {case}: expected a ReservedNodeId")),
        );
    }
    assert_eq!(
        held.iter().collect::<std::collections::BTreeSet<_>>().len(),
        held.len(),
        "case three purchases: expected three distinct ids, got {held:?}"
    );

    for (position, reserved_node_id) in held.iter().enumerate() {
        let case = format!("the reservation bought {}", position + 1);
        let block = reservation(&emulator, &case, reserved_node_id).await;
        assert_eq!(
            one(&case, &block, "ReservedNodeId"),
            *reserved_node_id,
            "case {case}: expected just the reservation the id names, got {block}"
        );
    }

    // An id the account holds no reservation under names nothing: "The specified
    // reserved compute node not found" (HTTP 404), not an empty list. Rows differ
    // along HOW the id misses: never minted, well formed but unheld, and a held
    // id altered in case, length or surrounding space.
    let a_held = held[0].clone();
    let uppercased = a_held.to_uppercase();
    let truncated = a_held[..a_held.len() - 1].to_string();
    let spaced = format!(" {a_held} ");
    for (case, reserved_node_id) in [
        ("an id nobody minted", "reservation-1"),
        (
            "a well-formed id nobody holds",
            "00000000-0000-0000-0000-000000000000",
        ),
        ("a held id in upper case", uppercased.as_str()),
        ("a held id missing its last character", truncated.as_str()),
        ("a held id with surrounding space", spaced.as_str()),
    ] {
        let answer = emulator
            .call(
                "DescribeReservedNodes",
                &[("ReservedNodeId", reserved_node_id)],
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, Some("ReservedNodeNotFound".to_string())),
            "case {case} ({reserved_node_id:?}): expected a 404 refusal, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }
}

/// The documented `MaxRecords` window and the `Marker` cursor govern the page.
#[tokio::test]
async fn the_documented_max_records_window_and_marker_cursor_govern_the_page() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;
    let offering = offering_of(
        "the paged offering",
        &offerings,
        "ra3.large",
        ONE_YEAR,
        "No Upfront",
    );

    // One more reservation than the smallest page the window allows, so a page is
    // truncated and the cursor has somewhere to resume.
    let total = state::MAX_RECORDS_MIN as usize + 1;
    let mut expected_ids: Vec<String> = Vec::new();
    for position in 0..total {
        let case = format!("purchase {}", position + 1);
        let answer = purchase(&emulator, &offering, Some(1), &case).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected PurchaseReservedNodeOffering 200, got {} with body {}",
            answer.status, answer.body
        );
        expected_ids.push(
            answer
                .text("ReservedNodeId")
                .unwrap_or_else(|| panic!("case {case}: expected a ReservedNodeId")),
        );
    }

    // "Default: 100 Constraints: minimum 20, maximum 100." — each published
    // bound, and one past each; only the minimum is enforced.
    for (case, max_records, accepted) in [
        ("one below the minimum", state::MAX_RECORDS_MIN - 1, false),
        ("at the minimum", state::MAX_RECORDS_MIN, true),
        ("inside the window", state::MAX_RECORDS_MIN + 1, true),
        ("at the maximum", state::MAX_RECORDS_MAX, true),
        ("one above the maximum", state::MAX_RECORDS_MAX + 1, true),
        ("zero", 0, false),
    ] {
        let value = max_records.to_string();
        let answer = emulator
            .call("DescribeReservedNodes", &[("MaxRecords", &value)])
            .await;
        assert_eq!(
            answer.status == 200,
            accepted,
            "case MaxRecords {case} ({value}): expected accepted={accepted}, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
        if accepted {
            let page = element_blocks(&answer.body, "ReservedNode");
            assert!(
                page.len() <= max_records as usize,
                "case MaxRecords {case} ({value}): expected at most {max_records} reservations, \
                 got {}",
                page.len()
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case MaxRecords {case} ({value}): expected a 400 refusal, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // Walking the cursor reports every reservation, in order, exactly once.
    let page_size = state::MAX_RECORDS_MIN.to_string();
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut params: Vec<(&str, &str)> = vec![("MaxRecords", page_size.as_str())];
        if let Some(cursor) = marker.as_deref() {
            params.push(("Marker", cursor));
        }
        let (answer, page) = described(&emulator, "a walked page", &params).await;
        assert!(
            page.len() <= state::MAX_RECORDS_MIN as usize,
            "case page {pages}: expected at most {} reservations, got {}",
            state::MAX_RECORDS_MIN,
            page.len()
        );
        walked.extend(
            page.iter()
                .map(|block| one("a walked page", block, "ReservedNodeId")),
        );
        pages += 1;
        assert!(
            pages <= total,
            "case marker walk: the cursor must terminate, got {pages} pages"
        );
        marker = answer.text("Marker");
        if marker.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, expected_ids,
        "case marker walk: expected every reservation across {pages} page(s), got {walked:?}"
    );
    assert!(
        pages > 1,
        "case marker walk: the reservations must span more than one smallest page, got {pages}"
    );

    // A cursor nobody handed out is refused rather than served as an offset.
    for (case, cursor) in [
        ("not a number", "not-a-cursor"),
        ("an invented offset", "7"),
        ("an offset with a forged tag", "20.00000000"),
    ] {
        let bogus = emulator
            .call("DescribeReservedNodes", &[("Marker", cursor)])
            .await;
        assert_eq!(
            (bogus.status, bogus.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case marker {case} ({cursor:?}): expected a 400 refusal, got {} / {:?}",
            bogus.status,
            bogus.error_code()
        );
    }
}

/// A purchase the service cannot serve changes nothing.
#[tokio::test]
async fn a_purchase_the_service_cannot_serve_is_refused() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;
    let offering = offering_of(
        "the probe offering",
        &offerings,
        "rg.xlarge",
        ONE_YEAR,
        "All Upfront",
    );
    let offering_id = one("the probe offering", &offering, "ReservedNodeOfferingId");

    // An offering id the catalog does not publish names nothing to buy:
    // "Specified offering does not exist" (HTTP 404). Rows differ along how the
    // id misses, exactly as the offerings describe refuses it.
    let uppercased = offering_id.to_uppercase();
    let truncated = offering_id[..offering_id.len() - 1].to_string();
    for (case, named) in [
        ("an id nobody published", "offering-1"),
        (
            "a well-formed id nobody published",
            "00000000-0000-0000-0000-000000000000",
        ),
        ("a published id in upper case", uppercased.as_str()),
        ("a published id missing its last character", truncated.as_str()),
    ] {
        let answer = emulator
            .call(
                "PurchaseReservedNodeOffering",
                &[("ReservedNodeOfferingId", named)],
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, Some("ReservedNodeOfferingNotFound".to_string())),
            "case {case} ({named:?}): expected a 404 refusal, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }

    // A count that reserves no node at all describes no purchase.
    for (case, node_count) in [("zero nodes", "0"), ("a negative count", "-1")] {
        let answer = emulator
            .call(
                "PurchaseReservedNodeOffering",
                &[
                    ("ReservedNodeOfferingId", offering_id.as_str()),
                    ("NodeCount", node_count),
                ],
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, Some("ValidationException".to_string())),
            "case {case} ({node_count}): expected a 400 refusal, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }

    // Nothing a refusal touched is held: the account still has no reservation.
    let (_, held) = described(&emulator, "after the refusals", &[]).await;
    assert!(
        held.is_empty(),
        "case after the refusals: expected no reservation recorded, got {held:?}"
    );
}

/// Reading the reservations changes nothing a later caller sees beyond the
/// payment confirmation the clock already owed.
#[tokio::test]
async fn reading_the_reservations_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let offerings = catalog(&emulator).await;
    let offering = offering_of(
        "the read offering",
        &offerings,
        "rg.12xlarge",
        THREE_YEARS,
        "Partial Upfront",
    );

    let bought = purchase(&emulator, &offering, Some(2), "the read purchase").await;
    assert_eq!(
        bought.status, 200,
        "case the read purchase: expected 200, got {} with body {}",
        bought.status, bought.body
    );
    let reserved_node_id = bought
        .text("ReservedNodeId")
        .expect("the purchase reports a ReservedNodeId");

    // Once the confirmation the clock owed has been materialised, every later
    // read is the same read: nothing a report does moves the record on.
    let (settled, _) = poll_until_state(
        &emulator,
        "the settled reservation",
        &reserved_node_id,
        state::RESERVED_NODE_STATE_ACTIVE,
    )
    .await;
    for pass in 1..=3 {
        let again = reservation(&emulator, "a repeated read", &reserved_node_id).await;
        assert_eq!(
            again, settled,
            "case repeated read {pass}: expected a stable record, got {again}"
        );
    }

    // A read neither mints a reservation nor consumes an offering: the store
    // holds the one purchase and the published catalog untouched.
    let (_, held) = described(&emulator, "the whole list", &[]).await;
    assert_eq!(
        held.len(),
        1,
        "case the whole list: expected the one reservation bought, got {held:?}"
    );
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let (reservations, published) = {
        let store = store.lock();
        (store.reserved_nodes.len(), store.reserved_node_offerings.len())
    };
    assert_eq!(
        reservations, 1,
        "case the store after the reads: expected one reservation held, got {reservations}"
    );
    assert_eq!(
        published,
        state::published_reserved_node_offerings().len(),
        "case the store after the reads: expected the published catalog untouched, got \
         {published} offering(s)"
    );
}
