// SPDX-License-Identifier: CC0-1.0

//! @op SnapToRoads
//!
//! Each trace point is snapped onto the nearest road of the network in [`crate::state`]
//! that the requested travel mode may use. The answer carries one snapped point per
//! submitted trace point, at the same index, with the submitted position echoed as
//! `OriginalPosition`, the point on the road as `SnappedPosition` and a `Confidence` read
//! off the match itself ([`match_confidence`]): how far the point had to move to reach its
//! road, and how much nearer that road is than the next candidate road. A point whose
//! nearest usable road is further away than `SnapRadius` is not matched: it keeps its
//! submitted position, scores zero confidence, and its index is listed under a
//! `TracePointsNotMatched` notice. So is a point the network holds no road anywhere near —
//! one in the open sea, where [`crate::state`] materialises no road at all — which is the
//! case the notice exists for: a client reading `SnappedTracePoints` alone could not tell
//! that fix from a road fix.
//!
//! `SnappedGeometry` is the line along the ROADS the matched points were matched onto,
//! from the first of them to the last ([`matched_geometry`]); a point reported unmatched is
//! left out of it, so the geometry and the per-point confidence cannot disagree, and a
//! trace with fewer than two matched points reports no geometry at all.
//!
//! `TracePoints[].Timestamp`, `Speed` and `Heading` are read but have nothing to act on:
//! the network carries no one-way or direction data for a heading to disambiguate and no
//! time-varying road data for a timestamp or a speed to select, so the snap of a point
//! depends on its position and the travel mode alone. `TravelModeOptions` is the same —
//! the model carries no vehicle restriction for a truck's dimensions or cargo to meet.
//!
//! ## Authorization
//!
//! The emulator verifies every request's SIGNATURE before any handler runs, so this handler
//! checks no credential. What it does check is whether the caller MAY perform the action:
//! AWS decides a signed call against the calling principal's identity-based IAM policies at
//! the IAM layer, ahead of the service reading the request, so the handler takes that
//! decision first through [`crate::state::identity_authorization`] and answers the modelled
//! `AccessDeniedException` (HTTP 403) with IAM's own sentence for a caller no readable
//! statement grants the action to, or one an explicit `Deny` refuses. The modelled `Key`
//! (API key) member is still not consulted: this emulator holds no API-key records to
//! authorize a key against, and refusing a correctly signed request for want of one would
//! refuse a valid caller.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `SnapToRoads`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-georoutes-routesservice/smithy-model/com-amazonaws-georoutes-routesservice.json")
//! ```
//!
//! SnapToRoads matches GPS trace to roads most likely traveled on. For more information,
//! see Snap to Roads in the Amazon Location Service Developer Guide.
//!
//! ## Input
//!
//! * `Key` (ApiKey, len ..1000) — Optional: The API key to be used for authorization. Either
//!   an API key or valid SigV4 signature must be provided when making a request.
//! * `SnapRadius` (DistanceMeters, range 0..4294967295, default 0) **required** — The radius
//!   around the provided tracepoint that is considered for snapping. Unit: meters Default
//!   value: 300
//! * `SnappedGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) — Chooses
//!   what the returned SnappedGeometry format should be. Default value: FlexiblePolyline
//! * `TracePoints` (RoadSnapTracePointList) **required** — List of trace points to be snapped
//!   onto the road network.
//! * `TravelMode` (RoadSnapTravelMode, one of: Car | Pedestrian | Scooter | Truck) —
//!   Specifies the mode of transport when calculating a route. Used in estimating the speed
//!   of travel and road compatibility. Default value: Car
//! * `TravelModeOptions` (RoadSnapTravelModeOptions) — Travel mode related options for the
//!   provided travel mode.
//!
//! ## Output
//!
//! * `Notices` (RoadSnapNoticeList) **required** — Notices are additional information
//!   returned that indicate issues that occurred during route calculation.
//! * `PricingBucket` (String) **required** — The pricing bucket for which the query is
//!   charged at.
//! * `SnappedGeometry` (RoadSnapSnappedGeometry) — The interpolated geometry for the snapped
//!   route onto the road network.
//! * `SnappedGeometryFormat` (GeometryFormat, one of: FlexiblePolyline | Simple) **required**
//!   — Specifies the format of the geometry returned for each leg of the route.
//! * `SnappedTracePoints` (RoadSnapSnappedTracePointList) **required** — The trace points
//!   snapped onto the road network.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You don't have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an AWS service.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{self, Position, TravelMode};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `SnapToRoadsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-georoutes-routesservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `SnapToRoadsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.georoutes#SnapToRoads")]
#[derive(Debug)]
pub enum SnapToRoadsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::snap_to_roads::SnapToRoadsError`). The wrapper is kept because it is
/// also the only place a request this operation models no error for could be refused;
/// every refusal below uses the model's own `ValidationException`.
pub type SnapToRoadsError = crate::scaffolded::Scaffolded<SnapToRoadsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<SnapToRoadsErrorModeled> for SnapToRoadsError {
    fn from(modeled: SnapToRoadsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `SnapToRoads` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct SnapToRoads<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> SnapToRoads<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for SnapToRoads<Eng>
{
    type Input = SnapToRoadsInput;
    type Output = SnapToRoadsOutput;
    type Error = SnapToRoadsError;

    const NAME: &'static str = "SnapToRoads";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SNAP_TO_ROADS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SNAP_TO_ROADS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // AWS decides authorization at the IAM layer, ahead of the service looking at the
        // request's own input, so this is the FIRST thing this handler does: a caller whose
        // identity-based policies do not allow this action is refused and nothing is
        // computed for it.
        if let Some(denial) = crate::state::identity_authorization::refusal(
            &*self.engine,
            ctx,
            <Self as smithy_server_runtime::Operation>::NAME,
        ) {
            return Err(access_denied(denial));
        }

        let travel_mode = match input.travel_mode.as_deref() {
            // `Default value: Car`.
            None => TravelMode::Car,
            Some(value) => TravelMode::from_model_values(value, ROAD_SNAP_TRAVEL_MODE_VALUES).ok_or_else(|| {
                invalid(
                    "TravelMode",
                    state::must_be_one_of("TravelMode", ROAD_SNAP_TRAVEL_MODE_VALUES),
                )
            })?,
        };
        let geometry_format = state::geometry_format("SnappedGeometryFormat", input.snapped_geometry_format.as_ref())
            .map_err(|message| invalid("SnappedGeometryFormat", message))?;

        // A per-mode option block belongs to the one travel mode its member names, so a
        // request may set only the block the `TravelMode` it selected names; this
        // operation's options structure declares a block for one of the four modes its
        // travel-mode enum admits, and a mode with none permits no block at all.
        let declared = declared_travel_mode_options(input.travel_mode_options.as_ref());
        let permitted: Vec<&str> = state::block_of_mode(travel_mode.as_model(), &declared)
            .into_iter()
            .collect();
        state::check_travel_mode_options(travel_mode.as_model(), &permitted, &declared)
            .map_err(|(member, message)| invalid(&member, message))?;

        // `SnapRadius` narrows the range of the shape it targets: `DistanceMeters` is
        // `0..4294967295` and the member is `0..10000`, so the boundary validator enforces
        // the TARGET's wider range and the member's own upper bound is the handler's to
        // apply. The model's `@default` is 0, which is no radius at all; the documented
        // default the service applies is 300 metres, so a radius outside the member's range
        // is refused rather than read as "unset".
        state::check_range(
            "SnapRadius",
            (0.0, state::MAX_SNAP_RADIUS_M as f64),
            input.snap_radius as f64,
        )
        .map_err(|message| invalid("SnapRadius", message))?;
        let snap_radius_m = if input.snap_radius == 0 {
            state::DEFAULT_SNAP_RADIUS_M as f64
        } else {
            input.snap_radius as f64
        };

        // The documented list bounds ("List of trace points to be snapped onto the road
        // network", 2..5000 on `TracePoints`). They are enforced HERE rather than relied on
        // from the boundary validator, so the per-request work below is bounded whatever
        // the generated validator does with the member's length.
        state::check_length(
            "TracePoints",
            (MIN_TRACE_POINTS, MAX_TRACE_POINTS),
            input.trace_points.len(),
        )
        .map_err(|message| invalid("TracePoints", message))?;
        let mut positions = Vec::with_capacity(input.trace_points.len());
        for (index, trace_point) in input.trace_points.iter().enumerate() {
            let member = format!("TracePoints[{index}].Position");
            positions.push(
                state::validate_position(&trace_point.position)
                    .map_err(|message| invalid(&member, message))?,
            );
        }

        let state_handle =
            state::service_state(&*self.engine).ok_or_else(|| internal("the routes service state is unavailable"))?;

        // Snapping runs in chunks: the lock on the shared state is taken per chunk and
        // released between chunks, where the handler yields — so a long trace neither
        // holds the lock for the whole request nor leaves a client's cancellation or the
        // server's deadline nowhere to land.
        let mut snapped: Vec<Option<state::SnappedPosition>> = Vec::with_capacity(positions.len());
        for chunk in positions.chunks(SNAP_CHUNK) {
            {
                let mut state = state_handle.lock();
                snapped.extend(chunk.iter().map(|position| state.snap(*position, travel_mode)));
            }
            tokio::task::yield_now().await;
        }
        let pricing_bucket = state_handle.lock().pricing_bucket(Self::NAME);

        let mut snapped_trace_points = Vec::with_capacity(positions.len());
        // The matched points in order, each with the road it matched onto: the snapped
        // geometry is the stretch of ROAD between consecutive ones, so the road is needed
        // beside the position.
        let mut matched_path: Vec<state::SnappedPosition> = Vec::with_capacity(positions.len());
        // The unmatched points, kept apart by WHY no road matched them: one lies beyond
        // the radius of the roads around it, the other lies where the network holds no
        // road at any radius. The notice states the reason, so the two cannot be reported
        // as each other.
        let mut beyond_radius: Vec<i32> = Vec::new();
        let mut off_network: Vec<i32> = Vec::new();
        for ((index, position), candidate) in positions.iter().enumerate().zip(snapped) {
            // Within the radius the point matches a road; beyond it there is no candidate
            // to match, so the point is reported unmatched rather than moved.
            let matched = candidate.filter(|candidate| candidate.offset_m <= snap_radius_m);
            match matched {
                Some(candidate) => {
                    snapped_trace_points.push(RoadSnapSnappedTracePoint {
                        confidence: match_confidence(*position, &candidate),
                        original_position: state::position_to_vec(*position),
                        snapped_position: state::position_to_vec(candidate.position),
                    });
                    matched_path.push(candidate);
                }
                None => {
                    if let Ok(index) = i32::try_from(index) {
                        if state::is_mapped_territory(*position) {
                            beyond_radius.push(index);
                        } else {
                            off_network.push(index);
                        }
                    }
                    // An unmatched point is NOT on the snapped route, so it is left out of
                    // the snapped geometry: the line would otherwise run through a
                    // position the same answer reports as matching no road.
                    snapped_trace_points.push(RoadSnapSnappedTracePoint {
                        confidence: 0.0,
                        original_position: state::position_to_vec(*position),
                        snapped_position: state::position_to_vec(*position),
                    });
                }
            }
        }

        // The indexes of the points no road matched, as the model's notice reports them,
        // under the title of the reason they were not matched.
        // `TracePointIndexes` carries at most `MAX_NOTICE_INDEXES` entries, so a trace with
        // more unmatched points than that is reported over as many notices of the same
        // code as it takes: every unmatched index is reported, and no notice breaks the
        // modeled length.
        let mut notices = Vec::new();
        for (indexes, reason) in [
            (
                &beyond_radius,
                format!("no road within the {snap_radius_m} metre snap radius"),
            ),
            (
                &off_network,
                "no road of the network at all: the position is outside the mapped territory the network covers"
                    .to_string(),
            ),
        ] {
            for indexes in indexes.chunks(MAX_NOTICE_INDEXES) {
                notices.push(RoadSnapNotice {
                    code: "TracePointsNotMatched".to_string(),
                    title: format!("{} trace points had {reason}", indexes.len()),
                    trace_point_indexes: indexes.to_vec(),
                });
            }
        }

        let snapped_geometry = snapped_geometry(&matched_geometry(&matched_path), &geometry_format)?;
        Ok(SnapToRoadsOutput {
            notices,
            pricing_bucket,
            snapped_geometry,
            snapped_geometry_format: geometry_format,
            snapped_trace_points,
        })
    }
}

/// The bounds of the `TracePoints` list. The Amazon Location Service API reference states
/// for `SnapToRoads` request parameter `TracePoints`: "Array Members: Minimum number of 2
/// items. Maximum number of 5000 items." — a trace is a sequence of positions travelled
/// through, so a single point is not one. This crate's Smithy model omits the `@length`
/// trait on `RoadSnapTracePointList`, so the framework's constraint walk does not enforce
/// it and this handler does; the upper bound also bounds the work one request can ask for.
const MIN_TRACE_POINTS: usize = 2;
const MAX_TRACE_POINTS: usize = 5_000;

/// How many trace points are snapped under one acquisition of the shared state lock.
const SNAP_CHUNK: usize = 256;

/// Most indexes one notice may carry (`RoadSnapNotice$TracePointIndexes` is `@length`
/// 1..1000).
const MAX_NOTICE_INDEXES: usize = 1_000;

/// How certain the match of `position` onto the road `candidate` names is, as
/// `Confidence` ("Confidence value for the correctness of this point match", a
/// `@range 0..1` double) reports it.
///
/// It is read off the MATCH and nothing else. Two things make a map match uncertain, and
/// the score is their product:
///
/// * how far the point had to move to reach the road, over the scale on which a position
///   inside this network's coverage is ambiguous ([`state::snap_ambiguity_m`]) — a point
///   sitting on the road was observed where the road is, one as far from it as a point
///   can be was not;
/// * how much nearer the matched road is than the nearest other road the traveller may
///   use — a point as near a second road could be on either, so the match is a guess
///   between them, while a point with no other road near it can only be on this one.
///
/// The radius the request allowed is deliberately NOT part of it: `SnapRadius` says which
/// points may be matched at all, so scoring against it would report the same match as
/// confident or not depending on how much slack the caller left, and would never signal a
/// poor match at a generous radius.
fn match_confidence(position: Position, candidate: &state::SnappedPosition) -> f64 {
    let ambiguity = state::snap_ambiguity_m(position[1]);
    // A Gaussian error model on the distance from the observed point to a road: the
    // relative likelihood that the point was observed on it. Strictly positive, so a
    // matched point always carries some confidence.
    let likelihood = |offset_m: f64| (-(offset_m / ambiguity).powi(2)).exp();
    let matched = likelihood(candidate.offset_m);
    let runner_up = candidate.runner_up_offset_m.map_or(0.0, likelihood);
    let share = if matched + runner_up > 0.0 {
        matched / (matched + runner_up)
    } else {
        // Both likelihoods underflowed, which leaves no ground to prefer either road.
        0.5
    };
    (matched * share).clamp(0.0, 1.0)
}

/// The geometry of the snapped route: the roads the trace was matched onto, from the first
/// matched point to the last.
///
/// Between two consecutive matched points the line is the stretch of the NETWORK that
/// joins them ([`state::road_path_between`]), not the chord between them: the trace was
/// matched onto roads, so the geometry the answer carries runs along those roads and a
/// client drawing it stays on the network. An unmatched point contributes nothing — the
/// line would otherwise run through a position the same answer reports as matching no
/// road.
fn matched_geometry(matched: &[state::SnappedPosition]) -> Vec<Position> {
    let mut path: Vec<Position> = matched.first().map(|first| vec![first.position]).unwrap_or_default();
    for pair in matched.windows(2) {
        for vertex in state::road_path_between(&pair[0], &pair[1]) {
            if path.last() != Some(&vertex) {
                path.push(vertex);
            }
        }
    }
    path
}

/// The snapped geometry in the requested format. `None` when there is no line to draw — a
/// `LineString` carries two positions at least.
fn snapped_geometry(path: &[Position], format: &str) -> Result<Option<RoadSnapSnappedGeometry>, SnapToRoadsError> {
    if path.len() < 2 {
        return Ok(None);
    }
    if format == "Simple" {
        return Ok(Some(RoadSnapSnappedGeometry {
            line_string: Some(state::line_string(path)),
            polyline: None,
        }));
    }
    Ok(Some(RoadSnapSnappedGeometry {
        line_string: None,
        polyline: Some(
            state::encode_flexible_polyline(path)
                .ok_or_else(|| internal("the snapped geometry could not be encoded as a Flexible Polyline"))?,
        ),
    }))
}

/// The model's own `ValidationException` (HTTP 400) for a request member this service
/// refuses, naming the member and the rule it broke.
fn invalid(member: &str, message: String) -> SnapToRoadsError {
    SnapToRoadsErrorModeled::ValidationException(ValidationException {
        message: message.clone(),
        reason: "FieldValidationFailed".to_string(),
        field_list: vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }],
    })
    .into()
}

/// The model's own `AccessDeniedException` (HTTP 403) for a caller the IAM policies in
/// force do not permit to perform this action, carrying IAM's own sentence
/// ([`crate::state::identity_authorization`]).
fn access_denied(message: String) -> SnapToRoadsError {
    SnapToRoadsErrorModeled::AccessDeniedException(AccessDeniedException { message }).into()
}

/// The model's `InternalServerException` (HTTP 500), for a failure of this emulator
/// rather than of the request.
fn internal(message: &str) -> SnapToRoadsError {
    SnapToRoadsErrorModeled::InternalServerException(InternalServerException {
        message: message.to_string(),
    })
    .into()
}

/// Every per-mode option block `RoadSnapTravelModeOptions` declares, in the model's order,
/// paired with whether the request set it — what [`state::check_travel_mode_options`]
/// holds against the selected `TravelMode`.
fn declared_travel_mode_options(
    options: Option<&RoadSnapTravelModeOptions>,
) -> [(&'static str, bool); 1] {
    [("Truck", options.is_some_and(|set| set.truck.is_some()))]
}
