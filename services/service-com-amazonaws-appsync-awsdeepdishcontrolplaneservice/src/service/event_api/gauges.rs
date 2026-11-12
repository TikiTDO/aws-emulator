// SPDX-License-Identifier: CC0-1.0

//! The periodic SAMPLING of the two `AWS/AppSync` gauges an Event API's data plane
//! reports: how many realtime connections are open on the API, and how many
//! subscriptions those connections hold.
//!
//! A gauge is not a count of events. Each of these two series measures a quantity that HAS
//! a value at every instant, so a reader laying periods over it expects a reading of the
//! quantity IN each period — including a period after it fell, which reads the lower value,
//! and every later period while it stays there. A series written only when the quantity
//! CHANGES cannot answer that: the last change is the last datapoint, so a reader windowing
//! any period after it finds no data at all and the value it last saw looks like the value
//! the quantity still has. A subscriber that unsubscribed on a connection it deliberately
//! holds open is exactly that case — the subscriptions fall to zero while the connections
//! do not — and an alarm or dashboard built on the subscription gauge would latch on the
//! reading taken before the drop.
//!
//! So the gauges are OBSERVED on a cadence ([`SAMPLE_INTERVAL`]) for as long as the API
//! holds a connection, which is what makes every period of that time carry a reading of
//! what the plane holds then. Three properties of it, each of them observable:
//!
//! * IT IS OFF THE RESPONSE PATH. No client call produces these datapoints: they are what
//!   an operator reads, so they exist whether or not anybody is calling. The sampler is
//!   claimed by the write that gives the API something to sample — a realtime connection
//!   being opened ([`drive_gauge_sampling`]) — and nothing on a read path starts, advances
//!   or stops it.
//! * NOTHING IS SAMPLED FOR A PLANE NOTHING IS CONNECTED TO. The sampler stands down once
//!   the API holds no connection
//!   (`crate::state::event_data_plane::EventDataPlane::gauge_sampling_wanted_or_release`),
//!   so an API nobody has connected to carries no datapoint in the window rather than a run
//!   of zeros, and the final reading of a plane that emptied is the one the closing
//!   connection published.
//! * THE READINGS ARE THE STORE'S. Every sample is the count the store holds AT that moment
//!   ([`super::PlaneContext::connection_counts`]); this module keeps no count of its own, so
//!   a gauge and the plane it measures cannot drift.

use ::std::time::Duration;

use super::PlaneContext;

/// How often the two gauges are observed while the API holds a connection.
///
/// An EMULATOR-CHOSEN cadence — AppSync publishes the series, not a sampling frequency —
/// and deliberately shorter than the one-minute resolution the datapoints are stored at
/// ([`super::metrics`]), so every minute of a held-open connection carries several readings
/// and the minute after a change reads what the plane holds rather than nothing. It is
/// non-zero, so the sampler hands the runtime back between samples instead of spinning.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(5);

/// Start the gauge sampler of the API `context` serves, if one is not already running.
///
/// Called by the handshake that has just recorded a realtime connection on the API: that is
/// the write which gives the plane a quantity to observe. At most one sampler runs per API
/// (the store carries the claim), it samples for as long as the API holds a connection, and
/// with no async runtime to spawn onto there is no sampler — the readings are then the ones
/// the connection's own lifecycle publishes.
pub fn drive_gauge_sampling(context: &PlaneContext) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = context
        .state
        .lock()
        .event_data_plane
        .claim_gauge_sampler(&context.api_id)
    else {
        return;
    };
    let sampling = context.clone();
    let handle = runtime.spawn(async move {
        // The sampler ends when the API holds no connection left to observe, which it sees
        // — and releases its claim in — inside one critical section.
        while wanted(&sampling, generation) {
            // Every sample waits first, which is what hands the runtime back between
            // them. No store lock is held across it.
            ::tokio::time::sleep(SAMPLE_INTERVAL).await;
            publish_sample(&sampling);
        }
    });
    context
        .state
        .lock()
        .event_data_plane
        .set_gauge_sampler(&context.api_id, generation, handle);
}

/// Take ONE sample of the API's two gauges and publish it, while the API holds a
/// connection.
///
/// The counts are READ from the store and published as they stand: nothing is written to
/// this service's own records, so a sample changes nothing a later caller sees. A plane that
/// emptied between two samples publishes nothing — its last reading is the one the closing
/// connection published — so the series carries readings only for the time the API was
/// connected to.
fn publish_sample(context: &PlaneContext) {
    let counts = context.connection_counts();
    if counts.connections == 0 {
        return;
    }
    context.metrics.sampled(counts);
}

/// Whether this API still has gauges to sample, releasing the claim `generation` holds when
/// it has not.
fn wanted(context: &PlaneContext, generation: u64) -> bool {
    context
        .state
        .lock()
        .event_data_plane
        .gauge_sampling_wanted_or_release(&context.api_id, generation)
}
