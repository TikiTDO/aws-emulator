// SPDX-License-Identifier: CC0-1.0

//! The throughput each request consumes, counted into the CloudWatch series DynamoDB
//! publishes it under.
//!
//! A table's consumed capacity is not only a member of the response that asked for it: Amazon
//! DynamoDB publishes it as the `AWS/DynamoDB` metrics `ConsumedReadCapacityUnits` and
//! `ConsumedWriteCapacityUnits`, dimensioned by the table it was consumed on. Those series are what a client
//! graphs, and what every alarm on a table's utilisation is evaluated against — including the
//! ones Application Auto Scaling mints for a target-tracking policy on
//! `dynamodb:table:ReadCapacityUnits` and its three siblings, which is how a provisioned table
//! scales itself under load. A table whose consumption is never published has no utilisation
//! to breach on, so no such alarm can ever fire.
//!
//! The units published are the SAME ones the operation reports as its `ConsumedCapacity`
//! (`crate::state::read_capacity_units` / `crate::state::write_capacity_units`), whether or
//! not the caller asked for them: what a request consumed is a property of the request, not of
//! the member it chose to read back.
//!
//! The datum is one observation of that request's consumption, timestamped when it happened,
//! so a `Sum` over a period is the units the table consumed in that period — which is how the
//! metric is defined and how an alarm on it is evaluated. Nothing here fails a request: a
//! CloudWatch store this engine does not hold, or one at capacity, drops the observation and
//! the request is answered as it was.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

/// The namespace Amazon DynamoDB publishes a table's own metrics under.
const NAMESPACE: &str = "AWS/DynamoDB";

/// The metric that counts the read capacity units a table consumed.
const CONSUMED_READ_CAPACITY_UNITS: &str = "ConsumedReadCapacityUnits";

/// The metric that counts the write capacity units it consumed.
const CONSUMED_WRITE_CAPACITY_UNITS: &str = "ConsumedWriteCapacityUnits";

/// The dimension that names the table a series belongs to.
const TABLE_NAME: &str = "TableName";

/// The storage resolution a standard-resolution metric is published at, in seconds — which is
/// what `PutMetricData` stores for a publisher that names no `StorageResolution`.
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// Which half of a table's provisioned throughput a request consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumed {
    /// Read capacity units.
    Read,
    /// Write capacity units.
    Write,
}

impl Consumed {
    /// The metric name this half of the throughput is counted under.
    fn metric_name(self) -> &'static str {
        match self {
            Self::Read => CONSUMED_READ_CAPACITY_UNITS,
            Self::Write => CONSUMED_WRITE_CAPACITY_UNITS,
        }
    }
}

/// How many observations one table holds before the oldest is dropped.
///
/// The EMULATOR's own bound, so a store nobody publishes from cannot grow without end: the
/// observations are published by the service seam every request of this service passes
/// through ([`crate::service`]), so a table only holds what one request charged it.
const MAX_PENDING_OBSERVATIONS: usize = 4_096;

/// One charge against a table's throughput: how much, of which half, and when.
#[derive(Debug, Clone, Copy)]
pub struct Observation {
    /// The instant the request consumed it at, which is the instant the datapoint carries.
    pub at: ::std::time::SystemTime,
    /// The capacity units it consumed.
    pub units: f64,
    /// Which half of the throughput they came out of.
    pub consumed: Consumed,
}

/// What a table has been charged and this service has not published yet.
///
/// The charge happens where the capacity is taken — under this store's lock, inside the
/// admission every read and every write of a table goes through — and the publish happens
/// with that lock released, in the CloudWatch store. So the observations wait here in
/// between, exactly as a capacity movement waits to be delivered to the service that owns the
/// scaled resource.
#[derive(Debug, Default)]
pub struct Pending(Vec<Observation>);

impl Pending {
    /// Record that this table consumed `units` of `consumed` throughput at `at`.
    ///
    /// A charge of nothing is no observation; past [`MAX_PENDING_OBSERVATIONS`] the oldest
    /// goes, which at worst loses an observation nothing has published in that long.
    pub fn charge(&mut self, consumed: Consumed, units: f64, at: ::std::time::SystemTime) {
        if !units.is_finite() || units <= 0.0 {
            return;
        }
        self.0.push(Observation {
            at,
            units,
            consumed,
        });
        let overflow = self.0.len().saturating_sub(MAX_PENDING_OBSERVATIONS);
        if overflow > 0 {
            self.0.drain(..overflow);
        }
    }

    /// Take everything recorded so far, leaving nothing behind: publishing is at-most-once.
    fn take(&mut self) -> Vec<Observation> {
        ::std::mem::take(&mut self.0)
    }
}

/// Publish what the tables of this store have been charged since the last publish, into the
/// CloudWatch series each table's throughput is reported under.
///
/// Call it with NO store lock held: this takes this service's lock to drain the observations
/// and CloudWatch's to append them, one after the other. It is called from the one seam every
/// request of this service passes through, so what a request charged is published as part of
/// serving it — which is what makes an alarm on a table's utilisation, and the auto scaling
/// built on one, see the load that happened.
pub fn publish_pending_consumed<Eng: Send + Sync + 'static>(engine: &Eng) {
    let Some(state) = crate::state::service_state(engine) else {
        return;
    };
    let charged: Vec<(String, Observation)> = {
        let mut store = state.lock();
        store
            .regions
            .values_mut()
            .flat_map(|tables| tables.values_mut())
            .flat_map(|table| {
                let name = table.name.clone();
                table
                    .pending_consumed
                    .take()
                    .into_iter()
                    .map(move |observation| (name.clone(), observation))
            })
            .collect()
    };
    if charged.is_empty() {
        return;
    }
    let Some(metrics) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let mut store = metrics.lock();
    for (table_name, observation) in charged {
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: observation.consumed.metric_name().to_string(),
            dimensions: [(TABLE_NAME.to_string(), table_name.clone())]
                .into_iter()
                .collect(),
        };
        let datum = cloudwatch::state::StoredDatum {
            timestamp_secs: observation
                .at
                .duration_since(::std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64(),
            sample_count: 1.0,
            sum: observation.units,
            minimum: observation.units,
            maximum: observation.units,
            unit: None,
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The request itself was already served,
            // so nothing is failed for it; the dropped observation is reported where a
            // developer can see it.
            ::tracing::warn!(
                table = %table_name,
                metric = %observation.consumed.metric_name(),
                "the CloudWatch metric store is at capacity, so the throughput this request \
                 consumed was not counted"
            );
        }
    }
}
