// SPDX-License-Identifier: CC0-1.0

//! `CreateCapacityReservationBySplitting`: how it reads the reservation a split takes
//! capacity OUT of, and how much capacity that reservation has to give.
//!
//! Driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test:
//!
//! 1. `SourceCapacityReservationId` is read by the SHAPE every EC2 resource id takes:
//!    a value OF that shape the account holds no reservation under is reported with the
//!    resource's own not-found code and the sentence naming the id the request sent, and only
//!    a value OUTSIDE the shape is refused as malformed, before any lookup. A client that
//!    branches on the not-found code to create the reservation it is missing depends on the
//!    difference.
//! 2. A split MOVES capacity out of a reservation that goes on holding capacity, so the
//!    capacity it has to give is the capacity nothing has launched into less the one instance
//!    it keeps: a request for more than that is refused `InvalidParameterValue` naming how
//!    much there is to split and how much was asked for, and writes nothing — a request
//!    within it is served, and the two reservations together hold exactly what the source
//!    held.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::wire::param;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// One call of `action` carrying `params`, as `(status, body)`.
async fn call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// The id of an `active` Capacity Reservation holding `instance_count` instances, created
/// over the wire so every row below starts from state the service itself wrote.
async fn create_reservation(
    router: &MultiServiceRouter,
    case: &str,
    instance_count: i32,
) -> String {
    let (status, body) = call(
        router,
        "CreateCapacityReservation",
        &[
            param("InstanceType", "m5.large"),
            param("InstancePlatform", "Linux/UNIX"),
            param("AvailabilityZone", "us-east-1a"),
            param("InstanceCount", &instance_count.to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 creating a reservation of {instance_count} \
         instances, got {status} with body {body}"
    );
    element(&body, "capacityReservationId").unwrap_or_else(|| {
        panic!("case {case:?}: the create answered no capacityReservationId: {body}")
    })
}

/// The `totalInstanceCount` of every reservation `DescribeCapacityReservations` reports for
/// `ids`, in the order it reports them.
async fn described_totals(
    router: &MultiServiceRouter,
    case: &str,
    ids: &[&str],
) -> Vec<String> {
    let (status, body) = call(
        router,
        "DescribeCapacityReservations",
        &indexed("CapacityReservationId", ids),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 describing {ids:?}, got {status} with body {body}"
    );
    elements(&body, "totalInstanceCount")
}

/// One source id a split cannot resolve, and how EC2 refuses it.
struct UnresolvableSource {
    /// What the row sends as `SourceCapacityReservationId`.
    id: &'static str,
    /// Why this value is the row it is — named in every assertion of the row.
    reading: &'static str,
    /// The `<Code>` the refusal must carry.
    code: &'static str,
    /// The `<Message>` the refusal must carry, or `None` where the row asserts the class
    /// alone.
    message: Option<String>,
}

/// The not-found half of rule 1: a value of the EC2 resource-id shape this account holds no
/// reservation under, whatever its digits.
fn absent_sources() -> Vec<UnresolvableSource> {
    // The shape is `<prefix>-<8 or 17 lowercase hex digits>`
    // (`ec2::state::prefixed_id_is_well_formed`), so the rows differ along the digit count
    // and the digits themselves — the all-zero placeholder a client reaches for included,
    // which the mint counting from one can never have issued.
    ["cr-00000000000000000", "cr-0123456789abcdef1", "cr-0a1b2c3d"]
        .into_iter()
        .map(|id| UnresolvableSource {
            id,
            reading: "of the id shape, held by no reservation",
            code: ec2::state::INVALID_CAPACITY_RESERVATION_ID_NOT_FOUND,
            message: Some(format!("The capacity-reservation ID '{id}' does not exist")),
        })
        .collect()
}

/// The malformed half of rule 1: a value that is not a Capacity Reservation id at all, so
/// there is nothing to look it up as. The class alone is asserted here; what the sentence
/// says is the subject of the not-found rows above.
fn unreadable_sources() -> Vec<UnresolvableSource> {
    [
        // Another kind's prefix.
        "i-0123456789abcdef1",
        // The right prefix with the wrong digits: upper case, out of the hex alphabet, and
        // neither the short nor the long digit count.
        "cr-0123456789ABCDEF1",
        "cr-0123456789abcdefg",
        "cr-0123456",
        "cr-",
        "not-an-id-at-all",
    ]
    .into_iter()
    .map(|id| UnresolvableSource {
        id,
        reading: "outside the id shape",
        code: ec2::state::INVALID_CAPACITY_RESERVATION_ID_MALFORMED,
        message: None,
    })
    .collect()
}

#[tokio::test]
async fn an_absent_source_is_not_found_and_only_an_unreadable_one_is_malformed() {
    let (router, _engine) = super::ec2_query::emulator();
    for source in absent_sources().into_iter().chain(unreadable_sources()) {
        let case = format!("{} ({})", source.id, source.reading);
        let (status, body) = call(
            &router,
            "CreateCapacityReservationBySplitting",
            &[
                param("SourceCapacityReservationId", source.id),
                param("InstanceCount", "1"),
            ],
        )
        .await;
        let code = error_code(&body);
        assert_eq!(
            code, source.code,
            "case {case:?}: expected code {expected:?}, got {code:?} with body {body}",
            expected = source.code,
        );
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {code:?}, got {status} with body {body}"
        );
        if let Some(expected) = source.message {
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the message {expected:?}, got {message:?} with body \
                 {body}"
            );
        }
    }
}

/// One split asked of a source the store holds: how much capacity that source was created
/// with, and how many instances the request asks to split off it.
struct Split {
    /// The instances the source reservation is created with.
    held: i32,
    /// The `InstanceCount` the split asks for.
    requested: i32,
}

/// Every split that asks for more than its source has to give: the source keeps one
/// instance, so the capacity there is to split is `held - 1`. The rows differ along the
/// capacity the source holds and by how far the request overshoots — by one instance, and by
/// several.
fn oversized_splits() -> Vec<Split> {
    vec![
        Split { held: 1, requested: 1 },
        Split { held: 1, requested: 5 },
        Split { held: 2, requested: 2 },
        Split { held: 4, requested: 7 },
        Split { held: 9, requested: 9 },
    ]
}

#[tokio::test]
async fn a_split_past_what_the_source_has_to_give_is_refused_and_writes_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    for split in oversized_splits() {
        let case = format!(
            "split {requested} of {held}",
            requested = split.requested,
            held = split.held,
        );
        let source = create_reservation(&router, &case, split.held).await;
        let (status, body) = call(
            &router,
            "CreateCapacityReservationBySplitting",
            &[
                param("SourceCapacityReservationId", &source),
                param("InstanceCount", &split.requested.to_string()),
            ],
        )
        .await;
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidParameterValue",
            "case {case:?}: expected the split to be refused InvalidParameterValue, got \
             {code:?} at {status} with body {body}"
        );
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {code:?}, got {status} with body {body}"
        );
        // The capacity there is to split is what the source holds less the instance it keeps,
        // and the request's own count is named beside it.
        let expected = format!(
            "There isn't enough available capacity in the source Capacity Reservation. The \
             current available instance count is {available}. You requested to split \
             {requested}.",
            available = split.held - 1,
            requested = split.requested,
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?} with body {body}"
        );

        // A refused split writes nothing: the source still holds every instance it was
        // created with, and no second reservation was minted to hold any of them.
        let totals = described_totals(&router, &case, &[source.as_str()]).await;
        assert_eq!(
            totals,
            vec![split.held.to_string()],
            "case {case:?}: expected the refused split to leave {held} instances in \
             {source}, got {totals:?}",
            held = split.held,
        );
    }
}

/// Every split a source has the capacity for: the destination takes the instances asked for
/// and the source keeps the rest. The rows differ along the capacity the source holds and the
/// share of it that moves — the largest split a source has to give, and a smaller one.
fn served_splits() -> Vec<Split> {
    vec![
        Split { held: 2, requested: 1 },
        Split { held: 5, requested: 4 },
        Split { held: 9, requested: 2 },
    ]
}

#[tokio::test]
async fn a_split_moves_the_capacity_it_reports_and_the_two_sides_conserve_it() {
    let (router, _engine) = super::ec2_query::emulator();
    for split in served_splits() {
        let case = format!(
            "split {requested} of {held}",
            requested = split.requested,
            held = split.held,
        );
        let source = create_reservation(&router, &case, split.held).await;
        let (status, body) = call(
            &router,
            "CreateCapacityReservationBySplitting",
            &[
                param("SourceCapacityReservationId", &source),
                param("InstanceCount", &split.requested.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 splitting {requested} instances off {source}, \
             got {status} with body {body}",
            requested = split.requested,
        );
        let parsed = super::wire::document(&case, &body);
        let destination = super::wire::child(parsed.root_element(), "destinationCapacityReservation")
            .and_then(|node| super::wire::child_text(node, "capacityReservationId"))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the split answered no destination reservation: {body}")
            });
        assert_ne!(
            destination, source,
            "case {case:?}: expected the split to mint a reservation of its own, got the \
             source back in {body}"
        );

        // Read both sides back out of the store: a split that reported capacity it did not
        // move would differ here, and the two together hold exactly what the source held.
        let totals =
            described_totals(&router, &case, &[source.as_str(), destination.as_str()]).await;
        let expected = vec![
            (split.held - split.requested).to_string(),
            split.requested.to_string(),
        ];
        assert_eq!(
            totals, expected,
            "case {case:?}: expected the stored reservations to hold {expected:?} instances, \
             got {totals:?}"
        );
    }
}
