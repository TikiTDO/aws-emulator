// SPDX-License-Identifier: CC0-1.0

//! The data source introspections this service has run: the record each one leaves
//! behind, and how the schema it read is reported.
//!
//! `StartDataSourceIntrospection` "Creates a new introspection. Returns the
//! introspectionId of the new introspection after its creation", and "when a new
//! instrospection has been created, the status will be set to PROCESSING. Once the
//! operation has been completed, the status will change to SUCCESS or FAILED depending
//! on how the data was parsed" (`StartDataSourceIntrospectionResponse$introspectionStatus`).
//! So the call SCHEDULES the introspection and leaves the record in the non-terminal
//! [`INTROSPECTION_PROCESSING`] state; the outcome comes due one
//! [`DATA_SOURCE_INTROSPECTION_SPAN`] later and [`IntrospectionRecord::settle`]
//! materialises it. `GetDataSourceIntrospection` "Retrieves the record of an existing
//! introspection", so both calls read the one record kept here and cannot disagree about
//! an introspection.
//!
//! What a successful introspection holds is the SCHEMA of the database the call named:
//! one model per table, with the fields that table's columns declare, the primary key
//! those columns form and the indexes the schema declares over them. That schema is read
//! from the service that owns the cluster's databases when the call arrives
//! ([`super::introspected_cluster`]) — the work is done on the database, never on
//! anything the response needs.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DataSourceIntrospectionModel, DataSourceIntrospectionModelField,
    DataSourceIntrospectionModelFieldType, DataSourceIntrospectionModelIndex,
};

/// The `DataSourceIntrospectionStatus` a scheduled introspection carries until it has
/// been performed: "by default, when a new instrospection has been created, the status
/// will be set to PROCESSING".
pub const INTROSPECTION_PROCESSING: &str = "PROCESSING";

/// The `DataSourceIntrospectionStatus` of an introspection that read the schema.
pub const INTROSPECTION_SUCCESS: &str = "SUCCESS";

/// The `DataSourceIntrospectionStatus` of an introspection that could not read it.
pub const INTROSPECTION_FAILED: &str = "FAILED";

/// How long after a `StartDataSourceIntrospection` the introspection it scheduled comes
/// due.
///
/// A chosen span: the operation reports the introspection in the non-terminal
/// [`INTROSPECTION_PROCESSING`] state, so the span is never zero and a caller reading the
/// record straight after the call still observes that state. Nothing sleeps for it — the
/// due instant is stored on the record and [`IntrospectionRecord::settle`] applies it.
pub const DATA_SOURCE_INTROSPECTION_SPAN: Duration = Duration::from_secs(3);

/// The text a `NotFoundException` (HTTP 404) carries for an `introspectionId` this
/// account holds no introspection record for.
///
/// AppSync's own sentence for the condition, which names what the caller has to change:
/// the ID the request supplied matches no introspection record.
pub const INTROSPECTION_NOT_FOUND_MESSAGE: &str =
    "No introspection was found that matches the supplied ID.";

/// The text a `BadRequestException` (HTTP 400) carries for a call that names no data
/// source to introspect at all.
///
/// Worded after this service's other missing-configuration sentence
/// ([`super::missing_data_source_config_message`]), with the subject this member's own doc
/// names ("the rdsDataApiConfig object data", "the metadata required to introspect the RDS
/// cluster").
pub const MISSING_RDS_DATA_API_CONFIG_MESSAGE: &str = "Missing RDS Data API config";

/// The `kind` of a field whose type is a primitive: "`Scalar`: Indicates the value is a
/// primitive type (scalar)" (`DataSourceIntrospectionModelFieldType$kind`).
pub const SCALAR_FIELD_KIND: &str = "Scalar";

/// The `kind` wrapping the type of a field that cannot be null: "`NonNull`: Indicates the
/// field cannot be `null`" (the same member's doc).
pub const NON_NULL_FIELD_KIND: &str = "NonNull";

/// What an `introspectionId`'s bytes are mixed with, so one can never coincide with
/// another id this state's counter is drawn through (the connection ids and event
/// identifiers of the data planes are mixed with domains of their own).
const INTROSPECTION_ID_MIX_DOMAIN: u64 = 0x4170_7053_796E_6303;

/// How many introspection records this emulator keeps at once.
///
/// A bound on a store a caller grows by calling: a `StartDataSourceIntrospection` creates
/// one record and no operation deletes one, so without a cap a stream of calls would grow
/// the store without limit. The most recent records are the ones kept, the way this
/// service caps its other per-API stores.
pub const MAX_INTROSPECTION_RECORDS: usize = 100;

/// What an introspection settles to once the clock makes it due.
///
/// The outcome is computed when the call arrives, from the database that call named, and
/// kept here until it is due: the transition the clock owes then installs it without
/// reading anything of a later request.
#[derive(Debug, Clone)]
pub enum IntrospectionOutcome {
    /// The database's schema was read; these are the models it defines, in the order
    /// their tables are named.
    Introspected(Vec<IntrospectedModel>),
    /// The schema could not be read, for this reason — "the exact error that was
    /// generated during the operation"
    /// (`GetDataSourceIntrospectionResponse$introspectionStatusDetail`).
    Failed(String),
}

/// One model an introspection found: a table of the database, as this service holds it.
#[derive(Debug, Clone)]
pub struct IntrospectedModel {
    /// The table's name, which is the model's name.
    pub name: String,
    /// One field per column the table declares, in declaration order.
    pub fields: Vec<IntrospectedField>,
    /// The columns the table's PRIMARY KEY covers, in key order, and empty for a table
    /// that declares none.
    pub primary_key: Vec<String>,
    /// The indexes the schema declares over the table, each with the columns it covers,
    /// keyed and ordered by the index's own name.
    pub indexes: BTreeMap<String, Vec<String>>,
}

/// One field of a model: a column of the table, as this service holds it.
#[derive(Debug, Clone)]
pub struct IntrospectedField {
    /// The column's name, which is the field's name.
    pub name: String,
    /// The AppSync scalar the column's values are reported as — one of the names every
    /// AppSync schema holds without defining it ([`super::BUILT_IN_SCALARS`]).
    pub scalar: String,
    /// Whether the column cannot hold a null, which is what wraps the field's type in
    /// [`NON_NULL_FIELD_KIND`].
    pub non_null: bool,
    /// "The length value of the introspected field": the number of characters the
    /// column's declaration bounds it to, and `0` for a column whose type carries no
    /// such bound.
    pub length: i64,
}

impl IntrospectedField {
    /// This field's type as the introspection reports it: the scalar its values are
    /// reported as, wrapped in a non-null type when the column cannot hold a null.
    fn to_field_type(&self) -> DataSourceIntrospectionModelFieldType {
        let scalar = DataSourceIntrospectionModelFieldType {
            kind: Some(SCALAR_FIELD_KIND.to_string()),
            name: Some(self.scalar.clone()),
            r#type: None,
            values: None,
        };
        if !self.non_null {
            return scalar;
        }
        DataSourceIntrospectionModelFieldType {
            kind: Some(NON_NULL_FIELD_KIND.to_string()),
            name: None,
            r#type: Some(Box::new(scalar)),
            values: None,
        }
    }

    /// This field as GraphQL's SDL writes it: `name: Type` with a `!` for a column that
    /// cannot hold a null.
    fn to_sdl_field(&self) -> String {
        let non_null = if self.non_null { "!" } else { "" };
        format!("  {}: {}{non_null}", self.name, self.scalar)
    }
}

impl IntrospectedModel {
    /// This model as the introspection result reports it.
    ///
    /// `include_models_sdl` is the call's own flag: "a boolean flag that determines
    /// whether SDL should be generated for introspected types. If set to true, each model
    /// will contain an sdl property that contains the SDL for that type"
    /// (`GetDataSourceIntrospectionRequest$includeModelsSDL`).
    pub fn to_model(&self, include_models_sdl: bool) -> DataSourceIntrospectionModel {
        DataSourceIntrospectionModel {
            name: Some(self.name.clone()),
            fields: Some(
                self.fields
                    .iter()
                    .map(|field| DataSourceIntrospectionModelField {
                        name: Some(field.name.clone()),
                        r#type: Some(field.to_field_type()),
                        length: field.length,
                    })
                    .collect(),
            ),
            primary_key: (!self.primary_key.is_empty()).then(|| {
                DataSourceIntrospectionModelIndex {
                    name: Some(primary_key_name(&self.name)),
                    fields: Some(self.primary_key.clone()),
                }
            }),
            indexes: Some(
                self.indexes
                    .iter()
                    .map(|(name, columns)| DataSourceIntrospectionModelIndex {
                        name: Some(name.clone()),
                        fields: Some(columns.clone()),
                    })
                    .collect(),
            ),
            sdl: include_models_sdl.then(|| self.to_sdl()),
        }
    }

    /// This model's SDL: the type its fields declare, and nothing else — "the SDL only
    /// contains the type data and no additional metadata or directives".
    fn to_sdl(&self) -> String {
        let fields: Vec<String> = self
            .fields
            .iter()
            .map(IntrospectedField::to_sdl_field)
            .collect();
        format!("type {} {{\n{}\n}}", self.name, fields.join("\n"))
    }
}

/// The name of the index a table's PRIMARY KEY is held in.
///
/// PostgreSQL names the constraint of a primary key the table's own name followed by
/// `_pkey` where the declaration names none, which is what the catalog then holds the
/// index under; the introspected cluster runs PostgreSQL, so that is the name its schema
/// gives the key this model reports.
fn primary_key_name(table: &str) -> String {
    format!("{table}_pkey")
}

/// The record one introspection leaves behind: the status the call that created it left,
/// the detail that goes with that status, and the outcome the clock still owes.
#[derive(Debug, Clone)]
pub struct IntrospectionRecord {
    /// Creation order, so a record can be reported in the order it was created even
    /// though the minted id is opaque.
    pub seq: u64,
    /// The minted `introspectionId` this record is addressed by.
    pub introspection_id: String,
    /// The `DataSourceIntrospectionStatus` the introspection is in.
    pub status: String,
    /// "The error detail field. When a FAILED introspectionStatus is returned, the
    /// introspectionStatusDetail will also return the exact error that was generated
    /// during the operation" — so there is nothing to detail until it has failed.
    pub detail: Option<String>,
    /// The instant the introspection this record scheduled comes due, or `None` once
    /// there is none left owed. Stored rather than slept on: see
    /// [`DATA_SOURCE_INTROSPECTION_SPAN`].
    pub due_at: Option<SystemTime>,
    /// What the introspection settles to when it comes due, or `None` once it has
    /// settled.
    pub outcome: Option<IntrospectionOutcome>,
    /// The models the introspection read, once it has settled to
    /// [`INTROSPECTION_SUCCESS`]: the schema of the database it introspected.
    pub models: Vec<IntrospectedModel>,
}

impl IntrospectionRecord {
    /// The record a `StartDataSourceIntrospection` at `now` leaves behind: the
    /// introspection is SCHEDULED rather than performed, so it is left in the
    /// non-terminal [`INTROSPECTION_PROCESSING`] state with the due instant the clock
    /// settles.
    pub fn scheduled(
        seq: u64,
        introspection_id: String,
        outcome: IntrospectionOutcome,
        now: SystemTime,
    ) -> Self {
        Self {
            seq,
            introspection_id,
            status: INTROSPECTION_PROCESSING.to_string(),
            detail: None,
            due_at: Some(now + DATA_SOURCE_INTROSPECTION_SPAN),
            outcome: Some(outcome),
            models: Vec::new(),
        }
    }

    /// Materialise the introspection the CLOCK already made due at `now`.
    ///
    /// Owed by the clock alone: the due instant was recorded by the call that scheduled
    /// the introspection, and so was its outcome, so the same transition is applied
    /// whether or not anybody read the record in between.
    pub fn settle(&mut self, now: SystemTime) {
        if self.due_at.is_none_or(|due| now < due) {
            return;
        }
        self.due_at = None;
        match self.outcome.take() {
            Some(IntrospectionOutcome::Introspected(models)) => {
                self.models = models;
                self.status = INTROSPECTION_SUCCESS.to_string();
            }
            Some(IntrospectionOutcome::Failed(detail)) => {
                self.status = INTROSPECTION_FAILED.to_string();
                self.detail = Some(detail);
            }
            // Unreachable: a record that is due carries the outcome the call that
            // scheduled it computed, and settling takes it exactly once.
            None => {}
        }
    }

    /// Whether the introspection this record scheduled has been performed and read a
    /// schema, i.e. whether there is a result to report at all: "if the retrieval is
    /// successful, the result of the instrospection will also be returned"
    /// (`GetDataSourceIntrospection`).
    pub fn has_result(&self) -> bool {
        self.status == INTROSPECTION_SUCCESS
    }
}

impl super::ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState {
    /// A fresh `introspectionId`: "each introspection contains a unique ID that can be
    /// used to reference the instrospection record".
    ///
    /// A UUID, the form this service's other opaque references take
    /// ([`super::is_association_id`]), with its bytes drawn from this state's own id
    /// counter rather than from the request, so two introspections of the same cluster
    /// never share one.
    fn mint_introspection_id(&mut self) -> String {
        super::event_data_plane::uuid_from(INTROSPECTION_ID_MIX_DOMAIN, self.next_id_draw())
    }

    /// Apply every introspection the clock owes at `now`, so a record reports the same
    /// status — and the same models — to every caller whether or not another call touched
    /// it first.
    pub fn settle_data_source_introspections(&mut self, now: SystemTime) {
        for introspection in self.data_source_introspections.values_mut() {
            introspection.settle(now);
        }
    }

    /// Schedule the introspection `outcome` was computed for, and report the record it
    /// leaves behind.
    ///
    /// Scheduling is all this does: the introspection itself comes due one
    /// [`DATA_SOURCE_INTROSPECTION_SPAN`] later and [`IntrospectionRecord::settle`]
    /// performs it. Every call creates a record of its own — "each introspection contains
    /// a unique ID" — so nothing a caller sends chooses which record it gets.
    ///
    /// No operation deletes an introspection record, so the records are a store a caller
    /// grows by calling: this emulator keeps the most recent
    /// [`MAX_INTROSPECTION_RECORDS`] of them and forgets the oldest past that, which is
    /// then a record no caller holds the id of any more.
    pub fn start_data_source_introspection(
        &mut self,
        outcome: IntrospectionOutcome,
        now: SystemTime,
    ) -> IntrospectionRecord {
        let seq = self.next_seq();
        let introspection_id = self.mint_introspection_id();
        let record = IntrospectionRecord::scheduled(seq, introspection_id.clone(), outcome, now);
        self.data_source_introspections
            .insert(introspection_id, record.clone());
        while self.data_source_introspections.len() > MAX_INTROSPECTION_RECORDS {
            let oldest = self
                .data_source_introspections
                .values()
                .min_by_key(|kept| kept.seq)
                .map(|kept| kept.introspection_id.clone());
            match oldest {
                Some(oldest) => {
                    self.data_source_introspections.remove(&oldest);
                }
                // Unreachable: a map holding more records than the cap holds one.
                None => break,
            }
        }
        record
    }
}
