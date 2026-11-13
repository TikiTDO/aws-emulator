// SPDX-License-Identifier: CC0-1.0

//! The recommendation report FILE a report generation task produces: where it is written,
//! and what it holds.
//!
//! `RecommendationReportDetails$s3Bucket` is "The S3 bucket where the report file is
//! located" and `s3Keys` "The Amazon S3 key name of the report file", so a completed task
//! names an object that EXISTS: the start call generates the file here and writes it into
//! the bucket the account named for the reports this service generates
//! ([`super::RegionState::report_data_bucket`]), and the task reports that pair only once
//! the object is there — the same rule the import's status report follows
//! ([`super::import_status_report_key`]).
//!
//! The file is in the `OutputFormat` the start call asked for: `Json` is written as a JSON
//! document and `Excel` as an Excel 2007 workbook, so a caller that fetches the object gets
//! the format it named. Either way every value in it is computed from the portfolio the
//! report covers — the servers the request's `groupIdFilter` selected and the application
//! components running on them, each with the recommendation the account's assessment
//! produced for it.

use std::time::SystemTime;

use super::{rendered_timestamp, ApplicationComponentRecord, ServerRecord};

/// The key prefix every recommendation report object is written under, inside the account's
/// report data bucket.
///
/// Neither AWS nor the model publishes the key layout, so the layout is the emulator's
/// choice; what it has to be is DERIVED from the task that produced the file (see
/// [`report_key`]), so the `s3Keys` a read reports name the one object that task wrote and
/// two tasks never write over each other.
pub const RECOMMENDATION_REPORT_PREFIX: &str = "recommendation-reports";

/// The `OutputFormat` value whose report file is a JSON document.
///
/// The other value, `Excel`, is the default ([`super::DEFAULT_OUTPUT_FORMAT`]); both come
/// from `smithy_query(query="shape",shape="OutputFormat")`.
pub const OUTPUT_FORMAT_JSON: &str = "Json";

/// The key of the report object the task `task_id` writes for `output_format`.
///
/// Built from the task's minted id alone, so it is stable for the life of the task, and
/// its extension is the format the file is actually in.
pub fn report_key(task_id: &str, output_format: &str) -> String {
    format!(
        "{RECOMMENDATION_REPORT_PREFIX}/{task_id}/recommendation-report.{}",
        file_extension(output_format)
    )
}

/// The file name extension of a report file in `output_format`.
fn file_extension(output_format: &str) -> &'static str {
    if output_format == OUTPUT_FORMAT_JSON {
        "json"
    } else {
        "xlsx"
    }
}

/// The `Content-Type` the report object is stored with, so a caller fetching it is told
/// what the bytes are.
pub fn content_type(output_format: &str) -> &'static str {
    if output_format == OUTPUT_FORMAT_JSON {
        "application/json"
    } else {
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
    }
}

/// What the report covers: the servers in scope and the application components running on
/// them, each with the members the report file reports for it — and nothing else of the
/// record it came from.
///
/// Collected while the store is held ([`super::RegionState::report_scope`]), so only these
/// values are copied under the lock and the document itself is rendered (and the object
/// written) without it.
#[derive(Debug, Default)]
pub struct ReportScope {
    /// One entry per server the request's `groupIdFilter` selected.
    pub servers: Vec<ReportedServer>,
    /// One entry per application component running on one of those servers.
    pub components: Vec<ReportedComponent>,
    /// True when the portfolio in scope holds more rows than one report file covers
    /// ([`MAX_REPORT_ROWS`]), in which case nothing is collected and no file is rendered.
    pub exceeds_limit: bool,
}

/// How many rows — servers plus application components — one report file covers.
///
/// Neither AWS nor the model publishes a bound on the portfolio a report covers, so this is
/// the emulator's OWN bound on the work one start call does: it is what the scope is
/// collected, rendered and buffered under, and it is the ceiling an import's own ingestion
/// carries ([`super::import_file::MAX_IMPORT_RECORDS`]). A scope past it produces no report
/// file, and the task says so, rather than a render whose size the store alone decides.
pub const MAX_REPORT_ROWS: usize = 10_000;

/// How many bytes of report file one task writes.
///
/// The rows are bounded by [`MAX_REPORT_ROWS`] and each row holds a fixed number of cells,
/// but the cells themselves are values the account imported, so this is the ceiling on the
/// rendered document: the same one an import's own file is read under
/// ([`super::import_file::MAX_IMPORT_FILE_BYTES`]). A document past it is not written and
/// the task names no file, so nothing larger is ever buffered or uploaded.
pub const MAX_REPORT_FILE_BYTES: usize = 8 * 1024 * 1024;

impl ReportScope {
    /// How many rows this scope renders: one per resource it covers.
    pub fn rows(&self) -> usize {
        self.servers.len().saturating_add(self.components.len())
    }
}

/// One server's row of the report: the members the file reports for it.
#[derive(Debug, Clone)]
pub struct ReportedServer {
    pub id: String,
    pub name: Option<String>,
    pub server_type: Option<String>,
    pub os_type: Option<String>,
    pub os_version: Option<String>,
    pub strategy: Option<String>,
    pub target_destination: Option<String>,
    pub transformation_tool: Option<String>,
    /// The instant the server was analysed, rendered where it is read off the record.
    pub last_analyzed: Option<String>,
}

impl ReportedServer {
    /// The row the server `record` is reported as.
    pub fn of(record: &ServerRecord) -> Self {
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
            server_type: record.server_type.clone(),
            os_type: record.os_type.clone(),
            os_version: record.os_version.clone(),
            strategy: record.strategy.clone(),
            target_destination: record.target_destination.clone(),
            transformation_tool: record.transformation_tool.clone(),
            last_analyzed: record.last_analyzed.and_then(rendered_timestamp),
        }
    }
}

/// One application component's row of the report, for the same reason.
#[derive(Debug, Clone)]
pub struct ReportedComponent {
    pub id: String,
    pub name: Option<String>,
    pub app_type: Option<String>,
    pub inclusion_status: Option<String>,
    pub strategy: Option<String>,
    pub target_destination: Option<String>,
    pub transformation_tool: Option<String>,
    pub associated_server_ids: Vec<String>,
    pub last_analyzed: Option<String>,
}

impl ReportedComponent {
    /// The row the application component `record` is reported as.
    pub fn of(record: &ApplicationComponentRecord) -> Self {
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
            app_type: record.app_type.clone(),
            inclusion_status: record.inclusion_status.clone(),
            strategy: record.strategy.clone(),
            target_destination: record.target_destination.clone(),
            transformation_tool: record.transformation_tool.clone(),
            associated_server_ids: record.associated_server_ids.clone(),
            last_analyzed: record.last_analyzed.and_then(rendered_timestamp),
        }
    }
}

/// The bytes of the report file the task `task_id` writes, in `output_format`.
///
/// `Err` carries why no file could be rendered, for the caller to report rather than to
/// name an object nothing wrote — including a document past
/// [`MAX_REPORT_FILE_BYTES`], which is not handed on to be written.
pub fn report_document(
    task_id: &str,
    output_format: &str,
    generated_at: SystemTime,
    scope: &ReportScope,
) -> Result<Vec<u8>, String> {
    let rendered = if output_format == OUTPUT_FORMAT_JSON {
        json_document(task_id, output_format, generated_at, scope)
    } else {
        excel_document(task_id, output_format, generated_at, scope)?
    };
    if rendered.len() > MAX_REPORT_FILE_BYTES {
        return Err(format!(
            "the rendered report is larger than the {MAX_REPORT_FILE_BYTES} bytes one report \
             file holds"
        ));
    }
    Ok(rendered)
}

/// The report as a JSON document: the task that produced it, and one entry per resource it
/// covers.
fn json_document(
    task_id: &str,
    output_format: &str,
    generated_at: SystemTime,
    scope: &ReportScope,
) -> Vec<u8> {
    let servers: Vec<::serde_json::Value> = scope
        .servers
        .iter()
        .map(|server| {
            ::serde_json::json!({
                "serverId": server.id,
                "serverName": server.name,
                "serverType": server.server_type,
                "osType": server.os_type,
                "osVersion": server.os_version,
                "strategy": server.strategy,
                "targetDestination": server.target_destination,
                "transformationTool": server.transformation_tool,
                "lastAnalyzedTimestamp": server.last_analyzed,
            })
        })
        .collect();
    let components: Vec<::serde_json::Value> = scope
        .components
        .iter()
        .map(|component| {
            ::serde_json::json!({
                "applicationComponentId": component.id,
                "applicationComponentName": component.name,
                "appType": component.app_type,
                "inclusionStatus": component.inclusion_status,
                "strategy": component.strategy,
                "targetDestination": component.target_destination,
                "transformationTool": component.transformation_tool,
                "associatedServerIds": component.associated_server_ids,
                "lastAnalyzedTimestamp": component.last_analyzed,
            })
        })
        .collect();
    ::serde_json::json!({
        "recommendationTaskId": task_id,
        "outputFormat": output_format,
        "generatedAt": rendered_timestamp(generated_at),
        "serversInScope": servers.len(),
        "servers": servers,
        "applicationComponents": components,
    })
    .to_string()
    .into_bytes()
}

/// The column headings of the sheet naming the task the workbook was produced by.
const REPORT_COLUMNS: &[&str] = &["Property", "Value"];

/// The column headings of the server sheet of an Excel report, in the order the rows below
/// carry their values.
const SERVER_COLUMNS: &[&str] = &[
    "Server ID",
    "Server name",
    "Server type",
    "OS type",
    "OS version",
    "Strategy",
    "Target destination",
    "Transformation tool",
    "Last analyzed",
];

/// The column headings of the application component sheet, for the same reason.
const COMPONENT_COLUMNS: &[&str] = &[
    "Application component ID",
    "Application component name",
    "App type",
    "Inclusion status",
    "Strategy",
    "Target destination",
    "Transformation tool",
    "Associated server IDs",
    "Last analyzed",
];

/// The report as an Excel 2007 workbook: one sheet of the servers in scope and one of the
/// application components running on them, each row the resource's own stored values.
///
/// The workbook is produced by the `rust_xlsxwriter` crate rather than by hand, so the
/// bytes a caller fetches really are the format `Excel` names.
fn excel_document(
    task_id: &str,
    output_format: &str,
    generated_at: SystemTime,
    scope: &ReportScope,
) -> Result<Vec<u8>, String> {
    let server_rows: Vec<Vec<String>> = scope
        .servers
        .iter()
        .map(|server| {
            vec![
                server.id.clone(),
                optional(server.name.as_deref()),
                optional(server.server_type.as_deref()),
                optional(server.os_type.as_deref()),
                optional(server.os_version.as_deref()),
                optional(server.strategy.as_deref()),
                optional(server.target_destination.as_deref()),
                optional(server.transformation_tool.as_deref()),
                optional(server.last_analyzed.as_deref()),
            ]
        })
        .collect();
    let component_rows: Vec<Vec<String>> = scope
        .components
        .iter()
        .map(|component| {
            vec![
                component.id.clone(),
                optional(component.name.as_deref()),
                optional(component.app_type.as_deref()),
                optional(component.inclusion_status.as_deref()),
                optional(component.strategy.as_deref()),
                optional(component.target_destination.as_deref()),
                optional(component.transformation_tool.as_deref()),
                component.associated_server_ids.join(" "),
                optional(component.last_analyzed.as_deref()),
            ]
        })
        .collect();

    let mut workbook = ::rust_xlsxwriter::Workbook::new();
    // The task that produced the file travels with it, so a fetched workbook says which
    // report it is — the same values the JSON document opens with.
    let about = vec![
        vec!["Recommendation task ID".to_string(), task_id.to_string()],
        vec!["Output format".to_string(), output_format.to_string()],
        vec![
            "Generated at".to_string(),
            rendered_timestamp(generated_at).unwrap_or_default(),
        ],
        vec![
            "Servers in scope".to_string(),
            server_rows.len().to_string(),
        ],
    ];
    write_sheet(&mut workbook, "Report", REPORT_COLUMNS, &about)?;
    write_sheet(&mut workbook, "Servers", SERVER_COLUMNS, &server_rows)?;
    write_sheet(
        &mut workbook,
        "Application components",
        COMPONENT_COLUMNS,
        &component_rows,
    )?;
    workbook
        .save_to_buffer()
        .map_err(|error| format!("the report workbook could not be written: {error}"))
}

/// Add one sheet named `name` to `workbook`: `columns` as its heading row, then one row per
/// entry of `rows`.
fn write_sheet(
    workbook: &mut ::rust_xlsxwriter::Workbook,
    name: &str,
    columns: &[&str],
    rows: &[Vec<String>],
) -> Result<(), String> {
    let sheet = workbook.add_worksheet();
    sheet
        .set_name(name)
        .map_err(|error| format!("the report sheet {name} could not be named: {error}"))?;
    for (column, heading) in columns.iter().enumerate() {
        write_cell(sheet, 0, column, heading)?;
    }
    for (index, values) in rows.iter().enumerate() {
        // Row 0 is the heading, so the first resource is row 1. How many rows there are is
        // bounded by [`MAX_REPORT_ROWS`] before the scope is collected, so this walk — and
        // the workbook it builds — is bounded by that constant and not by the portfolio the
        // account imported; the conversion below only keeps the index honest.
        let row = u32::try_from(index + 1)
            .map_err(|_| format!("the report sheet {name} holds more rows than it can carry"))?;
        for (column, value) in values.iter().enumerate() {
            write_cell(sheet, row, column, value)?;
        }
    }
    Ok(())
}

/// Write `value` into one cell of `sheet`.
fn write_cell(
    sheet: &mut ::rust_xlsxwriter::Worksheet,
    row: u32,
    column: usize,
    value: &str,
) -> Result<(), String> {
    let column = u16::try_from(column)
        .map_err(|_| "the report sheet holds more columns than it can carry".to_string())?;
    sheet
        .write_string(row, column, value)
        .map(|_| ())
        .map_err(|error| format!("a report cell could not be written: {error}"))
}

/// A member the record holds nothing for, as the empty cell of a spreadsheet that has no
/// absent value to carry (the JSON document reports it as `null`).
fn optional(value: Option<&str>) -> String {
    value.unwrap_or_default().to_string()
}
