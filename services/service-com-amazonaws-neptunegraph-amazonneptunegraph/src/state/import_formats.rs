// SPDX-License-Identifier: CC0-1.0

//! The bulk-load DATA FORMATS an import task reads, and the graph elements each of
//! them yields.
//!
//! One module for the four `Format` values the model declares
//! (`smithy_query(query="shape",shape="Format")` -> `CSV | NTRIPLES | OPEN_CYPHER |
//! PARQUET`), because they differ only in how a file SPELLS an element: every one of
//! them is read into the same [`LoadBuilder`], which is where the element-level rules
//! live — an element is addressed by its own id, a node carries labels and properties,
//! an edge carries a label and joins two nodes OF THIS LOAD — so no format can admit
//! data another would refuse.
//!
//! The parsing itself is done by the maintained crates for each syntax (`csv` for the
//! two delimited formats, `parquet` for the columnar one, `oxttl` for RDF N-Triples):
//! nothing here matches on source text.
//!
//! What a file holds is decided by its COLUMNS, not by its name: a row that names two
//! endpoints is an edge and any other row is a node, which is how one prefix can hold
//! a load's node files and edge files together.

use super::*;

/// The character that separates the several values one cell may carry — a node's
/// labels in both delimited formats.
const MULTI_VALUE_SEPARATOR: char = ';';

/// The most characters one value a load STORES keeps: an element id, a label, a
/// property key or a property value.
///
/// Emulator resource guard, NOT a quota AWS publishes. The ceilings the source is
/// fetched under bound the ENCODED bytes, and a columnar or compressed object can
/// expand one cell into far more than it occupies, so the text each stored value keeps
/// is clamped in its own right — the same treatment [`clamped_detail`] gives the
/// sentence a load reports.
const MAX_IMPORT_VALUE_CHARS: usize = 4096;

/// The most bytes of DECODED data one import materialises, across every object it
/// reads.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the companion of
/// [`MAX_IMPORT_SOURCE_ROWS`]: the row count bounds how many values a load decodes and
/// [`MAX_IMPORT_VALUE_CHARS`] bounds each one, and this bounds their PRODUCT, so no
/// encoding of a source inside its byte ceilings can make one import hold more than
/// this.
pub const MAX_IMPORT_DECODED_BYTES: usize = 16 * 1024 * 1024;

/// The most columns one load object may declare.
///
/// Emulator resource guard, NOT a quota AWS publishes: a row carries the reserved
/// element columns ([`LoadColumns`]) and the properties one element may hold
/// ([`MAX_PROPERTIES_PER_ELEMENT`]), so an object declaring more columns than that asks
/// for per-row work no element could keep.
pub const MAX_IMPORT_COLUMNS: usize = MAX_PROPERTIES_PER_ELEMENT + 8;

/// The most labels one node a load stores carries.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the sibling of
/// [`MAX_PROPERTIES_PER_ELEMENT`]: a label cell carries as many labels as the caller
/// separates, and rows that repeat an id merge onto one node, so without this ceiling
/// one node's label set would be bounded only by the rows the source holds.
pub const MAX_LABELS_PER_NODE: usize = 20;

/// The most rows, records or triples one import DECODES out of its source.
///
/// Emulator resource guard, NOT a quota AWS publishes, and the sibling of
/// [`MAX_NODES_PER_GRAPH`]: the byte ceilings the objects are fetched under bound the
/// encoded source, so without this one a small columnar or compressed object could ask
/// the emulator to decode an unbounded number of rows. Set well above the elements one
/// graph of this emulator holds, so a load inside the graph's own ceilings is never
/// stopped by this one.
pub const MAX_IMPORT_SOURCE_ROWS: usize = 100_000;

/// The most characters `ImportTaskDetails$errorDetails` reports.
///
/// A detail names where in the source the error was, which can include a column the
/// CALLER sized, and the task keeps it for every later read — so the sentence is
/// clamped rather than left as long as the object it came from.
const MAX_ERROR_DETAIL_CHARS: usize = 256;

/// `detail` as `ImportTaskDetails$errorDetails` reports it: no longer than
/// [`MAX_ERROR_DETAIL_CHARS`], cut on a character boundary.
fn clamped_detail(detail: &str) -> String {
    detail.chars().take(MAX_ERROR_DETAIL_CHARS).collect()
}

/// `text` as a load stores it: no longer than [`MAX_IMPORT_VALUE_CHARS`], cut on a
/// character boundary. Applied where text ENTERS the builder, so nothing a load retains
/// is as long as the object it came from.
fn clamped_value(text: &str) -> String {
    text.chars().take(MAX_IMPORT_VALUE_CHARS).collect()
}

/// How many decoded bytes one stored property value is charged: its text, or the few a
/// number or a boolean occupies. Read off the value rather than rendered, so charging it
/// allocates nothing.
fn value_bytes(value: &serde_json::Value) -> usize {
    value.as_str().map_or(16, str::len)
}

/// The detail of a row that named more properties than one element may carry. ONE
/// sentence per row, carrying how many the row named past the ceiling, so a wide row is
/// one error and not one per property.
fn refused_properties_detail(refused: usize) -> String {
    format!(
        "A row names {refused} more properties than this emulator loads onto one element: \
         {MAX_PROPERTIES_PER_ELEMENT}."
    )
}

/// The detail of a row that named more labels than one node may carry, worded as
/// [`refused_properties_detail`] and for the same reason.
fn refused_labels_detail(refused: usize) -> String {
    format!(
        "A row names {refused} more labels than this emulator loads onto one node: \
         {MAX_LABELS_PER_NODE}."
    )
}

/// One size a load object's metadata declares, as the count it is read as. A figure the
/// object declares is not a figure this emulator computed, so a negative one is read as
/// nothing and one past what a count can hold as everything.
fn declared_size(declared: i64) -> usize {
    usize::try_from(declared.max(0)).unwrap_or(usize::MAX)
}

/// The `statusReason` of a load that stopped because an object declares a nested column.
pub const NESTED_COLUMNS_REASON: &str =
    "A load object declares a nested column, and the bulk-load formats this emulator reads \
     name only flat columns.";

/// The `statusReason` of a load that stopped because an object declares more columns
/// than this emulator reads in one.
fn too_many_columns_reason() -> String {
    format!(
        "A load object declares more columns than this emulator reads in one: \
         {MAX_IMPORT_COLUMNS}."
    )
}

/// The `statusReason` of a load that stopped because its source holds more rows than
/// this emulator decodes in one import. The bound is the SERVICE's own, so the sentence
/// states it and quotes nothing out of the data.
fn too_many_rows_reason() -> String {
    format!(
        "The source holds more rows than this emulator reads in one import task: \
         {MAX_IMPORT_SOURCE_ROWS}."
    )
}

/// The `statusReason` of a load that stopped because the data it decoded is past the
/// bytes this emulator materialises in one import.
fn decoded_too_large_reason() -> String {
    format!(
        "The source's data is larger decoded than the {MAX_IMPORT_DECODED_BYTES} bytes this \
         emulator loads in one import task."
    )
}

/// The column names one delimited bulk-load format spells its elements with.
///
/// Neptune's Gremlin load format names the element columns `~id`, `~label`, `~from`
/// and `~to`, and its openCypher load format names them `:ID`, `:LABEL`, `:START_ID`,
/// `:END_ID` and `:TYPE`; every other column is a property, spelled `key:Type` in
/// both. The two sets are the only difference between the formats, so they are stated
/// here once and the row rules below are shared.
#[derive(Debug, Clone, Copy)]
pub struct LoadColumns {
    /// The column holding the element's own id.
    id: &'static str,
    /// The column holding a node's labels.
    label: &'static str,
    /// The column holding the id of the node an edge leaves.
    from: &'static str,
    /// The column holding the id of the node an edge enters.
    to: &'static str,
    /// The column holding an edge's label.
    edge_label: &'static str,
}

impl LoadColumns {
    /// The Gremlin load format's columns, which Neptune's columnar (Parquet) load
    /// format spells identically.
    pub const GREMLIN: Self = Self {
        id: "~id",
        label: "~label",
        from: "~from",
        to: "~to",
        // The Gremlin format labels a node and an edge with the SAME column.
        edge_label: "~label",
    };

    /// The openCypher load format's columns.
    pub const OPEN_CYPHER: Self = Self {
        id: ":ID",
        label: ":LABEL",
        from: ":START_ID",
        to: ":END_ID",
        edge_label: ":TYPE",
    };
}

/// One file's header, resolved to the positions the loader reads each column at.
#[derive(Debug, Default)]
struct Header {
    id: Option<usize>,
    label: Option<usize>,
    from: Option<usize>,
    to: Option<usize>,
    edge_label: Option<usize>,
    /// Every other column: where it sits, the property key it sets, and the type the
    /// header declared for it, if any.
    properties: Vec<(usize, String, Option<String>)>,
}

/// The elements read so far, and what could not be read.
///
/// Edges are STAGED rather than stored as they are read: an edge joins two nodes of
/// the same load and a load's node files and edge files are separate objects, so an
/// edge's endpoints are resolved once every object has been read.
#[derive(Debug, Default)]
pub struct LoadBuilder {
    nodes: BTreeMap<String, GraphNode>,
    staged_edges: Vec<(String, GraphEdge)>,
    /// How many rows, triples or records the load has read, read or not.
    rows: usize,
    /// How many bytes of DECODED data the load has materialised.
    decoded: usize,
    /// Which object of the source is being read, counted from one. It scopes the ids a
    /// DOCUMENT-local name converts to ([`blank_node_iri`]), because two objects of one
    /// load name their own local names independently.
    document: usize,
    /// Whether the load halts on the first error it meets ([`Self::note_error`]).
    fail_on_error: bool,
    /// How many rows, triples or files the load could not read.
    errors: usize,
    /// What the FIRST of those was — what `ImportTaskDetails$errorDetails` reports.
    first_error: Option<String>,
    /// Why the load could not be carried out at all, when it could not: the
    /// `statusReason` the task stops `FAILED` with.
    failure: Option<String>,
}

impl LoadBuilder {
    /// A builder for a load that halts on its first error, or skips the data that
    /// caused one, as `fail_on_error` says.
    ///
    /// "If set to true, the task halts when an import error is encountered. If set to
    /// false, the task skips the data that caused the error and continues if possible"
    /// (`smithy_query(query="doc",shape="StartImportTaskInput",member="failOnError")`),
    /// so the flag is held HERE, where an error is met, rather than applied once at the
    /// end: a load that halts has read no further than the row it stopped on.
    pub fn new(fail_on_error: bool) -> Self {
        Self {
            fail_on_error,
            ..Self::default()
        }
    }

    /// Begin reading the next object of the source.
    pub fn begin_document(&mut self) {
        self.document = self.document.saturating_add(1);
    }

    /// Which object of the source is being read, counted from one.
    pub fn document(&self) -> usize {
        self.document
    }

    /// Record that something in the source could not be read. The count is what
    /// `ImportTaskDetails$errorCount` reports; the first one's `detail` is what
    /// `errorDetails` reports, so a load that hit many errors still reports one
    /// sentence rather than an unbounded transcript.
    ///
    /// A load that halts on an error stops HERE, so nothing past the row that caused it
    /// is read and the counts report the run that really happened.
    pub fn note_error(&mut self, detail: String) {
        self.errors = self.errors.saturating_add(1);
        if self.first_error.is_none() {
            self.first_error = Some(clamped_detail(&detail));
        }
        if self.fail_on_error {
            self.fail(halted_on_error_reason());
        }
    }

    /// Record that the load cannot be carried out at all — the one condition that
    /// stops it where it is.
    pub fn fail(&mut self, reason: String) {
        if self.failure.is_none() {
            self.failure = Some(reason);
        }
    }

    /// Whether the load has already stopped, so no further object is read.
    pub fn has_failed(&self) -> bool {
        self.failure.is_some()
    }

    /// Admit one more row, record or triple of the source, or stop the load.
    ///
    /// The rows a load DECODES are bounded in their own right, and not left to the
    /// byte ceilings the objects are fetched under ([`MAX_IMPORT_OBJECT_BYTES`]):
    /// those bound the ENCODED source, and a columnar or compressed object can declare
    /// far more rows than its size suggests. A row that is not read — one that fails to
    /// decode, or repeats an element the load already holds — counts here too, because
    /// the work of reading it is what this bounds. `false` once the load has stopped,
    /// which is every caller's cue to read no further.
    pub fn admit_row(&mut self) -> bool {
        if self.failure.is_some() {
            return false;
        }
        if self.rows >= MAX_IMPORT_SOURCE_ROWS {
            self.fail(too_many_rows_reason());
            return false;
        }
        self.rows += 1;
        true
    }

    /// Charge `bytes` of decoded data against this load's budget, or stop it.
    ///
    /// Charged where the data is MATERIALISED, so the bytes one import holds are
    /// bounded whatever the encoding of the source they came out of
    /// ([`MAX_IMPORT_DECODED_BYTES`]). `false` once the load has stopped, which is every
    /// caller's cue to decode no further.
    pub fn charge(&mut self, bytes: usize) -> bool {
        if self.failure.is_some() {
            return false;
        }
        self.decoded = self.decoded.saturating_add(bytes);
        if self.decoded > MAX_IMPORT_DECODED_BYTES {
            self.fail(decoded_too_large_reason());
            return false;
        }
        true
    }

    /// The node `id` names, created empty when the load has not seen it yet.
    ///
    /// A node a load mentions only as an edge endpoint is a node of the load: an edge
    /// dangling off a node nobody can read would be counted by every later summary of
    /// the graph's data, which is the same rule [`GraphData::insert_edge`] enforces.
    /// `None` once the load holds as many nodes as a graph may hold, which stops it.
    fn node_mut(&mut self, id: &str) -> Option<&mut GraphNode> {
        if !self.nodes.contains_key(id) && self.nodes.len() >= MAX_NODES_PER_GRAPH {
            self.fail(format!(
                "The data holds more nodes than this emulator loads into one graph: \
                 {MAX_NODES_PER_GRAPH}."
            ));
            return None;
        }
        Some(self.nodes.entry(id.to_string()).or_default())
    }

    /// Set `properties` on the element `on`, counting an error for each property past
    /// the most one element may carry.
    fn set_properties(
        target: &mut BTreeMap<String, serde_json::Value>,
        properties: Vec<(String, serde_json::Value)>,
    ) -> usize {
        let mut refused = 0usize;
        for (key, value) in properties {
            if !target.contains_key(&key) && target.len() >= MAX_PROPERTIES_PER_ELEMENT {
                refused += 1;
                continue;
            }
            target.insert(key, value);
        }
        refused
    }

    /// Take one node row: the labels it carries and the properties it sets.
    ///
    /// Every value is clamped and charged against the load's decoded-byte budget here,
    /// because this is where a value becomes one the load RETAINS.
    fn take_node(
        &mut self,
        id: &str,
        labels: Vec<String>,
        properties: Vec<(String, serde_json::Value)>,
    ) {
        let charged = id.len()
            + labels.iter().map(String::len).sum::<usize>()
            + properties
                .iter()
                .map(|(key, value)| key.len() + value_bytes(value))
                .sum::<usize>();
        if !self.charge(charged) {
            return;
        }
        let Some(node) = self.node_mut(id) else {
            return;
        };
        let mut refused_labels = 0usize;
        for label in labels {
            // A label the node already carries is the label it carries, so a repeated
            // one is not a further label (`GraphNode::labels` is a set).
            if !node.labels.contains(&label) && node.labels.len() >= MAX_LABELS_PER_NODE {
                refused_labels += 1;
                continue;
            }
            node.labels.insert(label);
        }
        let refused = Self::set_properties(&mut node.properties, properties);
        if refused > 0 {
            self.note_error(refused_properties_detail(refused));
        }
        if refused_labels > 0 {
            self.note_error(refused_labels_detail(refused_labels));
        }
    }

    /// Take one edge row: its label, the nodes it joins, and the properties it sets.
    ///
    /// The endpoints are recorded as nodes of the load here and the edge is staged for
    /// [`Self::finish`] to resolve, so an edge file read before its node files still
    /// joins the nodes those files describe.
    fn take_edge(
        &mut self,
        id: &str,
        label: &str,
        from: &str,
        to: &str,
        properties: Vec<(String, serde_json::Value)>,
    ) {
        let charged = id.len()
            + label.len()
            + from.len()
            + to.len()
            + properties
                .iter()
                .map(|(key, value)| key.len() + value_bytes(value))
                .sum::<usize>();
        if !self.charge(charged) {
            return;
        }
        for endpoint in [from, to] {
            if self.node_mut(endpoint).is_none() {
                return;
            }
        }
        if self.staged_edges.len() >= MAX_EDGES_PER_GRAPH {
            self.fail(format!(
                "The data holds more edges than this emulator loads into one graph: \
                 {MAX_EDGES_PER_GRAPH}."
            ));
            return;
        }
        let mut edge = GraphEdge {
            label: label.to_string(),
            from_node: from.to_string(),
            to_node: to.to_string(),
            properties: BTreeMap::new(),
        };
        let refused = Self::set_properties(&mut edge.properties, properties);
        if refused > 0 {
            self.note_error(refused_properties_detail(refused));
        }
        self.staged_edges.push((id.to_string(), edge));
    }

    /// The load this builder read: the elements it holds, the counts over them, and
    /// the outcome the run ends in.
    pub fn finish(mut self) -> ImportLoad {
        let mut edges: BTreeMap<String, GraphEdge> = BTreeMap::new();
        let staged = std::mem::take(&mut self.staged_edges);
        for (id, edge) in staged {
            if !self.nodes.contains_key(&edge.from_node) || !self.nodes.contains_key(&edge.to_node)
            {
                self.note_error(
                    "An edge joins a node the load does not hold, so it was not loaded."
                        .to_string(),
                );
                continue;
            }
            edges.insert(id, edge);
        }
        let data = ImportedData {
            nodes: std::mem::take(&mut self.nodes),
            edges,
        };
        let (statement_count, dictionary_entry_count) = load_counts(&data);
        let error_count = i32::try_from(self.errors).unwrap_or(i32::MAX);
        let failure_reason = self.failure;
        ImportLoad {
            // A run that ends FAILED loads nothing: the data it had read is rolled
            // back rather than left half-loaded in the graph.
            data: if failure_reason.is_some() {
                ImportedData::default()
            } else {
                data
            },
            statement_count,
            dictionary_entry_count,
            error_count,
            error_details: self.first_error,
            source_read: true,
            failure_reason,
        }
    }
}

/// The `statusReason` of a run that stopped because `failOnError` left it no choice.
///
/// It states the rule and nothing else: the error it stopped on is what
/// `ImportTaskDetails$errorDetails` reports, and nothing out of the data is quoted here.
fn halted_on_error_reason() -> String {
    "The import task halted on the first import error because failOnError is true.".to_string()
}

/// The statements and the dictionary entries a load writes.
///
/// A STATEMENT is one fact the load writes: a label on a node, a property on a node or
/// an edge, and the edge itself. A DICTIONARY ENTRY is one distinct value the load
/// interns — an element id, a label, a property key, or a property value — which is
/// why a value repeated across rows is one entry and not many. Both are counted off
/// the elements the load actually read, so neither can report work that was not done.
fn load_counts(data: &ImportedData) -> (i64, i64) {
    let mut statements = 0usize;
    let mut dictionary: BTreeSet<String> = BTreeSet::new();
    for (id, node) in &data.nodes {
        dictionary.insert(id.clone());
        statements = statements.saturating_add(node.labels.len() + node.properties.len());
        dictionary.extend(node.labels.iter().cloned());
        for (key, value) in &node.properties {
            dictionary.insert(key.clone());
            dictionary.insert(value.to_string());
        }
    }
    for (id, edge) in &data.edges {
        dictionary.insert(id.clone());
        dictionary.insert(edge.label.clone());
        statements = statements.saturating_add(1 + edge.properties.len());
        for (key, value) in &edge.properties {
            dictionary.insert(key.clone());
            dictionary.insert(value.to_string());
        }
    }
    (element_count(statements), element_count(dictionary.len()))
}

/// Read one delimited bulk-load object (`CSV` or `OPEN_CYPHER`) into `builder`.
pub fn load_delimited(builder: &mut LoadBuilder, columns: LoadColumns, text: &str) {
    let mut reader = ::csv::ReaderBuilder::new()
        // A load file may leave the trailing property columns of a row out.
        .flexible(true)
        .trim(::csv::Trim::All)
        .from_reader(text.as_bytes());
    let header = match reader.headers() {
        Ok(header) => {
            // A row carries at most the columns one element can be stored with, and
            // every row is walked over the header's whole column list, so an object
            // declaring more columns than that is refused before a row is read: the
            // work one import performs is bounded by the ROWS it admits, not by the
            // product of its rows and a width the object chose.
            if header.len() > MAX_IMPORT_COLUMNS {
                builder.fail(too_many_columns_reason());
                return;
            }
            resolve_header(columns, header)
        }
        Err(_) => {
            builder.note_error("A load object could not be read as delimited data.".to_string());
            return;
        }
    };
    if header.id.is_none() {
        builder.note_error(format!(
            "A load object's header declares no {} column, so none of its rows names an \
             element.",
            columns.id
        ));
        return;
    }
    for (index, record) in reader.records().enumerate() {
        if !builder.admit_row() {
            return;
        }
        // The row number a caller counts from is the one in the file, after its header.
        let row = index + 2;
        match record {
            Ok(record) => take_record(builder, &header, &record, row),
            Err(_) => builder.note_error(format!("Row {row} could not be read as a data row.")),
        }
    }
}

/// Read one columnar (`PARQUET`) bulk-load object into `builder`.
///
/// Neptune's Parquet load format spells its columns exactly as the Gremlin CSV format
/// does, so the rows it yields are taken by the same row rules; only the reading of
/// the file differs.
pub fn load_parquet(builder: &mut LoadBuilder, columns: LoadColumns, object: &[u8]) {
    use ::parquet::file::reader::FileReader;

    let reader = match ::parquet::file::reader::SerializedFileReader::new(
        ::bytes::Bytes::copy_from_slice(object),
    ) {
        Ok(reader) => reader,
        Err(_) => {
            builder.note_error("A load object could not be read as Parquet.".to_string());
            return;
        }
    };
    // What an object's pages hold once DECOMPRESSED is DECLARED in its own metadata, so
    // the declaration is read as a LOWER bound on what decoding it would cost: an object
    // declaring more rows, or more uncompressed bytes in a row group or in one of its
    // column chunks, than one import may materialise is refused before a page of it is
    // read. What the rows really decode to is charged cell by cell below, which is the
    // authoritative bound; the byte ceilings the object was fetched under bound its
    // encoded size only, and a compressed column can expand far past that.
    for group in reader.metadata().row_groups() {
        if declared_size(group.num_rows()) > MAX_IMPORT_SOURCE_ROWS {
            builder.fail(too_many_rows_reason());
            return;
        }
        let chunks = group
            .columns()
            .iter()
            .map(|chunk| declared_size(chunk.uncompressed_size()))
            .fold(0usize, usize::saturating_add);
        if declared_size(group.total_byte_size()).max(chunks) > MAX_IMPORT_DECODED_BYTES {
            builder.fail(decoded_too_large_reason());
            return;
        }
    }
    // Neptune's columnar load format is FLAT — it spells the same columns the Gremlin CSV
    // format names — so an object declaring a nested column (a group, a list or a map)
    // declares no load column at all, and reading one would cost whatever that structure
    // holds.
    let flat = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .all(|column| column.max_rep_level() == 0 && column.path().parts().len() == 1);
    if !flat {
        builder.fail(NESTED_COLUMNS_REASON.to_string());
        return;
    }
    let names: Vec<String> = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .map(|column| clamped_value(column.name()))
        .collect();
    // A row carries at most the columns one element can be stored with: the reserved
    // element columns and the properties one element may hold. An object declaring more
    // asks for work no element could keep.
    if names.len() > MAX_IMPORT_COLUMNS {
        builder.fail(too_many_columns_reason());
        return;
    }
    let header = resolve_header(
        columns,
        &::csv::StringRecord::from(names.iter().map(String::as_str).collect::<Vec<_>>()),
    );
    if header.id.is_none() {
        builder.note_error(format!(
            "A load object's columns declare no {} column, so none of its rows names an \
             element.",
            columns.id
        ));
        return;
    }
    let rows = match reader.get_row_iter(None) {
        Ok(rows) => rows,
        Err(_) => {
            builder.note_error("A load object's Parquet rows could not be read.".to_string());
            return;
        }
    };
    for (index, row) in rows.enumerate() {
        if !builder.admit_row() {
            return;
        }
        // The row number a caller counts from is the one in the file.
        let row_number = index + 1;
        let row = match row {
            Ok(row) => row,
            Err(_) => {
                builder.note_error(format!(
                    "Row {row_number} could not be read as a data row."
                ));
                continue;
            }
        };
        // Every cell is charged against the load's decoded-byte budget at the size it
        // DECODED to, and only the characters a stored value keeps are copied out of it,
        // so a columnar encoding that expands a cell is paid for at its decoded size and
        // held at the clamped one.
        let mut cells: Vec<String> = Vec::with_capacity(names.len());
        for (_, field) in row.get_column_iter() {
            let (decoded, cell) = field_text(field);
            if !builder.charge(decoded) {
                return;
            }
            cells.push(cell);
        }
        let record =
            ::csv::StringRecord::from(cells.iter().map(String::as_str).collect::<Vec<_>>());
        take_record(builder, &header, &record, row_number);
    }
}

/// One Parquet cell as the bytes it DECODED to and the text the row rules read it as,
/// clamped to the characters a stored value keeps ([`clamped_value`]).
///
/// The decoded size is reported so it can be charged in full while only the clamped text
/// is copied: a byte cell is narrowed to the clamp's widest UTF-8 reading BEFORE a string
/// is built from it, and every remaining variant is a primitive whose rendering is a few
/// bytes, so no full-size copy of a large cell is ever made.
///
/// Each variant is rendered as the VALUE it holds: the crate's own `Display` quotes a
/// string and renders a null as a word, neither of which is what the cell holds.
fn field_text(field: &::parquet::record::Field) -> (usize, String) {
    use ::parquet::record::Field;
    match field {
        Field::Null => (0, String::new()),
        Field::Str(value) => (value.len(), clamped_value(value)),
        Field::Bytes(value) => {
            let data = value.data();
            // One character is at most four UTF-8 bytes, so this many bytes carry every
            // character the clamp keeps.
            let read = data.len().min(MAX_IMPORT_VALUE_CHARS.saturating_mul(4));
            let prefix = data.get(..read).unwrap_or(data);
            (data.len(), clamped_value(&String::from_utf8_lossy(prefix)))
        }
        // A NESTED cell carries no value a load reads, and nothing of it is rendered:
        // an object declaring a nested column is refused before a row of it is read
        // ([`NESTED_COLUMNS_REASON`]), so this is the reading of one that arrived
        // anyway.
        Field::Group(_) | Field::ListInternal(_) | Field::MapInternal(_) => (0, String::new()),
        // Every remaining variant is a primitive — a number, a boolean, a date or a
        // timestamp — whose rendering is a few bytes.
        other => {
            let rendered = other.to_string();
            (rendered.len(), clamped_value(&rendered))
        }
    }
}

/// Read one RDF N-Triples bulk-load object into `builder`.
///
/// The property-graph an RDF load yields is the standard reading of a triple: a triple
/// whose object is a LITERAL sets a property on the subject's node, keyed by the
/// predicate; a triple whose object is a RESOURCE joins the two nodes with an edge
/// labelled by the predicate; and `rdf:type` gives the subject a LABEL. The edge's id
/// is the triple it came from, so loading the same triple twice loads one edge — RDF
/// statements are a set.
///
/// `blank_node_handling` is the documented conversion — "Currently, only convertToIri
/// is supported, meaning blank nodes are converted to unique IRIs at load time"
/// (`smithy_query(query="doc",shape="StartImportTaskInput",member="blankNodeHandling")`)
/// — and the IRI a blank node converts to is built from its label, which the syntax
/// makes unique within the document.
pub fn load_ntriples(builder: &mut LoadBuilder, text: &str) {
    use ::oxrdf::{NamedOrBlankNode, Term};

    // A blank node's label is local to the DOCUMENT it is written in, so the IRI it
    // converts to is scoped to the object being read: two objects of one load that each
    // name `_:b0` name two different nodes.
    let document = builder.document();
    for (index, triple) in ::oxttl::NTriplesParser::new()
        .for_reader(text.as_bytes())
        .enumerate()
    {
        if !builder.admit_row() {
            return;
        }
        let line = index + 1;
        let Ok(triple) = triple else {
            builder.note_error(format!("Triple {line} could not be read as N-Triples."));
            continue;
        };
        // Every term a triple carries is clamped where it enters the builder, so a
        // document's own IRIs and literals cannot make the load retain more per element
        // than any other format's cells do.
        let subject = match &triple.subject {
            NamedOrBlankNode::NamedNode(node) => clamped_value(node.as_str()),
            NamedOrBlankNode::BlankNode(node) => {
                clamped_value(&blank_node_iri(document, node.as_str()))
            }
        };
        let predicate = clamped_value(triple.predicate.as_str());
        match &triple.object {
            Term::Literal(literal) => {
                builder.take_node(
                    &subject,
                    Vec::new(),
                    vec![(predicate, literal_value(literal))],
                );
            }
            Term::NamedNode(node) if triple.predicate == ::oxrdf::vocab::rdf::TYPE => {
                builder.take_node(&subject, vec![clamped_value(node.as_str())], Vec::new());
            }
            Term::NamedNode(node) => {
                let object = clamped_value(node.as_str());
                builder.take_edge(
                    &clamped_value(&edge_id(&subject, &predicate, &object)),
                    &predicate,
                    &subject,
                    &object,
                    Vec::new(),
                );
            }
            Term::BlankNode(node) => {
                let object = clamped_value(&blank_node_iri(document, node.as_str()));
                builder.take_edge(
                    &clamped_value(&edge_id(&subject, &predicate, &object)),
                    &predicate,
                    &subject,
                    &object,
                    Vec::new(),
                );
            }
        }
    }
}

/// The IRI a blank node labelled `label` in the `document`-th object of the source
/// converts to under `convertToIri`.
///
/// "blank nodes are converted to unique IRIs at load time": the label is unique within
/// the document the syntax wrote it in, so the IRI is that label scoped to the document,
/// which makes it unique across the whole load.
fn blank_node_iri(document: usize, label: &str) -> String {
    format!("urn:bnode:{document}:{label}")
}

/// The id of the edge a resource triple loads: the triple itself, so the same triple
/// read twice is one edge.
fn edge_id(subject: &str, predicate: &str, object: &str) -> String {
    format!("<{subject}> <{predicate}> <{object}>")
}

/// One RDF literal as the JSON value the property it sets keeps.
///
/// The XSD datatype the literal carries is what decides the type — a number stays a
/// number and a boolean stays a boolean — and everything else keeps its lexical form,
/// which is the only reading of it the datatype allows.
fn literal_value(literal: &::oxrdf::Literal) -> serde_json::Value {
    use ::oxrdf::vocab::xsd;

    let text = literal.value();
    let datatype = literal.datatype();
    if datatype == xsd::BOOLEAN {
        if let Ok(parsed) = text.parse::<bool>() {
            return serde_json::Value::Bool(parsed);
        }
    }
    if datatype == xsd::INTEGER
        || datatype == xsd::INT
        || datatype == xsd::LONG
        || datatype == xsd::SHORT
        || datatype == xsd::BYTE
    {
        if let Ok(parsed) = text.parse::<i64>() {
            return serde_json::Value::from(parsed);
        }
    }
    if datatype == xsd::DOUBLE || datatype == xsd::FLOAT || datatype == xsd::DECIMAL {
        if let Some(parsed) = text.parse::<f64>().ok().and_then(serde_json::Number::from_f64) {
            return serde_json::Value::Number(parsed);
        }
    }
    serde_json::Value::String(clamped_value(text))
}

/// Resolve one file's header to the positions each column sits at.
fn resolve_header(columns: LoadColumns, header: &::csv::StringRecord) -> Header {
    let mut resolved = Header::default();
    for (index, raw) in header.iter().enumerate() {
        let name = raw.trim();
        if name.eq_ignore_ascii_case(columns.id) {
            resolved.id = Some(index);
        } else if name.eq_ignore_ascii_case(columns.label) {
            resolved.label = Some(index);
        } else if name.eq_ignore_ascii_case(columns.from) {
            resolved.from = Some(index);
        } else if name.eq_ignore_ascii_case(columns.to) {
            resolved.to = Some(index);
        } else if name.eq_ignore_ascii_case(columns.edge_label) {
            resolved.edge_label = Some(index);
        } else {
            // Every other column is a property, spelled `key` or `key:Type`. Both
            // halves are a header cell the caller sized, so both are clamped where they
            // enter ([`clamped_value`]).
            let (key, declared) = match name.split_once(':') {
                Some((key, declared)) => (key.trim(), Some(clamped_value(declared.trim()))),
                None => (name, None),
            };
            if !key.is_empty() {
                resolved
                    .properties
                    .push((index, clamped_value(key), declared));
            }
        }
    }
    resolved
}

/// Take one row of a delimited or columnar load object.
///
/// A row that names BOTH endpoints is an edge and any other row is a node, so one
/// prefix can hold a load's node files and its edge files together; a row that names
/// only one endpoint names neither an edge nor a node, and a row with no id names no
/// element at all.
fn take_record(
    builder: &mut LoadBuilder,
    header: &Header,
    record: &::csv::StringRecord,
    row: usize,
) {
    // Every cell a row names is clamped where it enters the builder
    // ([`clamped_value`]), so what one element retains is bounded however long the
    // object's own cells are.
    let cell = |at: Option<usize>| {
        at.and_then(|index| record.get(index))
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let Some(id) = cell(header.id).map(clamped_value) else {
        builder.note_error(format!("Row {row} names no element id."));
        return;
    };
    let mut properties = Vec::new();
    for (index, key, declared) in &header.properties {
        let Some(raw) = cell(Some(*index)) else {
            continue;
        };
        match typed_value(declared.as_deref(), raw) {
            Ok(value) => properties.push((key.clone(), value)),
            Err(detail) => builder.note_error(format!("Row {row}, column {key:?}: {detail}")),
        }
    }
    let from = cell(header.from).map(clamped_value);
    let to = cell(header.to).map(clamped_value);
    match (from, to) {
        (Some(from), Some(to)) => {
            let label = cell(header.edge_label).or_else(|| cell(header.label));
            match label {
                Some(label) => {
                    builder.take_edge(&id, &clamped_value(label), &from, &to, properties);
                }
                None => builder.note_error(format!("Row {row} is an edge carrying no label.")),
            }
        }
        (None, None) => {
            // A label cell carries as many labels as the caller separates, so the cell
            // is read up to the most one node may carry and the rest is COUNTED rather
            // than held: a label past that ceiling is an error of the row, exactly as a
            // property past its own ceiling is.
            let mut labels: Vec<String> = Vec::new();
            let mut beyond = 0usize;
            if let Some(named) = cell(header.label) {
                for label in named
                    .split(MULTI_VALUE_SEPARATOR)
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                {
                    if labels.len() >= MAX_LABELS_PER_NODE {
                        beyond += 1;
                        continue;
                    }
                    labels.push(clamped_value(label));
                }
            }
            builder.take_node(&id, labels, properties);
            if beyond > 0 {
                builder.note_error(refused_labels_detail(beyond));
            }
        }
        _ => builder.note_error(format!("Row {row} names one endpoint of an edge, not both.")),
    }
}

/// One cell as the JSON value the property it sets keeps, from the type its column
/// declared.
///
/// Both delimited load formats declare a property column as `key:Type`, and the type
/// names are the ones those formats list (`Bool`/`Boolean`, `Byte`, `Short`, `Int`,
/// `Integer`, `Long`, `Float`, `Double`, `String`, `Date`/`DateTime`). A value that
/// does not read as the type its column declared is a data error, which is what
/// `failOnError` decides the fate of; a column that declares a type this loader does
/// not narrow keeps the cell's text, which every one of those types admits.
fn typed_value(declared: Option<&str>, raw: &str) -> Result<serde_json::Value, String> {
    let Some(declared) = declared.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(serde_json::Value::String(clamped_value(raw)));
    };
    let lowered = declared.to_ascii_lowercase();
    match lowered.as_str() {
        "bool" | "boolean" => raw
            .parse::<bool>()
            .map(serde_json::Value::Bool)
            .map_err(|_| format!("the value does not read as the declared {declared}.")),
        "byte" | "short" | "int" | "integer" | "long" => raw
            .parse::<i64>()
            .map(serde_json::Value::from)
            .map_err(|_| format!("the value does not read as the declared {declared}.")),
        "float" | "double" => raw
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .ok_or_else(|| format!("the value does not read as the declared {declared}.")),
        _ => Ok(serde_json::Value::String(clamped_value(raw))),
    }
}
