// SPDX-License-Identifier: CC0-1.0

//! The route server PROPAGATION store: which route tables each of the account's route
//! servers may install its learned routes into.
//!
//! "Defines which route tables the route server can update with routes. When enabled, route
//! server propagation installs the routes in the FIB on the route table you've specified"
//! (`EnableRouteServerPropagation`) — so a propagation is the (route server, route table)
//! PAIR itself, and the modelled [`RouteServerPropagation`] shape carries no id of its own.
//! That pair is therefore the key of
//! [`ComAmazonawsEc2Amazonec2State::route_server_propagations`], one entry per pair, and a
//! pair either propagates or does not.
//!
//! `State` is a fact of the record's AGE, so [`RouteServerPropagationRecord::state_at`]
//! derives it on read — `pending` until the enable has settled, `available` after it, and
//! `deleting` while a disable is being carried out — rather than letting two writers store
//! two different words for one pair. The catalog has no word for a pair that does not
//! propagate at all — `RouteServerPropagationState` holds `pending`, `available` and
//! `deleting` and nothing else — so once a disable has been carried out the pair is reported
//! no longer, the same reading the transit gateway propagation store takes of its own
//! catalog.
//!
//! A propagation is DERIVED from the association that backs it: route server propagation
//! installs routes into the route tables of the VPC a route server is associated with, so a
//! stored pair is reported only while the account still holds that route table AND the route
//! server is still associated with the VPC that table routes for. A route table that was
//! deleted, or a VPC whose association was dropped, therefore cannot leave behind a
//! propagation into a table that is gone or that this route server no longer serves.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// A propagation whose enable is being carried out: the `pending` member of
/// `RouteServerPropagationState`
/// (`smithy_query(query="shape",shape="RouteServerPropagationState")`).
pub const ROUTE_SERVER_PROPAGATION_STATE_PENDING: &str = "pending";

/// A propagation in force: that same enum's `available` member.
pub const ROUTE_SERVER_PROPAGATION_STATE_AVAILABLE: &str = "available";

/// A propagation whose disable is being carried out: that same enum's `deleting` member.
pub const ROUTE_SERVER_PROPAGATION_STATE_DELETING: &str = "deleting";

/// `InvalidRouteServerId.NotAssociated` (HTTP 400): the route server the request named is
/// not associated with the VPC the route table it named routes for, so there is no
/// association for the routes to be installed through.
///
/// EC2's `Invalid<Resource>Id.<Condition>` spelling, beside
/// [`INVALID_ROUTE_SERVER_ID_NOT_FOUND`] and [`INVALID_ROUTE_SERVER_ID_MALFORMED`], which
/// name the same resource under their own conditions.
pub const INVALID_ROUTE_SERVER_ID_NOT_ASSOCIATED: &str = "InvalidRouteServerId.NotAssociated";

/// One stored route server propagation: the instants the clock owes it its moves, and
/// nothing else.
///
/// The (route server, route table) PAIR is the whole identity of a propagation, so it is the
/// key of [`ComAmazonawsEc2Amazonec2State::route_server_propagations`] and is not repeated on
/// the record.
///
/// The record holds NO caller-chosen bytes: both ids name resources the store already holds
/// and charges for, so a propagation adds nothing to the store's byte budget — only to the
/// record count [`ComAmazonawsEc2Amazonec2State::has_route_server_propagation_capacity`]
/// bounds, exactly like [`RouteServerAssociationRecord`] beside it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RouteServerPropagationRecord {
    /// The instant from which this propagation's `pending` -> `available` move is due.
    /// Monotonic ([`Instant`]) for the reason [`RouteServerRecord::available_at`] is: it is
    /// only ever compared against the clock, never reported, so a wall-clock adjustment
    /// cannot move a propagation back to `pending`.
    #[serde(with = "::service_common::persist::instant")]
    pub available_at: Instant,
    /// The instant from which a disable that has been asked for has been carried out, after
    /// which the pair does not propagate at all. `None` for a propagation nobody has asked
    /// to disable.
    #[serde(with = "::service_common::persist::instant_opt")]
    pub removed_at: Option<Instant>,
}

impl RouteServerPropagationRecord {
    /// A propagation the clock owes its `pending` -> `available` move
    /// [`TRANSITION_DELAY`] after `now`, which is how an enable writes one: the span is
    /// never zero, so a caller reading straight after the enable still sees `pending`.
    pub fn enabled_at(now: Instant) -> Self {
        Self {
            available_at: now + TRANSITION_DELAY,
            removed_at: None,
        }
    }

    /// The propagation's `RouteServerPropagationState` at `now`, or `None` once a disable
    /// has been carried out — at which point the pair does not propagate and so is not a
    /// propagation of that route table any more.
    pub fn state_at(&self, now: Instant) -> Option<&'static str> {
        if let Some(removed_at) = self.removed_at {
            return (now < removed_at).then_some(ROUTE_SERVER_PROPAGATION_STATE_DELETING);
        }
        Some(if now < self.available_at {
            ROUTE_SERVER_PROPAGATION_STATE_PENDING
        } else {
            ROUTE_SERVER_PROPAGATION_STATE_AVAILABLE
        })
    }

    /// The propagation of `route_table_id` by `route_server_id` as of `now`: the pair it
    /// joins, with the state the clock has reached. A read observes the move the clock
    /// already owed without writing anything, and answers `None` for a pair whose disable
    /// has been carried out.
    pub fn view_at(
        &self,
        route_server_id: &str,
        route_table_id: &str,
        now: Instant,
    ) -> Option<RouteServerPropagation> {
        Some(RouteServerPropagation {
            route_server_id: Some(route_server_id.to_string()),
            route_table_id: Some(route_table_id.to_string()),
            state: Some(self.state_at(now)?.to_string()),
        })
    }

    /// Start carrying out a disable of this propagation from `now`, so the pair reports
    /// `deleting` until [`TRANSITION_DELAY`] has passed and is reported no longer after it.
    ///
    /// Idempotent in the only sense that matters to a caller: a propagation already being
    /// disabled keeps the instant its first disable set, so a second call cannot push the
    /// removal further out.
    pub fn begin_removal(&mut self, now: Instant) {
        self.removed_at = Some(self.removed_at.unwrap_or(now + TRANSITION_DELAY));
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another route server propagation fits inside the store's record ceiling
    /// ([`MAX_ROUTE_SERVER_RECORDS`]) — the same contract as
    /// [`Self::has_route_server_association_capacity`], asked of its own store so a full
    /// store of one kind cannot refuse a write of another.
    pub fn has_route_server_propagation_capacity(&self) -> bool {
        self.route_server_propagations.len() < MAX_ROUTE_SERVER_RECORDS
    }

    /// The VPC the route table `route_table_id` routes for, or `None` when the account holds
    /// no such table.
    ///
    /// The ONE place that question is answered for the propagation rules below, so the
    /// enable, the disable and the read cannot disagree about which network a table is in.
    pub fn vpc_of_route_table(&self, route_table_id: &str) -> Option<&str> {
        self.route_tables
            .get(route_table_id)?
            .vpc_id
            .as_deref()
    }

    /// Whether the route server `route_server_id` is associated with the VPC the route table
    /// `route_table_id` routes for — the association route server propagation installs its
    /// routes through.
    ///
    /// `false` for a table the account does not hold, there being no network to be
    /// associated with.
    pub fn route_server_serves_route_table(&self, route_server_id: &str, route_table_id: &str) -> bool {
        self.vpc_of_route_table(route_table_id)
            .is_some_and(|vpc_id| {
                self.route_server_associations
                    .contains_key(&(route_server_id.to_string(), vpc_id.to_string()))
            })
    }

    /// The propagation of `route_table_id` by `route_server_id` as of `now`, or `None` when
    /// the pair does not propagate.
    ///
    /// A stored pair is a propagation only while the association that backs it holds and the
    /// route table is still one the account has (see the module docs), so this is also the
    /// one place an enable asks whether there is already a propagation to refuse.
    pub fn route_server_propagation_at(
        &self,
        route_server_id: &str,
        route_table_id: &str,
        now: Instant,
    ) -> Option<RouteServerPropagation> {
        if !self.route_server_serves_route_table(route_server_id, route_table_id) {
            return None;
        }
        self.route_server_propagations
            .get(&(route_server_id.to_string(), route_table_id.to_string()))?
            .view_at(route_server_id, route_table_id, now)
    }

    /// Every propagation of the route server `route_server_id` as of `now`, in
    /// `RouteTableId` order — the order the store itself is kept in — narrowed to one route
    /// table when `route_table_id` names one.
    pub fn route_server_propagations_at(
        &self,
        route_server_id: &str,
        route_table_id: Option<&str>,
        now: Instant,
    ) -> Vec<RouteServerPropagation> {
        self.route_server_propagations
            .keys()
            .filter(|(propagating_server, propagated_table)| {
                propagating_server == route_server_id
                    && route_table_id.is_none_or(|named| named == propagated_table)
            })
            .filter_map(|(propagating_server, propagated_table)| {
                self.route_server_propagation_at(propagating_server, propagated_table, now)
            })
            .collect()
    }

    /// Record that `route_server_id` propagates its routes into `route_table_id` from `now`,
    /// and answer the propagation as of that instant.
    ///
    /// Written in the non-terminal `pending` state with the clock owing it `available` one
    /// [`TRANSITION_DELAY`] later, the same span every other clock-owed move of this crate
    /// takes, so a caller reading straight after the write still sees `pending`. A pair whose
    /// earlier disable has already been carried out is written afresh, so the record the
    /// store keeps is the one this enable made.
    pub fn insert_route_server_propagation(
        &mut self,
        route_server_id: &str,
        route_table_id: &str,
        now: Instant,
    ) -> RouteServerPropagation {
        // A pair whose disable the clock already carried out is not a propagation any more
        // (see the module docs), so the records holding one are dropped here, on a write
        // path: each was due at its own instant whether or not anybody called again.
        self.route_server_propagations
            .retain(|_, record| record.state_at(now).is_some());
        let record = RouteServerPropagationRecord::enabled_at(now);
        let view = RouteServerPropagation {
            route_server_id: Some(route_server_id.to_string()),
            route_table_id: Some(route_table_id.to_string()),
            state: Some(ROUTE_SERVER_PROPAGATION_STATE_PENDING.to_string()),
        };
        self.route_server_propagations.insert(
            (route_server_id.to_string(), route_table_id.to_string()),
            record,
        );
        view
    }

    /// Start carrying out a disable of the propagation of `route_table_id` by
    /// `route_server_id` from `now`, and answer it as it now stands — `None` when the pair
    /// held no propagation to disable.
    pub fn begin_route_server_propagation_removal(
        &mut self,
        route_server_id: &str,
        route_table_id: &str,
        now: Instant,
    ) -> Option<RouteServerPropagation> {
        // A pair the store holds no propagation for — or one whose propagation the backing
        // association or route table no longer makes a propagation at all — has nothing to
        // disable, which this one resolver answers for.
        self.route_server_propagation_at(route_server_id, route_table_id, now)?;
        let record = self
            .route_server_propagations
            .get_mut(&(route_server_id.to_string(), route_table_id.to_string()))?;
        record.begin_removal(now);
        record.view_at(route_server_id, route_table_id, now)
    }
}

/// Refuse a propagation request whose route server is not associated with the VPC the route
/// table it named routes for: HTTP 400 [`INVALID_ROUTE_SERVER_ID_NOT_ASSOCIATED`].
///
/// The resource is named in the message the way EC2 names this resource TYPE
/// ([`ROUTE_SERVER_TAGGABLE_RESOURCE`]), like [`invalid_route_server_id_not_found`], and the
/// values come from the REQUEST and from the store: the route server and route table the
/// request named, and the VPC the store says that table routes for.
pub fn invalid_route_server_id_not_associated<E>(
    route_server_id: &str,
    route_table_id: &str,
    vpc_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_SERVER_ID_NOT_ASSOCIATED,
        400,
        format!(
            "The {ROUTE_SERVER_TAGGABLE_RESOURCE} ID '{route_server_id}' is not associated \
             with the vpc ID '{vpc_id}' of routeTable ID '{route_table_id}'"
        ),
    )
}

/// Resolve the (route server, route table) pair a propagation request names, or refuse it.
///
/// The ONE place the rule `EnableRouteServerPropagation` and `DisableRouteServerPropagation`
/// share is written, so they cannot disagree about one pair:
///
/// * both ids are `@required` but modelled `clientOptional`, so one the request did not
///   carry arrives as an empty string and is EC2's `MissingParameter` rather than an id to
///   look up;
/// * the route server must be one the account holds, a malformed id being told from an
///   unknown one by the crate's one resolver for that lookup
///   ([`require_known_route_server`]);
/// * the route table must be one the account holds, or there is nothing to propagate into
///   ([`invalid_route_table_id_not_found`]);
/// * the route server must be associated with the VPC that table routes for, that
///   association being the path propagation installs routes through
///   ([`invalid_route_server_id_not_associated`]).
///
/// Nothing here writes, so a caller can ask the question before its `DryRun` arm and refuse
/// a dry run that would not have succeeded with the reason it would have failed for.
pub fn require_route_server_propagation_pair<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    route_server_id: &str,
    route_table_id: &str,
) -> Result<(), Scaffolded<E>> {
    for (parameter, value) in [
        ("routeServerId", route_server_id),
        ("routeTableId", route_table_id),
    ] {
        if value.is_empty() {
            return Err(missing_parameter(format!(
                "The request must contain the parameter {parameter}"
            )));
        }
    }
    require_known_route_server(store, route_server_id)?;
    let Some(vpc_id) = store.vpc_of_route_table(route_table_id) else {
        return Err(invalid_route_table_id_not_found(route_table_id));
    };
    if !store.route_server_serves_route_table(route_server_id, route_table_id) {
        return Err(invalid_route_server_id_not_associated(
            route_server_id,
            route_table_id,
            vpc_id,
        ));
    }
    Ok(())
}
