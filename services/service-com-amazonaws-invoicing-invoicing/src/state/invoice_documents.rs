// SPDX-License-Identifier: CC0-1.0

//! The DOCUMENTS issued with an invoice, where each one is downloaded from, and the bytes
//! served at that address.
//!
//! `GetInvoicePDF` "returns a URL to download the invoice document and supplemental
//! documents associated with an invoice. The URLs are pre-signed and have expiration
//! time" (`smithy_query(query="doc", shape="GetInvoicePDF")`), so an invoice this emulator
//! holds has TWO things beside its summary: the invoice document itself, which every
//! invoice has, and the supplemental documents issued with it, which are whatever the
//! issuer produced for that invoice — a government invoice, a tax e-invoice, a payment
//! receipt or a supplement ([`crate::ops::types::SUPPLEMENTAL_DOCUMENT_TYPE_VALUES`]).
//! Both are held with the invoice ([`StoredInvoice`]) rather than in a registry of their
//! own, so there is one copy of an invoice and its documents and no document set can
//! outlive, or exist without, the invoice it belongs to.
//!
//! No operation of this service's API issues an invoice or a document (the registry starts
//! EMPTY — see the `crate::state` module docs), so what a reader reports is exactly what
//! the registry holds: an invoice with no supplemental documents reports none, and an
//! invoice id the registry does not hold is the modelled `ResourceNotFoundException`.
//!
//! A document's URL is NOT stored with the document. A pre-signed URL carries a signature
//! and an expiry of its own, so it is minted per read ([`signed_document_url`]) against
//! the base URL this emulator is reached on and reported with the instant that signature
//! stops verifying; a stored URL would hand a later caller a signature that has already
//! expired. What IS stored is the document's own identity — its type and its id — which is
//! what the document the URL leads to is rendered from.
//!
//! The address itself is this module's ([`invoice_document_path`],
//! [`supplemental_document_path`]), read back by [`parse_invoice_document_path`], so the
//! URL `GetInvoicePDF` hands out and the request `crate::service`'s document plane claims
//! cannot disagree about which document is named. The invoice id is the LAST thing a path
//! carries because `InvoiceId` admits any text but a newline
//! (`StringWithoutNewLine`, `@pattern ^.*$`) — a `/` in an id included — and SigV4 leaves a
//! path's separators as written, so reading the id as everything after the fixed segments
//! names the same invoice the URL was signed for whatever the id holds.
//!
//! A path also carries the ACCOUNT whose invoices it addresses, taken from
//! [`::engine::emulator_account_id`] when the address is minted rather than from anything a
//! request holds, and an AUTHENTICATOR this store computed over the whole of WHICH document
//! the address names ([`ComAmazonawsInvoicingInvoicingState::document_token`]). A
//! pre-signed fetch carries no caller identity of its own — the framework verifies a
//! signature over the method and the path, which says nothing about whose documents may be
//! read — so what stands in its place is that authenticator: an address is a value only
//! this store could have produced, and
//! [`ComAmazonawsInvoicingInvoicingState::invoice_document`] serves a document only against
//! the one minted for it, in the account whose invoices this store holds. An address a
//! caller assembled, one minted for another document, and one minted by another process are
//! each answered exactly as an address naming a document that does not exist.

use std::time::Duration;

use super::*;
use crate::ops::types::{InvoiceCurrencyAmount, SupplementalDocument};

/// One invoice this emulator holds: the summary a reader reports it with, and the
/// supplemental documents issued with it.
///
/// `summary` is the modelled `InvoiceSummary` itself, so `ListInvoiceSummaries` reports
/// stored state rather than a projection of it, and the documents sit beside it because
/// they belong to the same invoice.
#[derive(Debug, Clone, Default)]
pub struct StoredInvoice {
    /// The invoice as a reader reports it.
    pub summary: InvoiceSummary,
    /// The supplemental documents issued with this invoice, in the order the issuer
    /// produced them — the order `GetInvoicePDF` reports them in.
    ///
    /// Each is the modelled `SupplementalDocument` with its `DocumentType` and
    /// `DocumentId` set and its two URL members left unset: the URL and its expiry are
    /// minted per read (see the module docs), never stored.
    pub supplemental_documents: Vec<SupplementalDocument>,
}

impl StoredInvoice {
    /// An invoice with no supplemental documents, which is what an invoice has unless the
    /// issuer produced some.
    #[must_use]
    pub fn new(summary: InvoiceSummary) -> Self {
        Self {
            summary,
            supplemental_documents: Vec::new(),
        }
    }

    /// The same invoice, with the supplemental documents issued with it.
    #[must_use]
    pub fn with_supplemental_documents(
        mut self,
        documents: Vec<SupplementalDocument>,
    ) -> Self {
        self.supplemental_documents = documents;
        self
    }
}

/// How long a document URL this service hands out stays valid.
///
/// The model documents that the URLs "are pre-signed and have expiration time" without
/// publishing the span, so this is the emulator's own choice: one hour, long enough for a
/// caller to fetch every document one read reported and well inside the maximum the
/// framework's signer accepts. The instant a reader REPORTS is the signature's own
/// ([`signed_document_url`]), so the reported expiry and the URL's real lifetime cannot
/// disagree whatever this span is.
pub const INVOICE_DOCUMENT_URL_EXPIRY: Duration = Duration::from_secs(60 * 60);

/// The media type an invoice document is served as: it is a PDF document
/// (`GetInvoicePDF`), written by a PDF writer.
pub const INVOICE_DOCUMENT_MEDIA_TYPE: &str = "application/pdf";

/// The query field a document address carries its AUTHENTICATOR in.
///
/// An address a reader hands out is authenticated by the store that minted it (see the
/// module docs and [`ComAmazonawsInvoicingInvoicingState::document_token`]), and the
/// authenticator travels in the query the way the other document planes of this emulator
/// carry the receipt that authorises a fetch. It is bound into the URL's own signature,
/// so it cannot be swapped for another document's.
pub const DOCUMENT_TOKEN_FIELD: &str = "documentToken";

/// The first path segment every document of this service is published under, which is what
/// makes a request one the document plane serves.
const DOCUMENT_ROOT_SEGMENT: &str = "invoice-documents";

/// The segment that names the INVOICE document of an invoice.
const INVOICE_DOCUMENT_SEGMENT: &str = "invoice";

/// The segment that names a SUPPLEMENTAL document of an invoice.
const SUPPLEMENTAL_DOCUMENT_SEGMENT: &str = "supplemental";

/// The byte that separates the fields of what an address's authenticator is computed over
/// ([`InvoiceDocumentTarget::cursor`]): a control character, so no id can spell it.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// Which document an address of this plane names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvoiceDocumentTarget {
    /// The invoice document of one invoice.
    Invoice {
        /// The account whose invoices the address names (see [`InvoiceDocumentTarget`]).
        account_id: String,
        /// The id the registry keys that invoice under.
        invoice_id: String,
    },
    /// One supplemental document of one invoice.
    Supplemental {
        /// The account whose invoices the address names.
        account_id: String,
        /// The id the registry keys that invoice under.
        invoice_id: String,
        /// The document's position in that invoice's stored list.
        position: usize,
    },
}

impl InvoiceDocumentTarget {
    /// The invoice document of the invoice `invoice_id` names, in the account this emulator
    /// serves — which is whose invoices the registry holds, so the account is taken from
    /// the account contract and never from anything a request carries.
    #[must_use]
    pub fn invoice(invoice_id: &str) -> Self {
        Self::Invoice {
            account_id: ::engine::emulator_account_id().to_string(),
            invoice_id: invoice_id.to_string(),
        }
    }

    /// The supplemental document at `position` in that invoice's stored list.
    ///
    /// The document is addressed by its POSITION rather than by its `DocumentId`: the
    /// position is the one part of a document's identity this service assigns, it carries
    /// no path separator, and it leaves the invoice id as the address's tail (see the
    /// module docs).
    #[must_use]
    pub fn supplemental(invoice_id: &str, position: usize) -> Self {
        Self::Supplemental {
            account_id: ::engine::emulator_account_id().to_string(),
            invoice_id: invoice_id.to_string(),
            position,
        }
    }

    /// The account the address this target was read from names.
    #[must_use]
    pub fn account_id(&self) -> &str {
        match self {
            Self::Invoice { account_id, .. } | Self::Supplemental { account_id, .. } => account_id,
        }
    }

    /// The path this document is published at.
    #[must_use]
    pub fn path(&self) -> String {
        match self {
            Self::Invoice {
                account_id,
                invoice_id,
            } => format!(
                "/{DOCUMENT_ROOT_SEGMENT}/{account_id}/{INVOICE_DOCUMENT_SEGMENT}/{invoice_id}"
            ),
            Self::Supplemental {
                account_id,
                invoice_id,
                position,
            } => format!(
                "/{DOCUMENT_ROOT_SEGMENT}/{account_id}/{SUPPLEMENTAL_DOCUMENT_SEGMENT}/{position}/\
                 {invoice_id}"
            ),
        }
    }

    /// What an address's authenticator is computed over: every part of WHICH document this
    /// is, the account included, in one text no two targets share.
    fn cursor(&self) -> String {
        match self {
            Self::Invoice {
                account_id,
                invoice_id,
            } => format!(
                "{account_id}{CURSOR_SEPARATOR}{INVOICE_DOCUMENT_SEGMENT}{CURSOR_SEPARATOR}\
                 {invoice_id}"
            ),
            Self::Supplemental {
                account_id,
                invoice_id,
                position,
            } => format!(
                "{account_id}{CURSOR_SEPARATOR}{SUPPLEMENTAL_DOCUMENT_SEGMENT}\
                 {CURSOR_SEPARATOR}{position}{CURSOR_SEPARATOR}{invoice_id}"
            ),
        }
    }
}

/// The document `segments` names — the path's segments, percent-decoded and with the empty
/// ones KEPT — or `None` when they address no document of this plane.
///
/// Built from the SAME parts [`InvoiceDocumentTarget::path`] lays out, so an address this
/// service minted resolves here and nothing else does, and an id is read back exactly as it
/// was written: the id is rejoined from every remaining segment, empty ones included, so two
/// ids differing only in their separators are two addresses rather than one. Neither whether
/// the address is one this store MINTED nor whether the document EXISTS is decided here:
/// [`ComAmazonawsInvoicingInvoicingState::invoice_document`] decides both, so a path naming
/// a document this emulator does not hold — or carrying no authenticator of this store — is
/// answered as the document it does not hold rather than as a request addressing nothing at
/// all.
#[must_use]
pub fn parse_invoice_document_path(segments: &[String]) -> Option<InvoiceDocumentTarget> {
    let (root, rest) = segments.split_first()?;
    if root != DOCUMENT_ROOT_SEGMENT {
        return None;
    }
    // WHOSE documents the path names, read back from the path; that it is the account whose
    // registry answers, and that this store minted the address at all, is checked in
    // [`ComAmazonawsInvoicingInvoicingState::invoice_document`].
    let (account_id, rest) = rest.split_first()?;
    let account_id = account_id.clone();
    let (kind, rest) = rest.split_first()?;
    match kind.as_str() {
        INVOICE_DOCUMENT_SEGMENT => Some(InvoiceDocumentTarget::Invoice {
            account_id,
            invoice_id: rest.join("/"),
        }),
        SUPPLEMENTAL_DOCUMENT_SEGMENT => {
            let (position, rest) = rest.split_first()?;
            Some(InvoiceDocumentTarget::Supplemental {
                account_id,
                position: position.parse::<usize>().ok()?,
                invoice_id: rest.join("/"),
            })
        }
        _ => None,
    }
}

/// One document URL, pre-signed against the base URL this emulator is reached on, with the
/// instant that signature stops verifying.
///
/// Signed through the framework's URL signer, which is what makes the address both a
/// pre-signed one and one this emulator ANSWERS: it signs for the emulator's current base
/// URL and for this service's own signing name, so the URL leads back to the document
/// plane that serves the document rather than to a host only AWS serves.
///
/// `token` is the authenticator the store minted for this exact document
/// ([`ComAmazonawsInvoicingInvoicingState::document_token`]). It travels as a query field
/// and is signed in, so the address is BOTH a pre-signed URL and one no caller could have
/// assembled: the document plane serves a document only against an authenticator this store
/// computed for it.
///
/// `None` for a request that cannot be signed. Nothing a caller sends reaches that — the
/// path holds a stored id and the expiry is a constant inside the signer's range — and a
/// URL that cannot be signed is one nothing could download the document from, so a reader
/// answers the modelled server error for it ([`document_url_unsignable`]) rather than a
/// success carrying no address.
pub fn signed_document_url(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    target: &InvoiceDocumentTarget,
    token: &str,
) -> Option<(String, DateTime)> {
    let path = target.path();
    let request = smithy_server_runtime::SignedUrlRequest::new(
        smithy_server_runtime::http::Method::GET,
        ctx.identity().signing_name(),
        &path,
    )
    .query(DOCUMENT_TOKEN_FIELD, token);
    match ctx.url_signer().sign(request, INVOICE_DOCUMENT_URL_EXPIRY) {
        Ok(signed) => Some((
            signed.as_str().to_string(),
            DateTime::from(signed.expires_at()),
        )),
        Err(error) => {
            ::tracing::warn!(%error, "an invoice document URL could not be signed");
            None
        }
    }
}

/// The 500 a reader answers when a document's address cannot be signed
/// ([`signed_document_url`]).
///
/// A read of an invoice this emulator holds owes an address the caller can fetch the
/// document from, so a URL that cannot be signed is a failure of the read rather than a
/// success reporting no address: the sentence is the one the model documents
/// `InternalServerException` with ("The processing request failed because of an unknown
/// error, exception, or failure", `smithy_query(query="shape",
/// shape="InternalServerException")`), because nothing about the request explains it.
#[must_use]
pub fn document_url_unsignable() -> InternalServerException {
    InternalServerException {
        message: Some(
            "The processing request failed because of an unknown error, exception, or failure."
                .to_string(),
        ),
        retry_after_seconds: None,
    }
}

impl ComAmazonawsInvoicingInvoicingState {
    /// The authenticator the address of `target` carries: this store's own tag over WHICH
    /// document the address names ([`InvoiceDocumentTarget::cursor`]), rendered as the hex
    /// the address spells it in.
    ///
    /// Computed under the secret this store drew from the system's randomness when it was
    /// created and never reveals ([`::service_common::page_tokens::PageTokenKey`], the one
    /// place this workspace authenticates a value a store hands out and takes back). So an
    /// address is a value only this store could have produced: a caller cannot assemble one
    /// for a document it was not handed an address for, whatever it knows about the account,
    /// the invoice ids or the path layout, and an address another process minted does not
    /// resolve here either.
    #[must_use]
    pub fn document_token(&self, target: &InvoiceDocumentTarget) -> String {
        ::hex::encode(self.document_address_key.mint(&target.cursor()))
    }

    /// The document `target` names, as the bytes served at its URL, or `None` when this
    /// store holds no such document: an address this store did not mint (`token` is not its
    /// authenticator for this exact document), an address naming another account's invoices,
    /// an invoice id the registry does not hold, or a position past the supplemental
    /// documents that invoice carries. Every one of those is answered the same way, so a
    /// refusal tells a caller nothing about which it was.
    ///
    /// Neither the ACCOUNT nor the document's identity is taken on the address's word. A
    /// pre-signed fetch carries no caller identity of its own — what the framework verifies
    /// is a signature over the method and the path, which says nothing about whose documents
    /// may be read — so what stands in for it is the authenticator
    /// ([`Self::document_token`]): the address is good for exactly the document this store
    /// minted it for, and the account it names is checked against the account whose invoices
    /// this store holds ([`::engine::emulator_account_id`]) before the registry is read at
    /// all.
    ///
    /// The document is RENDERED from the stored invoice on each fetch rather than held as
    /// bytes, so what a download serves is the invoice the registry holds at that moment
    /// and a reader and a download cannot disagree about it.
    #[must_use]
    pub fn invoice_document(
        &self,
        target: &InvoiceDocumentTarget,
        token: &str,
    ) -> Option<Vec<u8>> {
        if target.account_id() != ::engine::emulator_account_id() {
            return None;
        }
        let minted = self
            .document_address_key
            .cursor_of(&::hex::decode(token).ok()?)?;
        if minted != target.cursor() {
            return None;
        }
        match target {
            InvoiceDocumentTarget::Invoice { invoice_id, .. } => {
                let invoice = self.invoices.get(invoice_id)?;
                Some(pdf_page(&invoice_document_lines(invoice_id, invoice)))
            }
            InvoiceDocumentTarget::Supplemental {
                invoice_id,
                position,
                ..
            } => {
                let invoice = self.invoices.get(invoice_id)?;
                let document = invoice.supplemental_documents.get(*position)?;
                Some(pdf_page(&supplemental_document_lines(
                    invoice_id, invoice, document,
                )))
            }
        }
    }
}

/// What the invoice document of `invoice` SAYS: the invoice as the registry holds it, one
/// stored member per line, and nothing that is not stored.
///
/// Only the members the stored invoice carries are shown — an absent member is a member
/// the issuer did not state, so the document states nothing for it rather than a value
/// nothing holds.
fn invoice_document_lines(invoice_id: &str, invoice: &StoredInvoice) -> Vec<String> {
    let summary = &invoice.summary;
    let mut lines = vec![format!("Invoice {invoice_id}"), String::new()];
    let fields: [(&str, Option<String>); 14] = [
        ("Account id", summary.account_id.clone()),
        ("Issued date", summary.issued_date.as_ref().map(instant_text)),
        ("Due date", summary.due_date.as_ref().map(instant_text)),
        (
            "Billing period",
            summary.billing_period.as_ref().map(|period| {
                format!("{year}-{month:02}", year = period.year, month = period.month)
            }),
        ),
        (
            "Invoicing entity",
            summary
                .entity
                .as_ref()
                .and_then(|entity| entity.invoicing_entity.clone()),
        ),
        (
            "Billing entity",
            summary
                .entity
                .as_ref()
                .and_then(|entity| entity.billing_entity.clone()),
        ),
        ("Invoice type", summary.invoice_type.clone()),
        ("Bill type", summary.bill_type.clone()),
        ("Invoice frequency", summary.invoice_frequency.clone()),
        ("Receiver role", summary.receiver_role.clone()),
        ("Purchase order number", summary.purchase_order_number.clone()),
        ("Commercial invoice id", summary.commercial_invoice_id.clone()),
        ("Original invoice id", summary.original_invoice_id.clone()),
        (
            "Bill source accounts",
            summary
                .bill_source_accounts
                .as_ref()
                .filter(|accounts| !accounts.is_empty())
                .map(|accounts| accounts.join(", ")),
        ),
    ];
    lines.extend(
        fields
            .into_iter()
            .filter_map(|(label, value)| value.map(|value| format!("{label}: {value}"))),
    );
    let amounts: [(&str, Option<&InvoiceCurrencyAmount>); 3] = [
        ("Base currency total", summary.base_currency_amount.as_ref()),
        ("Tax currency total", summary.tax_currency_amount.as_ref()),
        (
            "Payment currency total",
            summary.payment_currency_amount.as_ref(),
        ),
    ];
    lines.extend(
        amounts
            .into_iter()
            .filter_map(|(label, amount)| amount_text(amount?).map(|text| format!("{label}: {text}"))),
    );
    for (position, document) in invoice.supplemental_documents.iter().enumerate() {
        lines.push(format!(
            "Supplemental document {ordinal}: {description}",
            ordinal = position + 1,
            description = supplemental_document_text(document),
        ));
    }
    lines.extend(EMULATOR_NOTE.iter().map(|line| (*line).to_string()));
    lines
}

/// What a supplemental document SAYS: which document it is, and which invoice it was
/// issued with.
fn supplemental_document_lines(
    invoice_id: &str,
    invoice: &StoredInvoice,
    document: &SupplementalDocument,
) -> Vec<String> {
    let mut lines = vec![
        format!("Supplemental document for invoice {invoice_id}"),
        String::new(),
        format!("Document: {}", supplemental_document_text(document)),
    ];
    if let Some(issued) = invoice.summary.issued_date.as_ref().map(instant_text) {
        lines.push(format!("Invoice issued date: {issued}"));
    }
    lines.extend(EMULATOR_NOTE.iter().map(|line| (*line).to_string()));
    lines
}

/// The closing note every document of this plane carries: the document is this emulator's
/// rendering of the invoice it holds, so nothing reading one takes it for the document AWS
/// itself issues.
const EMULATOR_NOTE: [&str; 3] = [
    "",
    "Rendered by the AWS Invoicing emulator from the invoice this account holds.",
    "The document the real service issues is AWS's own and is not reproduced here.",
];

/// How a stored supplemental document is described in a document's text: its type and its
/// id, each shown only when the stored document carries it.
fn supplemental_document_text(document: &SupplementalDocument) -> String {
    let parts: Vec<String> = [
        document.document_type.clone(),
        document
            .document_id
            .clone()
            .map(|id| format!("id {id}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    match parts.is_empty() {
        true => "an unidentified supplemental document".to_string(),
        false => parts.join(", "),
    }
}

/// One stored amount as a document states it: the total, and the currency it is stated in
/// when the stored amount names one.
fn amount_text(amount: &InvoiceCurrencyAmount) -> Option<String> {
    let total = amount.total_amount.clone()?;
    Some(match amount.currency_code.as_deref() {
        Some(currency) => format!("{total} {currency}"),
        None => total,
    })
}

/// One stored instant as a document states it: ISO-8601, the rendering the wire gives a
/// modelled timestamp.
fn instant_text(instant: &DateTime) -> String {
    instant
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| instant.secs().to_string())
}

/// A one-page PDF document showing `lines`, one per line, in the Base-14 Helvetica face
/// every PDF reader carries (so nothing is embedded and the document stands alone).
///
/// The bytes are written by the `pdf-writer` crate, so what the document plane serves is a
/// document a PDF reader opens rather than bytes shaped like one.
fn pdf_page(lines: &[String]) -> Vec<u8> {
    use ::pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

    /// The ISO 216 A4 page box in PDF points (1/72 inch).
    const PAGE_BOX: [f32; 4] = [0.0, 0.0, 595.28, 841.89];
    /// Point size of the body text, and the baseline-to-baseline distance that goes with
    /// it.
    const FONT_SIZE: f32 = 11.0;
    /// See [`FONT_SIZE`].
    const LEADING: f32 = 15.0;
    /// Where the first line's baseline sits, in page points from the page origin.
    const TEXT_ORIGIN: (f32, f32) = (56.0, 785.0);

    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let page_id = Ref::new(3);
    let font_id = Ref::new(4);
    let content_id = Ref::new(5);
    let font_name = Name(b"F1");

    let mut content = Content::new();
    content.begin_text();
    content.set_font(font_name, FONT_SIZE);
    // The first move is from the page origin to the top-left text start; every later one
    // steps down one line.
    content.next_line(TEXT_ORIGIN.0, TEXT_ORIGIN.1);
    for line in lines {
        content.show(Str(line.as_bytes()));
        content.next_line(0.0, -LEADING);
    }
    content.end_text();
    let content = content.finish();

    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids([page_id]).count(1);
    let mut page = pdf.page(page_id);
    page.parent(page_tree_id);
    page.media_box(Rect::new(
        PAGE_BOX[0], PAGE_BOX[1], PAGE_BOX[2], PAGE_BOX[3],
    ));
    page.contents(content_id);
    page.resources().fonts().pair(font_name, font_id);
    page.finish();
    pdf.type1_font(font_id).base_font(Name(b"Helvetica"));
    pdf.stream(content_id, &content);
    pdf.finish()
}
