// SPDX-License-Identifier: CC0-1.0

//! The ANSWER one compliance-inquiry query carries: the `response` this service produced
//! for it and the `citations` that support that response.
//!
//! A compliance inquiry asks a question about compliance, and the only compliance material
//! this service holds is the published report catalog it serves
//! ([`super::ComAmazonawsArtifactArtifactState::reports`]). So the answer to a query is
//! RETRIEVED from that catalog: the published documents whose text carries the query's
//! terms are the sources, the passage of each one the terms were found in is what that
//! source contributes, and the response is those passages, each under the source it was
//! read out of. The text a caller reads back is therefore text this service really holds,
//! quoted from the report it came from, and the citation beside it locates that report's own
//! read — not prose a model wrote, which nothing in this emulator could produce.
//!
//! The retrieval itself is the workspace's ONE free-text search over stored documents
//! ([`::service_common::document_text_search`]), the same one the other services that
//! answer a caller's query out of their own records use: a document matches when its text
//! carries a term of the query, it scores the share of the query's distinct terms it
//! carries, and the window around the match is what gets reported. Nothing here ranks or
//! excerpts in its own way.
//!
//! A query the catalog has nothing bearing on gets NO answer — the `response` and
//! `citations` members stay absent rather than carrying a passage no term was found in.

use super::*;

use ::service_common::document_text_search::{
    match_document, query_terms, DocumentFields, WindowBounds,
};

use crate::ops::types::Citation;

/// The path of the modelled read a citation locates its source by: `GetReportMetadata`'s
/// own URI (`smithy_query(query="operation",operation="GetReportMetadata")` — GET
/// `/v1/report/getMetadata`, with `reportId` and `reportVersion` as `@httpQuery` members).
const REPORT_METADATA_PATH: &str = "/v1/report/getMetadata";

/// The most sources one answer cites.
///
/// A bound this emulator owns — AWS publishes none — so one answer carries a readable
/// handful of passages rather than a quotation of every catalog entry that happens to share
/// a word with the query. The sources kept are the best-scoring ones, so raising it would
/// only append weaker matches.
pub const ANSWER_CITATION_CAPACITY: usize = 3;

/// The most distinct terms one query is retrieved on.
///
/// A bound this emulator owns: the scan costs the PRODUCT of the query's terms and the
/// catalog's own text, so it is the terms that have to be bounded — the text member
/// carrying a query is `@length(max: 2048)` and an inquiry submits up to
/// [`QUERIES_PER_INQUIRY_CEILING`] of them, so one request's work is bounded by this
/// number times those two. A query naming more terms than this is answered on the ones it
/// names first; no compliance question written by hand comes close.
pub const ANSWER_QUERY_TERMS: usize = 64;

/// The window of a cited document's text one citation reports: its size in characters, how
/// much of the text it opens ahead of the first match, and the most matches it is cut
/// around.
///
/// Also an emulator-owned bound, sized so that [`ANSWER_CITATION_CAPACITY`] passages and
/// their source labels together stay inside what the member reporting the response can
/// carry (`LongStringAttribute`, `@length(min: 1, max: 2048)`).
const CITATION_WINDOW: WindowBounds = WindowBounds {
    max_chars: 480,
    lead_chars: 64,
    max_spans: 16,
};

/// `text` as a `LongStringAttribute` member of this service can carry it, or `None` when
/// nothing reportable remains of it.
///
/// Every such member — `QuerySummary$query`, `QuerySummary$response`,
/// `Citation$sourceContent` — is `@length(min: 1, max: 2048)` with `@pattern ^[^<>]*$`, so
/// the characters that pattern excludes are dropped and the text is clipped to the modelled
/// maximum. ONE rule for the shape, so no member of it is ever reported outside what the
/// model declares.
pub fn reportable_long_text(text: &str) -> Option<String> {
    let text: String = text
        .chars()
        .filter(|character| *character != '<' && *character != '>')
        .take(QUERY_TEXT_LIMIT)
        .collect();
    (!text.is_empty()).then_some(text)
}

/// One source an answer cites: the passage, the document it was read out of, and which
/// stored report that is.
///
/// The `Citation$sourceLink` a reader sees is NOT held here: it is built per projection
/// ([`Self::source_link`]) against the endpoint the reading request reached this emulator
/// on, exactly as `GetReport` builds the URL it announces, so the link resolves for whoever
/// was handed it rather than for whoever submitted the query.
#[derive(Debug, Clone)]
pub struct QueryCitation {
    /// "Label of the compliance source" — the name of the published report the passage
    /// comes from.
    pub source_label: String,
    /// "Content text from the compliance source" — the window of that report's published
    /// text the query's terms were found in.
    pub source_content: String,
    /// The report the passage was read out of, and the version of it the catalog publishes:
    /// what [`Self::source_link`] locates, so the link always names a source this service
    /// really holds.
    pub report_id: String,
    pub report_version: i64,
}

impl QueryCitation {
    /// "Link to the compliance source": where the cited report is READ in this emulator —
    /// `GetReportMetadata` for exactly that report and version — on the endpoint the
    /// reading caller reached it on ([`super::document_url_host`]).
    ///
    /// The source is located rather than handed over: a citation says which published report
    /// a passage came from, and it is not the receipt that authorises a download of that
    /// report's document. A caller follows it to the cited report's own metadata, and
    /// reaches the DOCUMENT the way every other caller does — by accepting the term
    /// governing it (`GetTermForReport`) and presenting the grant to `GetReport` — so no
    /// link here stands in for an acceptance nobody made, and none of them points at a
    /// location this emulator would refuse to serve.
    pub fn source_link(&self, host: &str) -> String {
        format!(
            "https://{host}{REPORT_METADATA_PATH}?reportId={report_id}\
             &reportVersion={version}",
            report_id = self.report_id,
            version = self.report_version,
        )
    }

    /// This source as the modelled `Citation` a reader of the query reports.
    pub fn to_citation(&self, host: &str) -> Citation {
        Citation {
            source_label: Some(self.source_label.clone()),
            source_content: Some(self.source_content.clone()),
            source_link: Some(self.source_link(host)),
        }
    }
}

/// What the service produced for one query: the response, and the sources it was drawn
/// from.
///
/// INVARIANT: `citations` is never empty — the response IS the cited passages, so an answer
/// with nothing to cite is no answer at all and is never built
/// ([`answer_from_catalog`] answers `None` for it).
#[derive(Debug, Clone)]
pub struct QueryAnswer {
    /// "Generated response to the query" — the cited passages, each under the source it was
    /// read out of.
    pub response: String,
    /// "Supporting citations for the response", best-matching source first.
    pub citations: Vec<QueryCitation>,
}

impl QueryAnswer {
    /// The modelled `citations` list a reader of the query reports, against the endpoint
    /// that reader reached ([`QueryCitation::source_link`]).
    pub fn reported_citations(&self, host: &str) -> Vec<Citation> {
        self.citations
            .iter()
            .map(|citation| citation.to_citation(host))
            .collect()
    }
}

/// The catalog as the retrieval reads it: one searchable entry per published report.
///
/// Copied out of the store so the scan itself runs WITHOUT holding the state lock — the
/// catalog is service-owned and never written, so a copy of it cannot go stale, and one
/// caller's retrieval cannot hold up another's call while it runs.
#[derive(Debug, Clone)]
pub struct AnswerCorpus {
    entries: Vec<CorpusEntry>,
}

/// One searchable catalog entry: how the report is named and classified, the published text
/// a passage is quoted from, and which published report that text belongs to.
#[derive(Debug, Clone)]
struct CorpusEntry {
    label: String,
    /// How the catalog CLASSIFIES the report — its series and its category, which is what a
    /// reader names a kind of compliance document by.
    classification: String,
    text: String,
    report_id: String,
    report_version: i64,
}

impl AnswerCorpus {
    /// The searchable form of `catalog`: each report as the catalog itself reports it — its
    /// name, its series and category, and the published description of the version
    /// `ListReports` enumerates.
    ///
    /// A report holding no published version has nothing to quote and no read to link to, so
    /// it contributes no entry, exactly as it is passed over by the enumeration.
    pub fn from_catalog(catalog: &BTreeMap<String, Report>) -> Self {
        Self {
            entries: catalog
                .values()
                .filter_map(|report| {
                    let version = report.resolve(None)?;
                    Some(CorpusEntry {
                        label: report.name.clone(),
                        classification: format!("{} {}", report.series, report.category),
                        text: version.description.clone(),
                        report_id: report.id.clone(),
                        report_version: version.version,
                    })
                })
                .collect(),
        }
    }
}

/// The answer this service produces for `query` out of the published report `corpus`, or
/// `None` when the catalog holds nothing bearing on it.
///
/// The one place a compliance answer of this service is produced, so a query's response and
/// its citations can never disagree: the citations ARE what the response is made of.
///
/// The entries that matched are ranked by the share of the query's terms they carry, with
/// the report id breaking a tie so two equally good sources are cited in one stable order
/// rather than in the store's.
///
/// DETERMINISTIC in the catalog and the query text: the catalog is service-owned and never
/// written, so the answer to one query is the same whenever it is produced, which is what
/// lets it be produced once, when the query is submitted, and reported when the clock has
/// made that query's processing due ([`super::InquiryQuery::reported_answer`]).
pub fn answer_from_catalog(query: &str, corpus: &AnswerCorpus) -> Option<QueryAnswer> {
    // Bounded BEFORE any work proportional to it runs: each term is read against every
    // searched character of every catalog entry, so the scan costs their product.
    let mut terms = query_terms(query);
    terms.truncate(ANSWER_QUERY_TERMS);
    let mut ranked: Vec<(f64, QueryCitation)> = Vec::new();
    for entry in &corpus.entries {
        let Some(hit) = match_document(
            DocumentFields {
                name: &entry.classification,
                title: &entry.label,
                body: Some(&entry.text),
            },
            &terms,
            CITATION_WINDOW,
        ) else {
            continue;
        };
        // A source whose matched window carries nothing the member can report is not cited:
        // there is no passage to quote, and none is invented for it.
        let Some(source_content) = reportable_long_text(&hit.excerpt.text) else {
            continue;
        };
        ranked.push((
            hit.score,
            QueryCitation {
                source_label: entry.label.clone(),
                source_content,
                report_id: entry.report_id.clone(),
                report_version: entry.report_version,
            },
        ));
    }
    ranked.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.report_id.cmp(&right.1.report_id))
    });
    let mut citations: Vec<QueryCitation> = ranked
        .into_iter()
        .take(ANSWER_CITATION_CAPACITY)
        .map(|(_, citation)| citation)
        .collect();
    // Composed first: a source whose passage does not fit the response is dropped from the
    // citation list too, so the two always agree ([`compose_response`]).
    let response = compose_response(&mut citations)?;
    Some(QueryAnswer {
        response,
        citations,
    })
}

/// The response the cited `citations` make up: each passage under the source it was read
/// out of, best-matching source first, inside the bounds of the member that reports it
/// ([`reportable_long_text`]).
///
/// Composed one source at a time, and a source whose passage does not FIT in what remains
/// is dropped from the answer altogether — so the response and the citation list cannot
/// disagree about what was quoted, whatever the window and the capacity above are set to.
///
/// `None` when no source fits at all, which is what makes an answer with nothing to cite no
/// answer: the response is the quoted sources, so there is no text for it without them.
fn compose_response(citations: &mut Vec<QueryCitation>) -> Option<String> {
    let mut composed = String::new();
    citations.retain(|citation| {
        let separator = if composed.is_empty() { "" } else { " " };
        let quoted = format!(
            "{separator}{label}: {content}",
            label = citation.source_label,
            content = citation.source_content
        );
        // The ceiling is on CHARACTERS, as the member's `@length` is.
        if composed.chars().count() + quoted.chars().count() > QUERY_TEXT_LIMIT {
            return false;
        }
        composed.push_str(&quoted);
        true
    });
    reportable_long_text(&composed)
}
