// SPDX-License-Identifier: CC0-1.0

//! A table's Iceberg metadata, as this service stores it and as an Iceberg
//! catalog client reads it back.
//!
//! The control plane's `CreateTable` takes the table's own description — its
//! schema, its partition spec, its sort order and its properties — and S3 Tables
//! writes that description into the table's first metadata file. Everything an
//! Iceberg engine then asks the catalog about the table (its columns and their
//! field ids, where its files live, which sort order it writes in, what UUID
//! identifies it) is read out of that document, and so is the one precondition
//! the control plane itself reads from it: a compaction strategy that orders
//! records by the table's sort order needs the table to have one.
//!
//! The records below are therefore the table's metadata, held once and
//! projected two ways: [`IcebergTableMetadata::document`] renders the Iceberg
//! table metadata document a catalog read answers with, and
//! [`IcebergTableMetadata::carries_sort_order`] answers the control plane's
//! question about it. A commit through the catalog's own update endpoint goes
//! through [`IcebergTableMetadata::committed`], which applies the requested
//! updates to a COPY and hands it back, so a commit that cannot be applied
//! whole changes nothing.
//!
//! Nothing here is invented per read: a document is rendered from the stored
//! record, and the record only changes where a request changed it.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

/// The Iceberg table format version this service writes a table's metadata at.
/// S3 Tables creates Apache Iceberg v2 tables.
pub const ICEBERG_FORMAT_VERSION: i64 = 2;

/// The order id the Iceberg table specification reserves for the unsorted
/// order — the one order that names no field to sort by, which every table that
/// is not sorted carries.
pub const UNSORTED_ORDER_ID: i32 = 0;

/// The spec id the Iceberg table specification gives the first partition spec,
/// which for a table that partitions nothing is the empty spec.
pub const INITIAL_SPEC_ID: i32 = 0;

/// The schema id the first schema of a table carries.
pub const INITIAL_SCHEMA_ID: i32 = 0;

/// The first partition field id the Iceberg table specification assigns
/// (partition field ids are numbered from 1000), so a table that partitions
/// nothing has assigned none and its last assigned id is one below this.
pub const FIRST_PARTITION_FIELD_ID: i32 = 1000;

/// A sort order id an update may name to mean "the order this commit just
/// added", as the Iceberg REST catalog specification defines the value -1 for
/// the `set-default-sort-order`, `set-current-schema` and `set-default-spec`
/// updates.
pub const LAST_ADDED_ID: i32 = -1;

/// The most schemas, partition specs or sort orders this emulator keeps in one
/// table's metadata.
///
/// CHOSEN, not an AWS-published figure: a commit ADDS to the table's metadata
/// and nothing removes from it, so a caller that commits over and over would
/// otherwise grow the process's memory without limit. What matters is that the
/// document is BOUNDED; a commit that would pass the bound is refused as the
/// client request error it is.
pub const METADATA_ENTRIES_PER_KIND: usize = 100;

/// The most properties this emulator keeps in one table's metadata, bounded for
/// the same reason as [`METADATA_ENTRIES_PER_KIND`].
pub const PROPERTIES_PER_TABLE: usize = 1_000;

/// `at` as Unix epoch milliseconds, which is how the Iceberg table metadata
/// document carries an instant.
pub fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}

/// One column of a table's schema.
///
/// `field_type` is the column's Iceberg type as the document states it: the
/// text of a primitive (`"string"`, `"decimal(10, 2)"`) or the object a nested
/// type is spelled as, held as the document it was declared with so what the
/// catalog reads back is the type the table was created with.
#[derive(Debug, Clone, PartialEq)]
pub struct IcebergField {
    pub id: i32,
    pub name: String,
    pub field_type: Value,
    pub required: bool,
    pub doc: Option<String>,
}

impl IcebergField {
    /// The field as the metadata document carries it.
    fn document(&self) -> Value {
        let mut members = Map::new();
        members.insert("id".to_string(), Value::from(self.id));
        members.insert("name".to_string(), Value::from(self.name.clone()));
        members.insert("required".to_string(), Value::from(self.required));
        members.insert("type".to_string(), self.field_type.clone());
        if let Some(doc) = &self.doc {
            members.insert("doc".to_string(), Value::from(doc.clone()));
        }
        Value::Object(members)
    }
}

/// One schema of a table: the columns it declares, under the id the table
/// refers to it by.
#[derive(Debug, Clone, PartialEq)]
pub struct IcebergSchemaRecord {
    pub schema_id: i32,
    pub fields: Vec<IcebergField>,
    /// The columns that identify a row, by field id. Empty when the schema
    /// names none.
    pub identifier_field_ids: Vec<i32>,
}

impl IcebergSchemaRecord {
    /// The schema as the metadata document carries it: a struct of its fields.
    fn document(&self) -> Value {
        let mut members = Map::new();
        members.insert("type".to_string(), Value::from("struct"));
        members.insert("schema-id".to_string(), Value::from(self.schema_id));
        members.insert(
            "fields".to_string(),
            Value::Array(self.fields.iter().map(IcebergField::document).collect()),
        );
        if !self.identifier_field_ids.is_empty() {
            members.insert(
                "identifier-field-ids".to_string(),
                Value::Array(
                    self.identifier_field_ids
                        .iter()
                        .copied()
                        .map(Value::from)
                        .collect(),
                ),
            );
        }
        Value::Object(members)
    }

    /// The largest field id this schema assigns, or zero when it declares no
    /// column.
    fn last_column_id(&self) -> i32 {
        self.fields.iter().map(|field| field.id).max().unwrap_or(0)
    }
}

/// One partition field: a column, the transform applied to it and the name the
/// partition value is written under.
#[derive(Debug, Clone, PartialEq)]
pub struct IcebergPartitionFieldRecord {
    pub source_id: i32,
    pub field_id: i32,
    pub name: String,
    pub transform: String,
}

/// One partition spec of a table.
#[derive(Debug, Clone, PartialEq)]
pub struct IcebergPartitionSpecRecord {
    pub spec_id: i32,
    pub fields: Vec<IcebergPartitionFieldRecord>,
}

impl IcebergPartitionSpecRecord {
    /// The spec as the metadata document carries it.
    fn document(&self) -> Value {
        let fields = self
            .fields
            .iter()
            .map(|field| {
                let mut members = Map::new();
                members.insert("source-id".to_string(), Value::from(field.source_id));
                members.insert("field-id".to_string(), Value::from(field.field_id));
                members.insert("name".to_string(), Value::from(field.name.clone()));
                members.insert("transform".to_string(), Value::from(field.transform.clone()));
                Value::Object(members)
            })
            .collect();
        let mut members = Map::new();
        members.insert("spec-id".to_string(), Value::from(self.spec_id));
        members.insert("fields".to_string(), Value::Array(fields));
        Value::Object(members)
    }

    /// The largest partition field id this spec assigns, or `None` when it
    /// partitions nothing.
    fn last_partition_id(&self) -> Option<i32> {
        self.fields.iter().map(|field| field.field_id).max()
    }
}

/// One sort field: a column, the transform applied to it and the order its
/// values and nulls are written in.
#[derive(Debug, Clone, PartialEq)]
pub struct IcebergSortFieldRecord {
    pub source_id: i32,
    pub transform: String,
    pub direction: String,
    pub null_order: String,
}

/// One sort order of a table. An order with no field is the unsorted order.
#[derive(Debug, Clone, PartialEq)]
pub struct IcebergSortOrderRecord {
    pub order_id: i32,
    pub fields: Vec<IcebergSortFieldRecord>,
}

impl IcebergSortOrderRecord {
    /// The unsorted order: the order every table that is not sorted writes in.
    pub fn unsorted() -> Self {
        Self {
            order_id: UNSORTED_ORDER_ID,
            fields: Vec::new(),
        }
    }

    /// The order as the metadata document carries it.
    fn document(&self) -> Value {
        let fields = self
            .fields
            .iter()
            .map(|field| {
                let mut members = Map::new();
                members.insert("source-id".to_string(), Value::from(field.source_id));
                members.insert("transform".to_string(), Value::from(field.transform.clone()));
                members.insert("direction".to_string(), Value::from(field.direction.clone()));
                members.insert(
                    "null-order".to_string(),
                    Value::from(field.null_order.clone()),
                );
                Value::Object(members)
            })
            .collect();
        let mut members = Map::new();
        members.insert("order-id".to_string(), Value::from(self.order_id));
        members.insert("fields".to_string(), Value::Array(fields));
        Value::Object(members)
    }
}

/// A table's Iceberg metadata.
///
/// One record per table, written when the table's first metadata file is and
/// replaced by what a catalog commit makes of it. `metadata_version` counts the
/// metadata files written for the table, so each commit names a new file under
/// the table's warehouse location.
#[derive(Debug, Clone, PartialEq)]
pub struct IcebergTableMetadata {
    /// The UUID that identifies this table for the life of the record, whatever
    /// the table is renamed to.
    pub table_uuid: String,
    /// The warehouse location the table's data and metadata files live under.
    pub location: String,
    pub last_updated_ms: i64,
    pub schemas: Vec<IcebergSchemaRecord>,
    pub current_schema_id: i32,
    pub partition_specs: Vec<IcebergPartitionSpecRecord>,
    pub default_spec_id: i32,
    pub sort_orders: Vec<IcebergSortOrderRecord>,
    pub default_sort_order_id: i32,
    pub properties: BTreeMap<String, String>,
    /// The version prefix of the table's current metadata file: zero for the
    /// file the create wrote, one higher for each commit after it.
    pub metadata_version: u32,
}

impl IcebergTableMetadata {
    /// The schema the table currently writes under, when the record holds one.
    pub fn current_schema(&self) -> Option<&IcebergSchemaRecord> {
        self.schemas
            .iter()
            .find(|schema| schema.schema_id == self.current_schema_id)
    }

    /// The sort order the table currently writes in, when the record holds one.
    pub fn default_sort_order(&self) -> Option<&IcebergSortOrderRecord> {
        self.sort_orders
            .iter()
            .find(|order| order.order_id == self.default_sort_order_id)
    }

    /// Whether the table's metadata carries a sort order: a default order that
    /// names at least one field to sort by. The unsorted order names none, so a
    /// table carrying only it is a table with no sort order.
    pub fn carries_sort_order(&self) -> bool {
        self.default_sort_order()
            .is_some_and(|order| !order.fields.is_empty())
    }

    /// The largest field id any schema of the table has assigned.
    fn last_column_id(&self) -> i32 {
        self.schemas
            .iter()
            .map(IcebergSchemaRecord::last_column_id)
            .max()
            .unwrap_or(0)
    }

    /// The largest partition field id any spec of the table has assigned, which
    /// for a table that partitions nothing is one below the first id the
    /// specification assigns.
    fn last_partition_id(&self) -> i32 {
        self.partition_specs
            .iter()
            .filter_map(IcebergPartitionSpecRecord::last_partition_id)
            .max()
            .unwrap_or(FIRST_PARTITION_FIELD_ID.saturating_sub(1))
    }

    /// The Iceberg table metadata document for this record: what a catalog read
    /// answers with, and what a commit answers with once applied.
    ///
    /// The table holds no snapshot: this emulator serves no Iceberg data plane,
    /// so no data file has ever been committed to the table and the document
    /// states exactly that rather than claiming a snapshot nothing wrote.
    pub fn document(&self) -> Value {
        let mut members = Map::new();
        members.insert(
            "format-version".to_string(),
            Value::from(ICEBERG_FORMAT_VERSION),
        );
        members.insert(
            "table-uuid".to_string(),
            Value::from(self.table_uuid.clone()),
        );
        members.insert("location".to_string(), Value::from(self.location.clone()));
        members.insert("last-sequence-number".to_string(), Value::from(0));
        members.insert(
            "last-updated-ms".to_string(),
            Value::from(self.last_updated_ms),
        );
        members.insert(
            "last-column-id".to_string(),
            Value::from(self.last_column_id()),
        );
        members.insert(
            "schemas".to_string(),
            Value::Array(
                self.schemas
                    .iter()
                    .map(IcebergSchemaRecord::document)
                    .collect(),
            ),
        );
        members.insert(
            "current-schema-id".to_string(),
            Value::from(self.current_schema_id),
        );
        members.insert(
            "partition-specs".to_string(),
            Value::Array(
                self.partition_specs
                    .iter()
                    .map(IcebergPartitionSpecRecord::document)
                    .collect(),
            ),
        );
        members.insert(
            "default-spec-id".to_string(),
            Value::from(self.default_spec_id),
        );
        members.insert(
            "last-partition-id".to_string(),
            Value::from(self.last_partition_id()),
        );
        members.insert(
            "sort-orders".to_string(),
            Value::Array(
                self.sort_orders
                    .iter()
                    .map(IcebergSortOrderRecord::document)
                    .collect(),
            ),
        );
        members.insert(
            "default-sort-order-id".to_string(),
            Value::from(self.default_sort_order_id),
        );
        members.insert(
            "properties".to_string(),
            Value::Object(
                self.properties
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::from(value.clone())))
                    .collect(),
            ),
        );
        // No snapshot has ever been committed to the table, so there is no
        // current snapshot, no reference and no log of either.
        members.insert("current-snapshot-id".to_string(), Value::from(-1));
        members.insert("refs".to_string(), Value::Object(Map::new()));
        members.insert("snapshots".to_string(), Value::Array(Vec::new()));
        members.insert("snapshot-log".to_string(), Value::Array(Vec::new()));
        members.insert("metadata-log".to_string(), Value::Array(Vec::new()));
        Value::Object(members)
    }

    /// This record with `updates` applied, or the reason the commit was not
    /// applied.
    ///
    /// Applied to a COPY: a commit is all of its updates or none of them, so a
    /// commit this refuses leaves the table's metadata exactly as it was.
    /// `at` is the instant the commit is made, which the new metadata records as
    /// the table's last update.
    pub fn committed(
        &self,
        updates: Vec<MetadataUpdate>,
        at: SystemTime,
    ) -> Result<Self, String> {
        let mut draft = self.clone();
        // The ids the updates of THIS commit added, which is what an update
        // naming [`LAST_ADDED_ID`] refers to.
        let mut added_schema: Option<i32> = None;
        let mut added_spec: Option<i32> = None;
        let mut added_order: Option<i32> = None;
        for update in updates {
            match update {
                MetadataUpdate::AssignUuid(uuid) => {
                    if uuid != draft.table_uuid {
                        return Err(format!(
                            "Cannot assign UUID {uuid}: the table already has UUID {held}",
                            held = draft.table_uuid
                        ));
                    }
                }
                MetadataUpdate::UpgradeFormatVersion(version) => {
                    if version != ICEBERG_FORMAT_VERSION {
                        return Err(format!(
                            "Cannot upgrade to format version {version}: this catalog serves \
                             format version {ICEBERG_FORMAT_VERSION} tables"
                        ));
                    }
                }
                MetadataUpdate::AddSchema(schema) => {
                    bounded(draft.schemas.len(), "schemas")?;
                    let schema_id = draft.next_schema_id();
                    draft.schemas.push(IcebergSchemaRecord {
                        schema_id,
                        ..schema
                    });
                    added_schema = Some(schema_id);
                }
                MetadataUpdate::SetCurrentSchema(schema_id) => {
                    let schema_id = resolve(schema_id, added_schema, "schema")?;
                    if !draft
                        .schemas
                        .iter()
                        .any(|schema| schema.schema_id == schema_id)
                    {
                        return Err(format!("Cannot set current schema to unknown schema: {schema_id}"));
                    }
                    draft.current_schema_id = schema_id;
                }
                MetadataUpdate::AddPartitionSpec(spec) => {
                    bounded(draft.partition_specs.len(), "partition specs")?;
                    let spec_id = draft.next_spec_id();
                    // A spec added by a commit numbers its partition fields from
                    // the first id the table has not assigned yet, as the
                    // specification requires. The ids a table already holds came
                    // from a request, so the arithmetic saturates rather than
                    // wrapping, and a table that has assigned the whole range
                    // cannot number another field — which the refusal states.
                    let first_field_id = draft.last_partition_id().saturating_add(1);
                    if first_field_id == i32::MAX {
                        return Err("Cannot add a partition spec: the table has assigned every \
                                    partition field id"
                            .to_string());
                    }
                    let fields = spec
                        .fields
                        .into_iter()
                        .enumerate()
                        .map(|(position, field)| IcebergPartitionFieldRecord {
                            field_id: first_field_id
                                .saturating_add(i32::try_from(position).unwrap_or(i32::MAX)),
                            ..field
                        })
                        .collect();
                    draft
                        .partition_specs
                        .push(IcebergPartitionSpecRecord { spec_id, fields });
                    added_spec = Some(spec_id);
                }
                MetadataUpdate::SetDefaultSpec(spec_id) => {
                    let spec_id = resolve(spec_id, added_spec, "partition spec")?;
                    if !draft.partition_specs.iter().any(|spec| spec.spec_id == spec_id) {
                        return Err(format!(
                            "Cannot set default partition spec to unknown spec: {spec_id}"
                        ));
                    }
                    draft.default_spec_id = spec_id;
                }
                MetadataUpdate::AddSortOrder(order) => {
                    bounded(draft.sort_orders.len(), "sort orders")?;
                    let order_id = draft.next_sort_order_id();
                    draft
                        .sort_orders
                        .push(IcebergSortOrderRecord { order_id, ..order });
                    added_order = Some(order_id);
                }
                MetadataUpdate::SetDefaultSortOrder(order_id) => {
                    let order_id = resolve(order_id, added_order, "sort order")?;
                    if !draft.sort_orders.iter().any(|order| order.order_id == order_id) {
                        return Err(format!(
                            "Cannot set default sort order to unknown order: {order_id}"
                        ));
                    }
                    draft.default_sort_order_id = order_id;
                }
                MetadataUpdate::SetProperties(properties) => {
                    draft.properties.extend(properties);
                    if draft.properties.len() > PROPERTIES_PER_TABLE {
                        return Err(format!(
                            "Cannot set properties: a table holds at most \
                             {PROPERTIES_PER_TABLE} properties"
                        ));
                    }
                }
                MetadataUpdate::RemoveProperties(keys) => {
                    for key in keys {
                        draft.properties.remove(&key);
                    }
                }
                MetadataUpdate::SetLocation(location) => {
                    // S3 Tables owns where a table's files live: the create
                    // settles the table's warehouse location and no commit
                    // moves it, so a commit that names the location the table
                    // already has changes nothing and one that names another
                    // location is refused.
                    if location != draft.location {
                        return Err(
                            "Cannot set location: the table's location is managed by the service"
                                .to_string(),
                        );
                    }
                }
            }
        }
        draft.last_updated_ms = epoch_millis(at);
        draft.metadata_version = draft.metadata_version.saturating_add(1);
        Ok(draft)
    }

    /// Whether this record satisfies `requirement`, or the reason it does not.
    ///
    /// The reason is the sentence a commit is refused with, so it names the
    /// requirement that failed and the value the table holds instead.
    pub fn satisfies(&self, requirement: &MetadataRequirement) -> Result<(), String> {
        match requirement {
            // The table exists — this record IS the table's metadata — so a
            // commit that requires it not to exist cannot be applied.
            MetadataRequirement::AssertCreate => {
                Err("Requirement failed: table already exists".to_string())
            }
            MetadataRequirement::AssertTableUuid(uuid) => {
                if uuid == &self.table_uuid {
                    Ok(())
                } else {
                    Err(format!(
                        "Requirement failed: UUID does not match: expected {held} != {uuid}",
                        held = self.table_uuid
                    ))
                }
            }
            MetadataRequirement::AssertCurrentSchemaId(schema_id) => {
                if *schema_id == self.current_schema_id {
                    Ok(())
                } else {
                    Err(format!(
                        "Requirement failed: current schema changed: expected id {schema_id} != \
                         {held}",
                        held = self.current_schema_id
                    ))
                }
            }
            MetadataRequirement::AssertDefaultSortOrderId(order_id) => {
                if *order_id == self.default_sort_order_id {
                    Ok(())
                } else {
                    Err(format!(
                        "Requirement failed: default sort order changed: expected id {order_id} \
                         != {held}",
                        held = self.default_sort_order_id
                    ))
                }
            }
            MetadataRequirement::AssertDefaultSpecId(spec_id) => {
                if *spec_id == self.default_spec_id {
                    Ok(())
                } else {
                    Err(format!(
                        "Requirement failed: default partition spec changed: expected id \
                         {spec_id} != {held}",
                        held = self.default_spec_id
                    ))
                }
            }
            MetadataRequirement::AssertLastAssignedFieldId(field_id) => {
                if *field_id == self.last_column_id() {
                    Ok(())
                } else {
                    Err(format!(
                        "Requirement failed: last assigned field id changed: expected id \
                         {field_id} != {held}",
                        held = self.last_column_id()
                    ))
                }
            }
            MetadataRequirement::AssertLastAssignedPartitionId(partition_id) => {
                if *partition_id == self.last_partition_id() {
                    Ok(())
                } else {
                    Err(format!(
                        "Requirement failed: last assigned partition id changed: expected id \
                         {partition_id} != {held}",
                        held = self.last_partition_id()
                    ))
                }
            }
            // The table holds no snapshot (nothing has ever written a data file
            // to it), so a requirement that a reference point at one cannot be
            // satisfied, and one that requires the reference to be absent is.
            MetadataRequirement::AssertRefSnapshotId {
                reference,
                snapshot_id,
            } => match snapshot_id {
                None => Ok(()),
                Some(snapshot_id) => Err(format!(
                    "Requirement failed: branch or tag {reference} is missing, expected \
                     {snapshot_id}"
                )),
            },
        }
    }

    /// The id the next schema added to the table takes.
    fn next_schema_id(&self) -> i32 {
        self.schemas
            .iter()
            .map(|schema| schema.schema_id)
            .max()
            .map_or(INITIAL_SCHEMA_ID, |highest| highest.saturating_add(1))
    }

    /// The id the next partition spec added to the table takes.
    fn next_spec_id(&self) -> i32 {
        self.partition_specs
            .iter()
            .map(|spec| spec.spec_id)
            .max()
            .map_or(INITIAL_SPEC_ID, |highest| highest.saturating_add(1))
    }

    /// The id the next sort order added to the table takes. Order id zero is the
    /// unsorted order's, so an added order never takes it.
    fn next_sort_order_id(&self) -> i32 {
        self.sort_orders
            .iter()
            .map(|order| order.order_id)
            .max()
            .unwrap_or(UNSORTED_ORDER_ID)
            .saturating_add(1)
    }
}

/// The id an update names, with [`LAST_ADDED_ID`] resolved to the id this commit
/// added — or the reason it names nothing this commit added.
fn resolve(named: i32, added: Option<i32>, kind: &str) -> Result<i32, String> {
    if named != LAST_ADDED_ID {
        return Ok(named);
    }
    added.ok_or_else(|| format!("Cannot set last added {kind}: no {kind} has been added"))
}

/// Whether one more entry of `kind` fits in a table's metadata, or the reason a
/// commit that would add it is refused ([`METADATA_ENTRIES_PER_KIND`]).
fn bounded(held: usize, kind: &str) -> Result<(), String> {
    if held < METADATA_ENTRIES_PER_KIND {
        return Ok(());
    }
    Err(format!(
        "Cannot add to the table metadata: it already holds the maximum of \
         {METADATA_ENTRIES_PER_KIND} {kind}"
    ))
}

/// One update an Iceberg catalog commit makes to a table's metadata.
///
/// The variants are the metadata updates of the Iceberg REST catalog
/// specification this catalog applies. The updates that commit a SNAPSHOT are
/// not among them: a snapshot names data files, and this emulator serves no
/// Iceberg data plane that could have written any, so such a commit is refused
/// as unserved rather than recorded as if it had happened.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataUpdate {
    AssignUuid(String),
    UpgradeFormatVersion(i64),
    AddSchema(IcebergSchemaRecord),
    SetCurrentSchema(i32),
    AddPartitionSpec(IcebergPartitionSpecRecord),
    SetDefaultSpec(i32),
    AddSortOrder(IcebergSortOrderRecord),
    SetDefaultSortOrder(i32),
    SetProperties(BTreeMap<String, String>),
    RemoveProperties(Vec<String>),
    /// The location the commit states the table's files live under, which the
    /// service owns: a commit may restate it and may not change it.
    SetLocation(String),
}

/// One precondition an Iceberg catalog commit states about the table it commits
/// to, as the Iceberg REST catalog specification defines them.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataRequirement {
    AssertCreate,
    AssertTableUuid(String),
    AssertCurrentSchemaId(i32),
    AssertDefaultSortOrderId(i32),
    AssertDefaultSpecId(i32),
    AssertLastAssignedFieldId(i32),
    AssertLastAssignedPartitionId(i32),
    AssertRefSnapshotId {
        reference: String,
        snapshot_id: Option<i64>,
    },
}

/// The field ids a schema's columns or a partition spec's fields take: the id
/// one states, else the lowest id from `first` upwards that none of them has
/// taken.
///
/// `SchemaField.id` is optional and documented as "If not specified, S3 Tables
/// automatically assigns field IDs"
/// (`smithy_query(query="doc", shape="SchemaField", member="id")`), and
/// `IcebergPartitionField.fieldId` carries the same sentence; the ids assigned
/// run in the order the fields are declared in. `first` is where the numbering
/// starts, which for a schema's columns is 1 and for a partition spec's fields
/// is [`FIRST_PARTITION_FIELD_ID`].
pub fn assigned_ids(stated: &[Option<i32>], first: i32) -> Vec<i32> {
    let taken: ::std::collections::BTreeSet<i32> = stated.iter().flatten().copied().collect();
    let mut next = first;
    stated
        .iter()
        .map(|stated| match stated {
            Some(id) => *id,
            None => {
                // `taken` holds at most one id per field, so the search passes
                // at most that many ids before it finds a free one; the
                // arithmetic saturates so a document whose ids reach the end of
                // the range cannot wrap it.
                while taken.contains(&next) {
                    next = next.saturating_add(1);
                }
                let assigned = next;
                next = next.saturating_add(1);
                assigned
            }
        })
        .collect()
}
