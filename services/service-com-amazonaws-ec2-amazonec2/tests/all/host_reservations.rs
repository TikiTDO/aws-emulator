// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Dedicated Host RESERVATION plane — `PurchaseHostReservation` and
//! `DescribeHostReservations` — driven over the wire the service actually speaks (`ec2Query`,
//! through the crate's shared [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. A PURCHASE IS A WRITE THE DESCRIBE REPORTS: "this action results in the specified
//!    reservation being purchased and charged to your account", so one purchase stores ONE
//!    reservation covering the whole `HostIdSet`, carrying the published offering's term,
//!    instance family, payment option and prices, with `Count` the hosts it covers and `End`
//!    its `Start` plus that term — and every host it covers carries its `HostReservationId`
//!    from then on;
//! 2. ITS STATE FOLLOWS THE CLOCK: the charge has not settled when the call answers, so the
//!    reservation is `payment-pending` straight after the purchase and `active` one
//!    [`ec2::state::TRANSITION_DELAY`] later, whether or not anybody called in between;
//! 3. THE DESCRIBE SELECTS, AND WRITES NOTHING: `HostReservationIdSet` and the filters the
//!    operation documents narrow what is reported, an undocumented filter is refused, and two
//!    identical calls answer the same thing;
//! 4. IT PAGES in reservation-id order, `MaxResults` inside the published 5..500 range and
//!    `NextToken` naming where the next page starts;
//! 5. AN ID IT CANNOT RESOLVE IS REFUSED `InvalidHostReservationId.Malformed`, naming every
//!    such id, rather than reported as an absent row;
//! 6. A PURCHASE IT CANNOT MAKE IS REFUSED: a host the account does not hold active, an
//!    offering that is not the hosts' own, and a `LimitPrice` below the total upfront cost each
//!    buy nothing;
//! 7. `ClientToken` MAKES IT IDEMPOTENT: a repeat of one purchase reports the reservation it
//!    already bought and charges the account for no second one.
//!
//! Each table row is a distinct input along what its rule generalises over — the instance
//! family, the payment option and the term of the offering, how many hosts one reservation
//! covers, which filter narrows the answer, the page size, and every way an id, an offering or
//! a limit price can fail — so no row is the specification.

use std::time::Duration;

use ec2::ops::describe_host_reservation_offerings::{
    HOST_OFFERING_CURRENCY_CODE, HostOfferingRecord, format_price_milli, host_offering_by_id,
    parse_price_milli,
};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, indexed, member};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The zone the hosts these suites allocate sit in.
const ZONE: &str = "us-east-1a";

/// The one-year term, in seconds: "reservations are available in one-year and three-year terms
/// […] the number of seconds specified must be the number of seconds in a year (365x24x60x60)
/// times one of the supported durations (1 or 3)"
/// (`DescribeHostReservationOfferingsRequest$MinDuration`).
const ONE_YEAR_SECONDS: i32 = 365 * 24 * 60 * 60;

/// A reservation id inside the form EC2 issues that this account has bought no reservation
/// under.
const UNHELD_RESERVATION_ID: &str = "hr-0123456789abcdef0";

/// Long enough past [`ec2::state::TRANSITION_DELAY`] that the settlement the clock owes a fresh
/// purchase is certainly due.
fn past_the_transition() -> Duration {
    ec2::state::TRANSITION_DELAY + Duration::from_millis(250)
}

/// One tag a purchase applies to the reservation it buys.
type TagRow = (&'static str, &'static str);

/// One row of the purchase table: the case, the instance family, the payment option, the term
/// in seconds, how many hosts the one reservation covers, and the tags the purchase applies.
type PurchaseRow = (
    &'static str,
    &'static str,
    &'static str,
    i32,
    usize,
    &'static [TagRow],
);

/// One reservation a selection suite buys up front: its instance family, its payment option and
/// the tags it carries.
type ReservationPlan = (&'static str, &'static str, &'static [TagRow]);

/// One row of the purchase-refusal table: the case, the host ids the request names, the
/// offering it names, the extra form fields it carries, and the error code it earns.
type RefusalRow<'a> = (&'a str, Vec<&'a str>, &'a str, Vec<Param>, &'a str);

/// Allocate `quantity` Dedicated Hosts of `instance_family`, answering their ids.
async fn allocate(
    router: &MultiServiceRouter,
    instance_family: &str,
    quantity: usize,
) -> Vec<String> {
    let params = vec![
        param("AvailabilityZone", ZONE),
        param("InstanceFamily", instance_family),
        param("Quantity", &quantity.to_string()),
    ];
    let (status, body) = call(router, REGION, "AllocateHosts", &params).await;
    assert_eq!(
        status, 200,
        "AllocateHosts must succeed, got {status} with body {body}"
    );
    let ids = elements(&body, "item");
    assert_eq!(
        ids.len(),
        quantity,
        "AllocateHosts must report the {quantity} host(s) it allocated: {body}"
    );
    ids
}

/// The id of a published offering for `instance_family` on `payment_option` whose term is
/// `duration_seconds` — read out of the catalogue the describe publishes, so this suite names
/// no offering id of its own.
async fn published_offering(
    router: &MultiServiceRouter,
    instance_family: &str,
    payment_option: &str,
    duration_seconds: i32,
) -> String {
    let mut params = vec![
        param("MinDuration", &duration_seconds.to_string()),
        param("MaxDuration", &duration_seconds.to_string()),
    ];
    params.extend(super::ec2_query::filters(&[
        ("instance-family", &[instance_family]),
        ("payment-option", &[payment_option]),
    ]));
    let (status, body) = call(router, REGION, "DescribeHostReservationOfferings", &params).await;
    assert_eq!(
        status, 200,
        "DescribeHostReservationOfferings must succeed, got {status} with body {body}"
    );
    elements(&body, "offeringId")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the catalogue must publish an offering for {instance_family}"))
}

/// One page of the published offering catalogue, whatever it holds.
async fn described_offerings(router: &MultiServiceRouter) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeHostReservationOfferings",
        &[param("MaxResults", "5")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeHostReservationOfferings must succeed, got {status} with body {body}"
    );
    body
}

/// One `PurchaseHostReservation` call naming `host_ids` and `offering_id`, with `extra` form
/// fields beside them.
async fn purchase(
    router: &MultiServiceRouter,
    host_ids: &[&str],
    offering_id: &str,
    extra: &[Param],
) -> (u16, String) {
    let mut params: Vec<Param> = indexed("HostIdSet", host_ids);
    params.push(param("OfferingId", offering_id));
    params.extend(extra.iter().cloned());
    call(router, REGION, "PurchaseHostReservation", &params).await
}

/// One purchase that must be served, answering its response body.
async fn purchased(
    router: &MultiServiceRouter,
    case: &str,
    host_ids: &[&str],
    offering_id: &str,
    extra: &[Param],
) -> String {
    let (status, body) = purchase(router, host_ids, offering_id, extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PurchaseHostReservation must succeed, got {status}: {body}"
    );
    body
}

/// One `DescribeHostReservations` call with `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "DescribeHostReservations", params).await
}

/// One describe that must be served.
async fn described(router: &MultiServiceRouter, case: &str, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeHostReservations must succeed, got {status}: {body}"
    );
    body
}

/// The reservation ids one describe body reports, in the order it reported them.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "hostReservationId")
}

/// How many reservations the store holds — read through the service's own state accessor, so
/// "nothing was written" is asserted against the store rather than against a response.
fn stored_reservations(engine: &engine::InMemoryEngine) -> usize {
    let state = engine.state::<Ec2State>();
    let store = state.lock();
    store.host_reservations.len()
}

/// The published offering the catalogue holds under `offering_id`.
fn offering(case: &str, offering_id: &str) -> &'static HostOfferingRecord {
    host_offering_by_id(offering_id)
        .unwrap_or_else(|| panic!("case {case:?}: the catalogue published {offering_id}"))
}

/// The `HostReservationId` of the one host the `DescribeHosts` body reports for `host_id`.
async fn host_reservation_of(router: &MultiServiceRouter, host_id: &str) -> Option<String> {
    let (status, body) = call(router, REGION, "DescribeHosts", &indexed("HostId", &[host_id])).await;
    assert_eq!(
        status, 200,
        "DescribeHosts for {host_id} must succeed, got {status}: {body}"
    );
    element(&body, "hostReservationId")
}

/// RULE 1 and RULE 2: one purchase stores one reservation the describe reports, with the
/// offering's own terms and prices over the hosts it covers, and its state follows the clock.
#[tokio::test]
async fn a_purchase_is_reported_by_the_describe() {
    // Rows differ in the instance family, the payment option (which decides how the term's one
    // total is split between upfront and hourly), the term, how many hosts the reservation
    // covers, and the tags the purchase applies to it.
    let rows: [PurchaseRow; 4] = [
        (
            "one m5 host, All Upfront, one year, untagged",
            "m5",
            "AllUpfront",
            ONE_YEAR_SECONDS,
            1,
            &[],
        ),
        (
            "two m5 hosts, No Upfront, one year, one tag",
            "m5",
            "NoUpfront",
            ONE_YEAR_SECONDS,
            2,
            &[("Owner", "TeamA")],
        ),
        (
            "three c5 hosts, Partial Upfront, one year, two tags",
            "c5",
            "PartialUpfront",
            ONE_YEAR_SECONDS,
            3,
            &[("Owner", "TeamB"), ("Tier", "web")],
        ),
        (
            "one t3 host, All Upfront, three years, untagged",
            "t3",
            "AllUpfront",
            3 * ONE_YEAR_SECONDS,
            1,
            &[],
        ),
    ];

    for (case, instance_family, payment_option, duration_seconds, hosts, tags) in rows {
        let (router, engine) = super::ec2_query::emulator();
        let host_ids = allocate(&router, instance_family, hosts).await;
        let named: Vec<&str> = host_ids.iter().map(String::as_str).collect();
        let offering_id =
            published_offering(&router, instance_family, payment_option, duration_seconds).await;
        let offered = offering(case, &offering_id);

        let mut extra: Vec<Param> = Vec::new();
        if !tags.is_empty() {
            extra.push(param(
                "TagSpecification.1.ResourceType",
                ec2::state::HOST_RESERVATION_TAGGABLE_RESOURCE,
            ));
            for (position, (key, value)) in tags.iter().enumerate() {
                let nth = position + 1;
                extra.push(param(&format!("TagSpecification.1.Tag.{nth}.Key"), key));
                extra.push(param(&format!("TagSpecification.1.Tag.{nth}.Value"), value));
            }
        }
        let bought = purchased(&router, case, &named, &offering_id, &extra).await;

        // The purchase answers the reservation it bought, and the totals over its hosts.
        let reservation_id = element(&bought, "hostReservationId").unwrap_or_else(|| {
            panic!("case {case:?}: a purchase names the reservation it bought: {bought}")
        });
        let (hourly_total, upfront_total) = offered.total_prices_milli(hosts);
        assert_eq!(
            member(&bought, "totalUpfrontPrice").as_deref(),
            Some(format_price_milli(upfront_total).as_str()),
            "case {case:?}: expected the upfront price over every host, got {bought}"
        );
        assert_eq!(
            member(&bought, "totalHourlyPrice").as_deref(),
            Some(format_price_milli(hourly_total).as_str()),
            "case {case:?}: expected the hourly price over every host, got {bought}"
        );
        assert_eq!(
            member(&bought, "currencyCode").as_deref(),
            Some(HOST_OFFERING_CURRENCY_CODE),
            "case {case:?}: expected the currency the offerings are quoted in, got {bought}"
        );

        // RULE 1: the describe reports exactly that reservation, built from the offering.
        let reported = described(&router, case, &[]).await;
        assert_eq!(
            reported_ids(&reported),
            vec![reservation_id.clone()],
            "case {case:?}: the describe must report the one reservation bought, got {reported}"
        );
        for (name, expected) in [
            ("instanceFamily", instance_family.to_string()),
            ("paymentOption", payment_option.to_string()),
            ("offeringId", offering_id.clone()),
            ("duration", duration_seconds.to_string()),
            ("count", hosts.to_string()),
            ("currencyCode", HOST_OFFERING_CURRENCY_CODE.to_string()),
            (
                "hourlyPrice",
                format_price_milli(offered.hourly_price_milli),
            ),
            (
                "upfrontPrice",
                format_price_milli(offered.upfront_price_milli),
            ),
        ] {
            assert_eq!(
                element(&reported, name).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {name} {expected:?}, got {reported}"
            );
        }
        let covered = elements(&reported, "hostIdSet")
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected a hostIdSet in {reported}"));
        for host_id in &host_ids {
            assert!(
                covered.contains(host_id.as_str()),
                "case {case:?}: expected the reservation to cover {host_id}, got {covered}"
            );
        }
        for (key, value) in tags {
            assert!(
                reported.contains(&format!("<key>{key}</key>"))
                    && reported.contains(&format!("<value>{value}</value>")),
                "case {case:?}: expected the tag {key}={value} on the reservation, got {reported}"
            );
        }

        // `End` is `Start` plus the term the offering publishes.
        let start = element(&reported, "start")
            .and_then(|at| super::ec2_query::as_timestamp(&at))
            .unwrap_or_else(|| panic!("case {case:?}: expected a start instant in {reported}"));
        let end = element(&reported, "end")
            .and_then(|at| super::ec2_query::as_timestamp(&at))
            .unwrap_or_else(|| panic!("case {case:?}: expected an end instant in {reported}"));
        assert!(
            (end - start - f64::from(duration_seconds)).abs() < 1.0,
            "case {case:?}: expected the term {duration_seconds} s between start and end, \
             got {start} -> {end}"
        );

        // RULE 1: every host the reservation covers now carries its id.
        for host_id in &host_ids {
            assert_eq!(
                host_reservation_of(&router, host_id).await.as_deref(),
                Some(reservation_id.as_str()),
                "case {case:?}: expected the host {host_id} to carry its reservation"
            );
        }

        // RULE 2: the charge has not settled when the call answers, and has one
        // TRANSITION_DELAY later.
        assert_eq!(
            element(&reported, "state").as_deref(),
            Some(ec2::state::RESERVATION_STATE_PAYMENT_PENDING),
            "case {case:?}: a fresh purchase is still settling, got {reported}"
        );
        tokio::time::sleep(past_the_transition()).await;
        let settled = described(&router, case, &[]).await;
        assert_eq!(
            element(&settled, "state").as_deref(),
            Some(ec2::state::RESERVATION_STATE_ACTIVE),
            "case {case:?}: one TRANSITION_DELAY after the purchase the reservation must be \
             active, got {settled}"
        );

        // RULE 3: a describe writes nothing — one reservation held, and two calls agree.
        let again = described(&router, case, &[]).await;
        assert_eq!(
            reported_ids(&again),
            reported_ids(&settled),
            "case {case:?}: two describes must report the same reservations"
        );
        assert_eq!(
            stored_reservations(&engine),
            1,
            "case {case:?}: one purchase stores exactly one reservation"
        );
    }
}

/// RULE 3: `HostReservationIdSet` and every documented filter narrow what is reported, and a
/// filter this operation does not document is refused.
#[tokio::test]
async fn reservations_are_selected_by_id_and_by_the_documented_filters() {
    let (router, _engine) = super::ec2_query::emulator();

    // Three reservations that differ in instance family, payment option and tags, so each row
    // below selects a different subset of them.
    let mut bought: Vec<(&str, String)> = Vec::new();
    let plan: [ReservationPlan; 3] = [
        ("m5", "AllUpfront", &[("Owner", "TeamA")]),
        ("m5", "NoUpfront", &[("Owner", "TeamB")]),
        ("c5", "AllUpfront", &[]),
    ];
    for (instance_family, payment_option, tags) in plan {
        let host = allocate(&router, instance_family, 1).await.remove(0);
        let offering_id =
            published_offering(&router, instance_family, payment_option, ONE_YEAR_SECONDS).await;
        let mut extra: Vec<Param> = Vec::new();
        if !tags.is_empty() {
            extra.push(param(
                "TagSpecification.1.ResourceType",
                ec2::state::HOST_RESERVATION_TAGGABLE_RESOURCE,
            ));
            for (position, (key, value)) in tags.iter().enumerate() {
                let nth = position + 1;
                extra.push(param(&format!("TagSpecification.1.Tag.{nth}.Key"), key));
                extra.push(param(&format!("TagSpecification.1.Tag.{nth}.Value"), value));
            }
        }
        let body = purchased(
            &router,
            instance_family,
            &[host.as_str()],
            &offering_id,
            &extra,
        )
        .await;
        let reservation_id = element(&body, "hostReservationId")
            .unwrap_or_else(|| panic!("a purchase names its reservation: {body}"));
        bought.push((instance_family, reservation_id));
    }
    // Every reservation has settled, so the `state` rows below are about one known state
    // rather than about where inside TRANSITION_DELAY this test is running.
    tokio::time::sleep(past_the_transition()).await;

    let all: Vec<String> = {
        let mut ids: Vec<String> = bought.iter().map(|(_, id)| id.clone()).collect();
        ids.sort();
        ids
    };
    let m5: Vec<String> = {
        let mut ids: Vec<String> = bought
            .iter()
            .filter(|(family, _)| *family == "m5")
            .map(|(_, id)| id.clone())
            .collect();
        ids.sort();
        ids
    };
    let first = all.first().cloned().unwrap_or_default();

    // Rows differ in WHAT narrows the answer: each documented filter, a wildcard value, two
    // values of one filter (ORed), two filters (ANDed), the id set, and a selection that
    // matches nothing.
    let rows: Vec<(&str, Vec<Param>, Vec<String>)> = vec![
        ("no narrowing at all", Vec::new(), all.clone()),
        (
            "instance-family",
            super::ec2_query::filters(&[("instance-family", &["m5"])]),
            m5.clone(),
        ),
        (
            "instance-family with a wildcard",
            super::ec2_query::filters(&[("instance-family", &["m*"])]),
            m5.clone(),
        ),
        (
            "two values of one filter are ORed",
            super::ec2_query::filters(&[("instance-family", &["m5", "c5"])]),
            all.clone(),
        ),
        (
            "payment-option",
            super::ec2_query::filters(&[("payment-option", &["NoUpfront"])]),
            vec![bought[1].1.clone()],
        ),
        (
            "two filters are ANDed",
            super::ec2_query::filters(&[
                ("instance-family", &["m5"]),
                ("payment-option", &["AllUpfront"]),
            ]),
            vec![bought[0].1.clone()],
        ),
        (
            "state, which every settled reservation carries",
            super::ec2_query::filters(&[("state", &[ec2::state::RESERVATION_STATE_ACTIVE])]),
            all.clone(),
        ),
        (
            "a state no reservation is in",
            super::ec2_query::filters(&[("state", &[ec2::state::RESERVATION_STATE_RETIRED])]),
            Vec::new(),
        ),
        (
            "tag:Owner",
            super::ec2_query::filters(&[("tag:Owner", &["TeamB"])]),
            vec![bought[1].1.clone()],
        ),
        (
            "tag-key",
            super::ec2_query::filters(&[("tag-key", &["Owner"])]),
            {
                let mut tagged = vec![bought[0].1.clone(), bought[1].1.clone()];
                tagged.sort();
                tagged
            },
        ),
        (
            "tag-value, the rest of the tag family every tagged resource answers",
            super::ec2_query::filters(&[("tag-value", &["TeamA"])]),
            vec![bought[0].1.clone()],
        ),
        (
            "one id of the set",
            indexed("HostReservationIdSet", &[first.as_str()]),
            vec![first.clone()],
        ),
        (
            "an id set beside a filter that excludes it",
            {
                let mut params = indexed("HostReservationIdSet", &[bought[2].1.as_str()]);
                params.extend(super::ec2_query::filters(&[(
                    "instance-family",
                    &["m5"],
                )]));
                params
            },
            Vec::new(),
        ),
    ];

    for (case, params, expected) in rows {
        let body = described(&router, case, &params).await;
        let mut reported = reported_ids(&body);
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the reservations {expected:?}, got {reported:?} in {body}"
        );
    }

    // A filter name this operation does not document is refused whether or not anything would
    // have matched it.
    for case in ["host-id", "tag:", "InstanceFamily", "instance_family"] {
        let (status, body) = describe(&router, &super::ec2_query::filters(&[(case, &["m5"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an undocumented filter is refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// RULE 4: the describe pages in reservation-id order, inside the page size the member
/// publishes.
#[tokio::test]
async fn the_describe_pages_in_reservation_id_order() {
    let (router, _engine) = super::ec2_query::emulator();

    // Seven reservations, each covering a host of its own, so a page of five leaves two over.
    let hosts = allocate(&router, "m5", 7).await;
    let offering_id = published_offering(&router, "m5", "AllUpfront", ONE_YEAR_SECONDS).await;
    let mut all: Vec<String> = Vec::new();
    for host in &hosts {
        let body = purchased(&router, "a page of reservations", &[host.as_str()], &offering_id, &[])
            .await;
        all.push(
            element(&body, "hostReservationId")
                .unwrap_or_else(|| panic!("a purchase names its reservation: {body}")),
        );
    }
    all.sort();

    // Rows differ in the page size: the smallest the member admits, one that divides the set
    // unevenly, and the largest.
    for page_size in [5_usize, 6, 500] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(cursor) = token.as_deref() {
                params.push(param("NextToken", cursor));
            }
            let body = described(&router, "paging", &params).await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "page size {page_size}: a page must hold at most {page_size} reservations, \
                 got {}: {body}",
                page.len()
            );
            walked.extend(page);
            token = member(&body, "nextToken");
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= all.len(),
                "page size {page_size}: the walk must end, got {walked:?}"
            );
        }
        assert_eq!(
            walked, all,
            "page size {page_size}: the pages together must report every reservation once, \
             in id order"
        );
    }

    // The published range: "this value can be between 5 and 500. If maxResults is given a
    // larger value than 500, you receive an error" — each bound is served, and the value just
    // outside each is refused.
    for max_results in ["5", "500"] {
        let body = described(&router, max_results, &[param("MaxResults", max_results)]).await;
        assert!(
            !reported_ids(&body).is_empty(),
            "MaxResults {max_results:?}: a page size inside the published range is served, \
             got {body}"
        );
    }
    for (case, max_results) in [("below the range", "4"), ("above the range", "501")] {
        let (status, body) = describe(&router, &[param("MaxResults", max_results)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }

    // A token this operation did not issue is refused rather than read as an id.
    for case in ["not-a-token", "hr-0123456789abcdef0", "zzzz"] {
        let (status, body) = describe(&router, &[param("NextToken", case)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// RULE 5: every reservation id this account holds no reservation under is refused, naming all
/// of them.
#[tokio::test]
async fn an_unresolvable_reservation_id_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let host = allocate(&router, "m5", 1).await.remove(0);
    let offering_id = published_offering(&router, "m5", "AllUpfront", ONE_YEAR_SECONDS).await;
    let body = purchased(&router, "a held reservation", &[host.as_str()], &offering_id, &[]).await;
    let held = element(&body, "hostReservationId")
        .unwrap_or_else(|| panic!("a purchase names its reservation: {body}"));

    // Rows differ in HOW the id fails to name a reservation of the account: a well-formed id
    // nothing was bought under, the short id form, another resource's prefix, something that is
    // not an id at all, and an unresolvable id beside a held one — which is refused as a whole
    // rather than answered with the reservation it could resolve.
    let rows: [(&str, Vec<&str>); 5] = [
        (
            "a well-formed id nothing holds",
            vec![UNHELD_RESERVATION_ID],
        ),
        ("the short id form", vec!["hr-12345678"]),
        ("another resource's prefix", vec!["h-0123456789abcdef0"]),
        ("not an id at all", vec!["not-a-reservation"]),
        (
            "an unresolvable id beside a held one",
            vec![held.as_str(), UNHELD_RESERVATION_ID],
        ),
    ];

    for (case, reservation_ids) in rows {
        let (status, body) =
            describe(&router, &indexed("HostReservationIdSet", &reservation_ids)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidHostReservationId.Malformed",
            "case {case:?}: expected InvalidHostReservationId.Malformed, got {body}"
        );
        // The message names every id that could not be resolved, and only those.
        let unresolvable: Vec<String> = reservation_ids
            .iter()
            .filter(|reservation_id| **reservation_id != held.as_str())
            .map(|reservation_id| format!("'{reservation_id}'"))
            .collect();
        assert_eq!(
            error_message(&body),
            format!(
                "The specified host Reservation IDs [{}] are not valid.",
                unresolvable.join(", ")
            ),
            "case {case:?}: unexpected refusal message in {body}"
        );
    }
}

/// RULE 6: a purchase this emulator cannot make buys nothing.
#[tokio::test]
async fn a_purchase_that_cannot_be_made_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let hosts = allocate(&router, "m5", 2).await;
    let released = allocate(&router, "m5", 1).await.remove(0);
    let (status, body) = call(
        &router,
        REGION,
        "ReleaseHosts",
        &indexed("HostId", &[released.as_str()]),
    )
    .await;
    assert_eq!(
        status, 200,
        "ReleaseHosts must succeed, got {status} with body {body}"
    );
    let offering_id = published_offering(&router, "m5", "AllUpfront", ONE_YEAR_SECONDS).await;
    let other_family = published_offering(&router, "c5", "AllUpfront", ONE_YEAR_SECONDS).await;
    let offered = offering("the m5 offering", &offering_id);
    // What one host of this offering costs upfront, as the member documents a limit price:
    // "to indicate a limit price of USD 100, specify 100.00".
    let (_, upfront_total) = offered.total_prices_milli(1);

    // Rows differ in WHY the purchase cannot be made and in which parameter is at fault.
    let rows: Vec<RefusalRow<'_>> = vec![
        (
            "a well-formed host id nothing holds",
            vec!["h-0123456789abcdef0"],
            offering_id.as_str(),
            Vec::new(),
            "InvalidHostId.NotFound",
        ),
        (
            "a released host",
            vec![released.as_str()],
            offering_id.as_str(),
            Vec::new(),
            "InvalidHostId.NotFound",
        ),
        (
            "a released host beside a held one",
            vec![hosts[0].as_str(), released.as_str()],
            offering_id.as_str(),
            Vec::new(),
            "InvalidHostId.NotFound",
        ),
        (
            "a value that is not a host id",
            vec!["not-a-host"],
            offering_id.as_str(),
            Vec::new(),
            "InvalidHostID.Malformed",
        ),
        (
            "an offering of another instance family",
            vec![hosts[0].as_str()],
            other_family.as_str(),
            Vec::new(),
            "InvalidParameterValue",
        ),
        (
            "an id no published offering carries",
            vec![hosts[0].as_str()],
            "hro-0ffffffffffffffff",
            Vec::new(),
            "InvalidParameterValue",
        ),
        (
            "a limit price below the total upfront cost",
            vec![hosts[0].as_str()],
            offering_id.as_str(),
            vec![param(
                "LimitPrice",
                &format_price_milli(upfront_total.saturating_sub(1)),
            )],
            "InvalidParameterValue",
        ),
        (
            "a limit price that is no amount at all",
            vec![hosts[0].as_str()],
            offering_id.as_str(),
            vec![param("LimitPrice", "one hundred")],
            "InvalidParameterValue",
        ),
        (
            "a limit price with more places than a price carries",
            vec![hosts[0].as_str()],
            offering_id.as_str(),
            vec![param("LimitPrice", "100.00001")],
            "InvalidParameterValue",
        ),
        (
            "a negative limit price",
            vec![hosts[0].as_str()],
            offering_id.as_str(),
            vec![param("LimitPrice", "-1.00")],
            "InvalidParameterValue",
        ),
    ];

    for (case, host_ids, named_offering, extra, expected) in rows {
        let (status, body) = purchase(&router, &host_ids, named_offering, &extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
    assert_eq!(
        stored_reservations(&engine),
        0,
        "a refused purchase stores no reservation"
    );
    let reported = described(&router, "after the refusals", &[]).await;
    assert!(
        reported_ids(&reported).is_empty(),
        "a refused purchase leaves nothing for the describe to report, got {reported}"
    );

    // A limit price EQUAL to the total upfront cost is not above it, so that purchase is made:
    // "if the total upfront cost is GREATER than the specified price limit, the request fails".
    let at_the_limit = purchased(
        &router,
        "a limit price equal to the total upfront cost",
        &[hosts[1].as_str()],
        &offering_id,
        &[param("LimitPrice", &format_price_milli(upfront_total))],
    )
    .await;
    assert!(
        element(&at_the_limit, "hostReservationId").is_some(),
        "a purchase at its limit price names the reservation it bought, got {at_the_limit}"
    );
    assert_eq!(
        stored_reservations(&engine),
        1,
        "the purchase at its limit price stores its reservation"
    );
}

/// RULE 7: `ClientToken` makes the purchase idempotent.
#[tokio::test]
async fn a_repeat_of_one_purchase_buys_nothing_more() {
    let (router, engine) = super::ec2_query::emulator();
    // Two hosts: the replayed purchase covers the first, and the second token covers a host of
    // its own, so this suite says nothing about two reservations covering ONE host.
    let hosts = allocate(&router, "m5", 2).await;
    let host = hosts[0].clone();
    let offering_id = published_offering(&router, "m5", "AllUpfront", ONE_YEAR_SECONDS).await;

    // Rows differ in the token: one request replayed under its own token twice, and a second
    // token, which buys a reservation of its own.
    let token = "a-purchase-token";
    let first = purchased(
        &router,
        "the first purchase",
        &[host.as_str()],
        &offering_id,
        &[param("ClientToken", token)],
    )
    .await;
    let reservation_id = element(&first, "hostReservationId")
        .unwrap_or_else(|| panic!("a purchase names its reservation: {first}"));
    assert_eq!(
        member(&first, "clientToken").as_deref(),
        Some(token),
        "the purchase echoes the token the request carried, got {first}"
    );

    for attempt in ["the first replay", "the second replay"] {
        let replayed = purchased(
            &router,
            attempt,
            &[host.as_str()],
            &offering_id,
            &[param("ClientToken", token)],
        )
        .await;
        assert_eq!(
            element(&replayed, "hostReservationId").as_deref(),
            Some(reservation_id.as_str()),
            "case {attempt:?}: a replay reports the reservation the token already bought, \
             got {replayed}"
        );
        assert_eq!(
            member(&replayed, "totalUpfrontPrice"),
            member(&first, "totalUpfrontPrice"),
            "case {attempt:?}: a replay quotes the same total upfront price"
        );
        assert_eq!(
            stored_reservations(&engine),
            1,
            "case {attempt:?}: a replay buys no second reservation"
        );
    }

    let second = purchased(
        &router,
        "another token",
        &[hosts[1].as_str()],
        &offering_id,
        &[param("ClientToken", "another-purchase-token")],
    )
    .await;
    let other_id = element(&second, "hostReservationId")
        .unwrap_or_else(|| panic!("a purchase names its reservation: {second}"));
    assert_ne!(
        other_id, reservation_id,
        "another token buys a reservation of its own, got {second}"
    );
    assert_eq!(
        stored_reservations(&engine),
        2,
        "another token buys a second reservation"
    );
    let reported = described(&router, "both reservations", &[]).await;
    let mut ids = reported_ids(&reported);
    ids.sort();
    let mut expected = vec![reservation_id, other_id];
    expected.sort();
    assert_eq!(
        ids, expected,
        "the describe reports both reservations, got {reported}"
    );
}

/// A price's text and its value in thousandths are two readings of one amount: every price the
/// published catalogue quotes round-trips through `parse_price_milli`, and every amount a
/// caller may write as a `LimitPrice` reads back as the amount it names.
///
/// The first table is the catalogue's OWN prices, read back off
/// `DescribeHostReservationOfferings` rather than written here, so it spans every price this
/// plane quotes; the second is the forms a limit price may take — no places, one, two, three —
/// from what that member documents ("to indicate a limit price of USD 100, specify 100.00"),
/// and the forms that are no price at all.
#[tokio::test]
async fn a_price_and_its_text_are_two_readings_of_one_amount() {
    let (router, _engine) = super::ec2_query::emulator();
    let body = described_offerings(&router).await;
    let quoted: Vec<String> = elements(&body, "hourlyPrice")
        .into_iter()
        .chain(elements(&body, "upfrontPrice"))
        .collect();
    assert!(
        !quoted.is_empty(),
        "the catalogue must quote prices to read back: {body}"
    );
    for text in &quoted {
        let milli = parse_price_milli(text)
            .unwrap_or_else(|| panic!("case {text:?}: a quoted price must read as an amount"));
        assert_eq!(
            format_price_milli(milli),
            *text,
            "case {text:?}: expected the amount to render back as its own text"
        );
        // What a reservation covering several hosts is charged is that amount over them, which
        // is quoted in the same form.
        let total = milli.saturating_mul(3);
        assert_eq!(
            parse_price_milli(&format_price_milli(total)),
            Some(total),
            "case {text:?}: a total over three hosts round-trips too"
        );
    }

    // The forms a caller may write an amount in, and what each one names.
    let rows: [(&str, Option<i64>); 11] = [
        ("100", Some(100_000)),
        ("100.0", Some(100_000)),
        ("100.00", Some(100_000)),
        ("100.000", Some(100_000)),
        ("0.5", Some(500)),
        ("0.050", Some(50)),
        ("100.0001", None),
        ("100.", None),
        ("-1.00", None),
        ("one hundred", None),
        (".5", None),
    ];
    for (text, expected) in rows {
        assert_eq!(
            parse_price_milli(text),
            expected,
            "case {text:?}: expected {expected:?}, got {:?}",
            parse_price_milli(text)
        );
    }
}
