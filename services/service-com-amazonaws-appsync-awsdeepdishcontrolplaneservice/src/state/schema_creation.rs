// SPDX-License-Identifier: CC0-1.0

//! The schema a GraphQL API holds, and the asynchronous creation that installs one.
//!
//! `StartSchemaCreation` "Adds a new schema to your GraphQL API. This operation is
//! asynchronous", so the call SCHEDULES the creation and leaves the API in the
//! non-terminal `PROCESSING` state; the outcome comes due one
//! [`SCHEMA_CREATION_SPAN`] later and [`GraphqlApiRecord::settle_schema_creation`]
//! materialises it. `GetSchemaCreationStatus` is what a caller then "use[s] to
//! determine when it has completed", so both calls read the one record kept here and
//! cannot disagree about a schema.
//!
//! The document IS the API's type system: a creation that succeeds leaves the API
//! holding exactly the types its document declares, which is what `GetType` answers
//! afterwards. A document the GraphQL type system cannot hold together settles to
//! `FAILED`, carrying the reason as the status detail — the work is done on the
//! document the call carried, never on anything the response needs.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use super::{GraphqlApiRecord, TypeRecord};

/// The `SchemaStatus` a scheduled schema creation carries until it has been performed:
/// the schema is "PROCESSING".
pub const SCHEMA_PROCESSING: &str = "PROCESSING";

/// The `SchemaStatus` of a schema creation that has been performed.
pub const SCHEMA_SUCCESS: &str = "SUCCESS";

/// The `SchemaStatus` of a schema creation that could not be performed, because the
/// document it carried defines no type system.
pub const SCHEMA_FAILED: &str = "FAILED";

/// The `SchemaStatus` of an API no schema creation has ever run for: none of the other
/// states applies to it, which is what [`SCHEMA_NOT_APPLICABLE`] names.
pub const SCHEMA_NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// The `details` that goes with [`SCHEMA_PROCESSING`].
pub const SCHEMA_PROCESSING_DETAIL: &str =
    "The schema creation is scheduled and has not been performed yet.";

/// The `details` that goes with [`SCHEMA_SUCCESS`].
pub const SCHEMA_SUCCESS_DETAIL: &str = "The schema was created and is ready for use.";

/// How long after a `StartSchemaCreation` the creation it scheduled comes due.
///
/// A chosen span: the operation is asynchronous and reports the schema in the
/// non-terminal [`SCHEMA_PROCESSING`] state, so the span is never zero and a caller
/// acting straight after the call still observes that state and the refusals that go
/// with it. Nothing sleeps for it — the due instant is stored on the record and
/// [`GraphqlApiRecord::settle_schema_creation`] applies it.
pub const SCHEMA_CREATION_SPAN: Duration = Duration::from_secs(3);

/// What a scheduled schema creation settles to once the clock makes it due.
///
/// The outcome is computed when the call arrives, from the document that call carried,
/// and kept here until it is due: the transition the clock owes then installs it
/// without reading anything of a later request.
#[derive(Debug, Clone)]
pub enum SchemaCreationOutcome {
    /// The document defines a type system; these are the types it declares, keyed as
    /// every type operation addresses one — by name within its API.
    Created(BTreeMap<String, TypeRecord>),
    /// The document defines no type system, for this reason.
    Failed(String),
}

/// The schema creation of one GraphQL API: the document the last call carried, the
/// `SchemaStatus` that call left behind, and the outcome the clock still owes.
#[derive(Debug, Clone)]
pub struct SchemaCreationRecord {
    /// The schema document the last `StartSchemaCreation` carried, kept verbatim.
    pub definition: String,
    /// The `SchemaStatus` the schema is in.
    pub status: String,
    /// "Detailed information about the status of the schema creation operation" — the
    /// text that goes with [`Self::status`].
    pub details: String,
    /// The instant the creation this record scheduled comes due, or `None` once there
    /// is no creation left owed. Stored rather than slept on: see
    /// [`SCHEMA_CREATION_SPAN`].
    pub due_at: Option<SystemTime>,
    /// What the creation settles to when it comes due, or `None` once it has settled.
    pub outcome: Option<SchemaCreationOutcome>,
}

impl SchemaCreationRecord {
    /// The record a `StartSchemaCreation` at `now` leaves behind: the creation is
    /// SCHEDULED rather than performed, so the schema is left in the non-terminal
    /// [`SCHEMA_PROCESSING`] state with the due instant the clock settles.
    pub fn scheduled(definition: String, outcome: SchemaCreationOutcome, now: SystemTime) -> Self {
        Self {
            definition,
            status: SCHEMA_PROCESSING.to_string(),
            details: SCHEMA_PROCESSING_DETAIL.to_string(),
            due_at: Some(now + SCHEMA_CREATION_SPAN),
            outcome: Some(outcome),
        }
    }

    /// Whether the creation this record scheduled has not been performed yet, i.e.
    /// whether a modification of this API's schema is still in progress.
    pub fn is_processing(&self) -> bool {
        self.due_at.is_some()
    }
}

impl GraphqlApiRecord {
    /// Whether a schema creation of this API is still in progress.
    pub fn schema_creation_in_progress(&self) -> bool {
        self.schema
            .as_ref()
            .is_some_and(SchemaCreationRecord::is_processing)
    }

    /// Schedule the creation of the schema `definition` declares, and report the
    /// `SchemaStatus` the API is left in.
    ///
    /// Scheduling is all this does: the creation itself comes due one
    /// [`SCHEMA_CREATION_SPAN`] later and [`Self::settle_schema_creation`] performs it.
    /// A new call REPLACES the record a previous creation left behind — a schema
    /// document is the whole type system, so the last one that was created is the one
    /// the API holds.
    pub fn start_schema_creation(
        &mut self,
        definition: String,
        outcome: SchemaCreationOutcome,
        now: SystemTime,
    ) -> String {
        let record = SchemaCreationRecord::scheduled(definition, outcome, now);
        let status = record.status.clone();
        self.schema = Some(record);
        status
    }

    /// Materialise the schema creation the CLOCK already made due at `now`.
    ///
    /// Owed by the clock alone: the due instant was recorded by the call that scheduled
    /// the creation, and so was its outcome, so the same transition is applied whether
    /// or not anybody read the API in between. A creation that succeeds installs the
    /// types its document declares AS the API's types: the document is the whole type
    /// system, so a type the previous schema held and this document does not declare is
    /// no longer part of the API.
    pub fn settle_schema_creation(&mut self, now: SystemTime) {
        if self
            .schema
            .as_ref()
            .and_then(|schema| schema.due_at)
            .is_none_or(|due| now < due)
        {
            return;
        }
        let outcome = self.schema.as_mut().and_then(|schema| {
            schema.due_at = None;
            schema.outcome.take()
        });
        let (status, details) = match outcome {
            Some(SchemaCreationOutcome::Created(types)) => {
                self.types = types;
                // The document itself is kept too: it declares what the type map cannot
                // hold — the root operation types a `schema` definition binds and the
                // directives the schema defines — which the introspection schema reports.
                self.schema_document = self.schema.as_ref().map(|schema| schema.definition.clone());
                (SCHEMA_SUCCESS, SCHEMA_SUCCESS_DETAIL.to_string())
            }
            Some(SchemaCreationOutcome::Failed(detail)) => (SCHEMA_FAILED, detail),
            // Unreachable: a record that is due carries the outcome the call that
            // scheduled it computed, and settling takes it exactly once.
            None => return,
        };
        if let Some(schema) = self.schema.as_mut() {
            schema.status = status.to_string();
            schema.details = details;
        }
    }

    /// The `SchemaStatus` of this API's schema and the detail that goes with it, as
    /// `GetSchemaCreationStatus` reports them.
    ///
    /// An API no schema creation has ever run for is in [`SCHEMA_NOT_APPLICABLE`] and
    /// has no detail to report: there is no creation to detail.
    pub fn schema_creation_status(&self) -> (String, Option<String>) {
        match self.schema.as_ref() {
            Some(schema) => (schema.status.clone(), Some(schema.details.clone())),
            None => (SCHEMA_NOT_APPLICABLE.to_string(), None),
        }
    }
}

impl super::ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
    /// Apply every schema creation the clock owes at `now`, so an API reports the same
    /// schema — and the same types — to every caller whether or not another call touched
    /// it first.
    ///
    /// The merges the clock owes are performed straight after, because a merge carries
    /// the types the creations above install and is due only once the source API's
    /// creation is ([`Self::settle_due_merges`]): the two dues are one schema transition
    /// of the APIs they touch, so every caller that materialises one materialises both
    /// and no two reads can disagree about a merged type.
    pub fn settle_schema_creations(&mut self, now: SystemTime) {
        for api in self.graphql_apis.values_mut() {
            api.settle_schema_creation(now);
        }
        self.settle_due_merges(now);
    }
}
