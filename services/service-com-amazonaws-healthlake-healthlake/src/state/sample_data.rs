// SPDX-License-Identifier: CC0-1.0

//! Inferring a profile's initial DRAFT content from the SAMPLE DATA file a create's
//! `Source` names in Amazon S3.
//!
//! `CreateDataTransformationProfile` documents the variant as "Creates the profile
//! from a sample data file stored in Amazon S3. Valid only when the source format
//! is Comma-separated values (CSV)", so what the new profile starts life with is a
//! mapping read OUT OF that file. This module reads it and infers that mapping.
//!
//! # Where the data comes from
//!
//! This emulator holds an Amazon S3 of its own, and it is reached the way any
//! emulated service reaches another: the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), through the one shared
//! object read ([`service_common::s3_object_read`]), which routes IN PROCESS — no
//! socket, no outbound request, no host resolution, and no address a caller
//! supplies is ever dialled. An object a caller staged with `PutObject` is
//! therefore the object a create reads, and an address the store holds nothing at
//! is a refusal rather than a mapping.
//!
//! The read is BOUNDED before any of it is parsed ([`MAX_SAMPLE_DATA_BYTES`]), and
//! the inference samples at most [`MAX_SAMPLE_DATA_ROWS`] data rows, so neither the
//! bytes taken into this process nor the work done on them is a product of
//! caller-chosen factors.
//!
//! # What is inferred, and from what
//!
//! The CSV is parsed by the `csv` crate, never by hand, and the mapping is computed
//! from what the file actually holds:
//!
//! * each column of the header row becomes one mapping file, naming the column, its
//!   position and the data type inferred from the values the sampled rows carry in
//!   it ([`SampleDataType`]);
//! * a column named for a FHIR R4 `Patient` element — optionally prefixed with the
//!   resource name — maps onto that element ([`PATIENT_ELEMENTS`], the element list
//!   the FHIR R4 `Patient` resource publishes); and
//! * every other column maps onto a FHIR R4 `Observation` keyed by the column's own
//!   name, whose `value[x]` choice follows the inferred data type — the choice
//!   types the FHIR R4 `Observation` resource publishes.
//!
//! Nothing here is keyed to a file name, an extension or any particular column: a
//! file holding no header row, or more columns than a profile mapping holds
//! entries, is refused rather than answered with content nothing read.

use std::collections::BTreeMap;

use service_common::s3_object_read::{read_object, ObjectRead};

use super::{
    content_file_slug, normalized_content_name, profile_target_format, split_s3_uri,
    MAX_PROFILE_MAPPING_ENTRIES, PROFILE_CONTENT_MANIFEST_FILE,
};

/// The most BYTES of a sample data file this emulator reads to infer a mapping.
///
/// A ceiling of this emulator's, not a claim about any AWS quota: it bounds the
/// bytes one create takes into this process, and it is the same order as the
/// content one profile version retains ([`super::MAX_PROFILE_MAPPING_BYTES`]) —
/// a header row and a sample of data rows is all the inference reads.
pub const MAX_SAMPLE_DATA_BYTES: usize = 1 << 20;

/// The most DATA ROWS the inference samples to decide a column's data type.
///
/// Bounded for the reason [`MAX_SAMPLE_DATA_BYTES`] is: the work one create does is
/// then bounded by the columns of one header row times this, rather than by how
/// many rows a caller staged.
pub const MAX_SAMPLE_DATA_ROWS: usize = 100;

/// The source format a sample data file is read as, as the modelled `SourceFormat`
/// enum spells it — the format the variant is documented as valid for ("Valid only
/// when the source format is Comma-separated values (CSV)").
pub const SAMPLE_DATA_SOURCE_FORMAT: &str = "CSV";

/// The FHIR R4 resource a column that is not a `Patient` element maps onto.
///
/// From the FHIR R4 resource list: an `Observation` is what FHIR R4 carries a
/// measured or asserted value keyed by its own code with.
const OBSERVATION_RESOURCE: &str = "Observation";

/// The FHIR R4 resource a column named for one of its elements maps onto.
const PATIENT_RESOURCE: &str = "Patient";

/// The elements the FHIR R4 `Patient` resource publishes, in the order that
/// resource lists them.
///
/// A sample column whose name is one of these — on its letters and digits alone,
/// and with a leading mention of the resource dropped — carries that element, so the
/// inferred mapping targets it rather than an `Observation`. Read only through
/// [`patient_element`].
const PATIENT_ELEMENTS: &[&str] = &[
    "id",
    "identifier",
    "active",
    "name",
    "telecom",
    "gender",
    "birthDate",
    "deceasedBoolean",
    "deceasedDateTime",
    "address",
    "maritalStatus",
    "multipleBirthBoolean",
    "multipleBirthInteger",
    "photo",
    "contact",
    "communication",
    "generalPractitioner",
    "managingOrganization",
    "link",
];

/// The directory the per-column mapping files live under.
const COLUMN_DIRECTORY: &str = "columns";

/// The name a mapping file takes for a column whose header is blank.
const UNNAMED_COLUMN_SLUG: &str = "column";

/// The sentence a create is refused with when its `Source` names no FILE: the
/// sample data is one object, so a URI naming a bucket alone names nothing to read.
pub const SAMPLE_DATA_NOT_A_FILE: &str =
    "Source.SampleData.S3Uri must name a sample data file in an Amazon S3 bucket";

/// The sentence a create is refused with when the object store answered that it
/// holds nothing at the address the request named.
pub const SAMPLE_DATA_ABSENT: &str =
    "Amazon S3 holds no sample data file at the location Source.SampleData.S3Uri names";

/// The sentence a create is refused with when the object store answered that the
/// read is not permitted.
pub const SAMPLE_DATA_DENIED: &str =
    "HealthLake is not permitted to read the sample data file Source.SampleData.S3Uri names";

/// The sentence a create is refused with when nothing answered the read at all.
pub const SAMPLE_DATA_UNANSWERED: &str =
    "The sample data file Source.SampleData.S3Uri names could not be read from Amazon S3";

/// The sentence a create is refused with when the file arrived but holds no CSV
/// header row to read columns from.
pub const SAMPLE_DATA_NO_HEADER: &str =
    "The sample data file holds no header row, so no CSV columns could be read from it";

/// The sentence a create is refused with when the file is not text at all.
pub const SAMPLE_DATA_NOT_TEXT: &str =
    "The sample data file is not UTF-8 text, so no CSV columns could be read from it";

/// The sentence a create is refused with when the file could not be read as CSV.
pub const SAMPLE_DATA_NOT_CSV: &str =
    "The sample data file could not be read as comma-separated values";

/// The data type a column's sampled values establish.
///
/// The join of the types of the values seen in one column: a column of whole
/// numbers is [`Self::Integer`], one mixing whole and fractional numbers is
/// [`Self::Decimal`], and a column whose values do not agree on a narrower type is
/// [`Self::String`] — the type that carries any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleDataType {
    Integer,
    Decimal,
    Boolean,
    Date,
    DateTime,
    String,
}

impl SampleDataType {
    /// How the inferred mapping names this type.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Decimal => "decimal",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::DateTime => "dateTime",
            Self::String => "string",
        }
    }

    /// The type one value carries, or `None` for a value that is empty — an empty
    /// field says nothing about the column's type.
    fn of_value(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("false") {
            return Some(Self::Boolean);
        }
        if value.parse::<i64>().is_ok() {
            return Some(Self::Integer);
        }
        if value.parse::<f64>().is_ok_and(f64::is_finite) {
            return Some(Self::Decimal);
        }
        if is_date(value) {
            return Some(Self::Date);
        }
        if is_date_time(value) {
            return Some(Self::DateTime);
        }
        Some(Self::String)
    }

    /// The type that carries both `self` and `other`: the wider of a whole and a
    /// fractional number, and of a date and a date-time, and otherwise a string.
    fn joined(self, other: Self) -> Self {
        if self == other {
            return self;
        }
        match (self, other) {
            (Self::Integer, Self::Decimal) | (Self::Decimal, Self::Integer) => Self::Decimal,
            (Self::Date, Self::DateTime) | (Self::DateTime, Self::Date) => Self::DateTime,
            _ => Self::String,
        }
    }
}

/// The DRAFT content the sample data file at `s3_uri` resolves to, or the sentence
/// the create is refused with.
///
/// Reads the object out of this emulator's own Amazon S3 and infers the mapping
/// from what it holds — see the module docs. Nothing is stored here: the content is
/// answered to the create, which persists it as the new profile's DRAFT.
pub async fn infer_sample_data_mapping(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    s3_uri: &str,
) -> Result<BTreeMap<String, String>, String> {
    let Some((bucket, key)) = split_s3_uri(s3_uri).filter(|(_, key)| !key.is_empty()) else {
        return Err(SAMPLE_DATA_NOT_A_FILE.to_string());
    };
    let bytes = match read_object(
        ctx.callback(),
        region,
        &bucket,
        &key,
        None,
        MAX_SAMPLE_DATA_BYTES,
    )
    .await
    {
        ObjectRead::Served(bytes) => bytes,
        ObjectRead::Absent => return Err(SAMPLE_DATA_ABSENT.to_string()),
        ObjectRead::Denied => return Err(SAMPLE_DATA_DENIED.to_string()),
        ObjectRead::Unreadable => return Err(sample_data_unreadable_message()),
        ObjectRead::Unanswered => return Err(SAMPLE_DATA_UNANSWERED.to_string()),
    };
    let text = ::std::str::from_utf8(&bytes).map_err(|_| SAMPLE_DATA_NOT_TEXT.to_string())?;
    infer_mapping_from_csv(text)
}

/// The DRAFT content the CSV `text` resolves to, or the sentence the create is
/// refused with.
///
/// Separated from the read so the inference is driven by BYTES rather than by a
/// transport: the rule it implements is about what the file holds.
fn infer_mapping_from_csv(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(true)
        // A sample file whose rows do not all carry every column is still a sample
        // of its columns, so a ragged row is read rather than refused.
        .flexible(true)
        .from_reader(text.as_bytes());
    let columns: Vec<String> = reader
        .headers()
        .map_err(|_| SAMPLE_DATA_NOT_CSV.to_string())?
        .iter()
        .map(str::to_string)
        .collect();
    if columns.is_empty() {
        return Err(SAMPLE_DATA_NO_HEADER.to_string());
    }
    // One file per column plus the manifest: a header row with more columns than
    // the modelled `ProfileMapping` holds entries could not be read back, so it is
    // refused rather than stored in part.
    if columns.len() + 1 > MAX_PROFILE_MAPPING_ENTRIES {
        return Err(sample_data_too_many_columns_message());
    }

    // The data type of each column, joined over the values the sampled rows carry
    // in it. A row the reader could not read ends the sampling: what was read is
    // what the types are inferred from.
    let mut types: Vec<Option<SampleDataType>> = vec![None; columns.len()];
    let mut rows_sampled = 0usize;
    for record in reader.records().take(MAX_SAMPLE_DATA_ROWS) {
        let Ok(record) = record else { break };
        rows_sampled += 1;
        for (held, value) in types.iter_mut().zip(record.iter()) {
            // Paired with the header's columns, so a ragged row carrying more
            // fields than the header has columns contributes nothing past them and
            // no field is read by index.
            if let Some(seen) = SampleDataType::of_value(value) {
                *held = Some(held.map_or(seen, |earlier| earlier.joined(seen)));
            }
        }
    }

    let mut mapping: BTreeMap<String, String> = BTreeMap::new();
    for (index, (column, data_type)) in columns.iter().zip(types.iter()).enumerate() {
        // A column no sampled row carried a value in says nothing narrower than a
        // string, which is the type that carries any value.
        let data_type = data_type.unwrap_or(SampleDataType::String);
        mapping.insert(
            column_file(index, column),
            column_mapping(index, column, data_type),
        );
    }
    mapping.insert(
        PROFILE_CONTENT_MANIFEST_FILE.to_string(),
        ::serde_json::json!({
            "sourceFormat": SAMPLE_DATA_SOURCE_FORMAT,
            "targetFormat": profile_target_format(),
            "columnCount": columns.len(),
            "rowsSampled": rows_sampled,
            "columns": columns,
        })
        .to_string(),
    );
    Ok(mapping)
}

/// The mapping file one column's rule is carried in: its position in the header
/// row, so two columns of the same name are two files, and its name.
fn column_file(index: usize, column: &str) -> String {
    let slug = content_file_slug(column);
    let slug = if slug.is_empty() {
        UNNAMED_COLUMN_SLUG.to_string()
    } else {
        slug
    };
    format!("{COLUMN_DIRECTORY}/{:03}-{slug}.json", index + 1)
}

/// The rule inferred for one column: what it is in the source, and what it becomes
/// in FHIR R4.
fn column_mapping(index: usize, column: &str, data_type: SampleDataType) -> String {
    // A column named for a `Patient` element maps onto that element; any other
    // column maps onto an `Observation`, which needs a code — and the column's own
    // name is what the file says its values are of.
    let (resource, element, code_text) = match patient_element(column) {
        Some(element) => (
            PATIENT_RESOURCE,
            format!("{PATIENT_RESOURCE}.{element}"),
            None,
        ),
        None => (
            OBSERVATION_RESOURCE,
            observation_value_element(data_type).to_string(),
            Some(column),
        ),
    };
    let mut rule = ::serde_json::json!({
        "sourceFormat": SAMPLE_DATA_SOURCE_FORMAT,
        "sourceColumn": column,
        // 1-based, as a CSV header row's columns are counted.
        "sourceColumnNumber": index + 1,
        "sourceDataType": data_type.as_wire(),
        "targetFormat": profile_target_format(),
        "targetResourceType": resource,
        "targetElement": element,
    });
    if let (Some(code_text), Some(rule)) = (code_text, rule.as_object_mut()) {
        rule.insert(
            "targetObservationCodeText".to_string(),
            ::serde_json::Value::from(code_text),
        );
    }
    rule.to_string()
}

/// The FHIR R4 `Patient` element the column `column` carries, or `None` for a
/// column that names none.
///
/// Matched on letters and digits alone, with a leading mention of the resource
/// dropped, so a column named for an element carries it however the file spells the
/// name's word separators and case.
fn patient_element(column: &str) -> Option<&'static str> {
    let normalized = normalized_content_name(column);
    let addressed = normalized
        .strip_prefix(&normalized_content_name(PATIENT_RESOURCE))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(normalized.as_str());
    PATIENT_ELEMENTS
        .iter()
        .copied()
        .find(|element| normalized_content_name(element) == addressed)
}

/// The FHIR R4 `Observation.value[x]` choice a column of `data_type` carries, from
/// the choice types that resource publishes.
fn observation_value_element(data_type: SampleDataType) -> &'static str {
    match data_type {
        SampleDataType::Integer | SampleDataType::Decimal => "Observation.valueQuantity",
        SampleDataType::Boolean => "Observation.valueBoolean",
        SampleDataType::Date | SampleDataType::DateTime => "Observation.valueDateTime",
        SampleDataType::String => "Observation.valueString",
    }
}

/// The sentence a create is refused with when the file could not be read through
/// the object store, the bound it would have had to be inside included.
pub fn sample_data_unreadable_message() -> String {
    format!(
        "The sample data file could not be read, or is larger than the {MAX_SAMPLE_DATA_BYTES} \
         bytes this emulator reads to infer a profile mapping"
    )
}

/// The sentence a create is refused with when the file's header row holds more
/// columns than one profile mapping holds entries — content that could be inferred
/// but never read back.
pub fn sample_data_too_many_columns_message() -> String {
    format!(
        "The sample data file holds more columns than one profile mapping holds entries: \
         {MAX_PROFILE_MAPPING_ENTRIES}"
    )
}

/// Whether `value` is a FHIR R4 `date`: `YYYY-MM-DD`, with a month and a day in
/// the ranges the calendar admits.
///
/// Checked against the calendar rather than by digit count alone, so a value whose
/// month or day could not exist is a string rather than a date; the day's bound is
/// the longest month's, since a sample column's type does not turn on which month
/// one of its values names. A value carrying a time of day is read by
/// [`is_date_time`], which parses it with `aws_smithy_types` rather than by hand.
fn is_date(value: &str) -> bool {
    let mut parts = value.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    year.len() == 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && month.len() == 2
        && day.len() == 2
        && month.parse::<u32>().is_ok_and(|month| (1..=12).contains(&month))
        && day.parse::<u32>().is_ok_and(|day| (1..=31).contains(&day))
}

/// Whether `value` is a FHIR R4 `dateTime`: an instant, read by the framework's own
/// RFC 3339 reader ([`aws_smithy_types::date_time::Format::DateTime`], the format
/// this service reports every modelled timestamp in) rather than by hand, so a
/// value this says carries a time of day is one that parses as one.
fn is_date_time(value: &str) -> bool {
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a column's data type is the join of the types of the values the
    /// sampled rows carry in it, and an empty field says nothing about it.
    #[test]
    fn a_column_takes_the_type_its_values_establish() {
        let rows: &[(&str, &[&str], SampleDataType)] = &[
            ("whole numbers", &["1", "2", "30"], SampleDataType::Integer),
            (
                "whole and fractional numbers",
                &["1", "2.5"],
                SampleDataType::Decimal,
            ),
            ("booleans", &["true", "FALSE"], SampleDataType::Boolean),
            ("dates", &["2026-01-02", "1999-12-31"], SampleDataType::Date),
            (
                "dates and date-times",
                &["2026-01-02", "2026-01-02T03:04:05Z"],
                SampleDataType::DateTime,
            ),
            (
                "numbers and words",
                &["1", "unknown"],
                SampleDataType::String,
            ),
            (
                "a month and a day the calendar does not admit",
                &["2026-13-45"],
                SampleDataType::String,
            ),
            (
                "an instant with no time of day after its T",
                &["2026-01-02T"],
                SampleDataType::String,
            ),
            (
                "empty fields beside numbers",
                &["", "7", ""],
                SampleDataType::Integer,
            ),
            ("empty fields alone", &["", ""], SampleDataType::String),
        ];
        for (case, values, expected) in rows {
            let joined = values
                .iter()
                .filter_map(|value| SampleDataType::of_value(value))
                .reduce(SampleDataType::joined)
                .unwrap_or(SampleDataType::String);
            assert_eq!(
                joined, *expected,
                "case {case:?}: expected {expected:?}, got {joined:?}"
            );
        }
    }

    /// RULE: a column named for a FHIR R4 `Patient` element carries that element,
    /// however the file spells the name; any other column does not.
    #[test]
    fn a_column_named_for_a_patient_element_carries_it() {
        let rows: &[(&str, &str, Option<&str>)] = &[
            ("the element name", "gender", Some("gender")),
            ("snake case", "birth_date", Some("birthDate")),
            ("prefixed with the resource", "patient_id", Some("id")),
            ("spaced and capitalised", "Marital Status", Some("maritalStatus")),
            ("a column of its own", "systolic_bp", None),
            ("the resource alone", "patient", None),
            ("a blank header", "", None),
        ];
        for (case, column, expected) in rows {
            let element = patient_element(column);
            assert_eq!(
                element, *expected,
                "case {case:?}: expected {expected:?}, got {element:?}"
            );
        }
    }
}
