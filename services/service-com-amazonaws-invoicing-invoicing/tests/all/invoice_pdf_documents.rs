// SPDX-License-Identifier: CC0-1.0

//! `GetInvoicePDF` and the document plane it hands out URLs for, driven end to end over
//! this service's wire protocol.
//!
//! Every API call here goes through the same `MultiServiceRouter` the production router
//! uses: `POST /` with `X-Amz-Target: Invoicing.GetInvoicePDF` and an awsJson body, and
//! every document is fetched by dispatching the URL the operation reported exactly as it
//! was reported — so what these tests see is what an SDK client and a plain HTTP client
//! see.
//!
//! The one thing these tests reach into the state struct for is SEEDING the invoice
//! registry: an invoice is issued by AWS out of a billing period's activity and no
//! operation of this service's API creates one, so there is no request that could put one
//! there (the same shape as the other suites of this crate).
//!
//! The rule the table below asserts, over a set of inputs rather than one request: for the
//! invoice its `InvoiceId` names, the operation reports a pre-signed URL that leads back to
//! this emulator — with the instant that URL's own signature stops verifying — for the
//! invoice document and for each supplemental document the invoice was issued with, in the
//! stored order; the document served at each address is rendered from that stored invoice;
//! and an `InvoiceId` the registry does not hold is the modelled `ResourceNotFoundException`.
//! Each address names the account whose invoices it serves, and an address naming another
//! account is served no document.
//! The rows differ along what the rule generalises over: how many supplemental documents an
//! invoice carries and of which modelled types, how much of its summary is stored, and the
//! form of the invoice id — including both ends of the length the model gives it, an id
//! carrying the separators a URL path is cut on, and pairs of ids differing ONLY in those
//! separators.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use service_com_amazonaws_invoicing_invoicing::ops::types::{
    BillingPeriod, Entity, InvoiceCurrencyAmount, InvoiceSummary, SupplementalDocument,
    SUPPLEMENTAL_DOCUMENT_TYPE_VALUES,
};
use service_com_amazonaws_invoicing_invoicing::state::StoredInvoice;

/// The account every seeded invoice below is issued to.
const ACCOUNT: &str = "111111111111";

/// The invoicing entity the richer rows store, which the invoice document names.
const INVOICING_ENTITY: &str = "Amazon Web Services, Inc.";

/// The span the pre-signed document URLs of this service are signed for, in seconds, as
/// `crate::state::INVOICE_DOCUMENT_URL_EXPIRY` states it.
const EXPIRY_SECONDS: i64 = 60 * 60;

/// The model's own maximum for `InvoiceId` (`smithy_query(query="operation",
/// operation="GetInvoicePDF")`: `StringWithoutNewLine`, `len 0..1024`), so the table covers
/// both published ends of the id's length.
const INVOICE_ID_MAX_LENGTH: usize = 1024;

/// One invoice to seed, and what the operation then owes about it.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    /// The id the registry keys the invoice under, which is the id a request names.
    invoice_id: String,
    /// `(DocumentType, DocumentId)` per supplemental document, in the order they were
    /// issued — the order the operation must report them in.
    supplemental: Vec<(String, String)>,
    /// Whether the stored summary carries the dated, entity and amount members as well as
    /// the account id, so the document rendered from it is shown to follow the record
    /// rather than a fixed layout.
    detailed: bool,
}

impl Row {
    fn new(case: &str, invoice_id: &str, supplemental: &[(&str, &str)], detailed: bool) -> Self {
        Self {
            case: case.to_string(),
            invoice_id: invoice_id.to_string(),
            supplemental: supplemental
                .iter()
                .map(|(kind, id)| ((*kind).to_string(), (*id).to_string()))
                .collect(),
            detailed,
        }
    }

    /// The invoice as the registry holds it.
    fn to_stored(&self) -> StoredInvoice {
        let mut summary = InvoiceSummary {
            account_id: Some(ACCOUNT.to_string()),
            invoice_id: Some(self.invoice_id.clone()),
            ..Default::default()
        };
        if self.detailed {
            summary.issued_date = Some(::aws_smithy_types::DateTime::from_secs(secs(
                "2025-08-03T00:00:00Z",
            )));
            summary.due_date = Some(::aws_smithy_types::DateTime::from_secs(secs(
                "2025-09-02T00:00:00Z",
            )));
            summary.billing_period = Some(BillingPeriod {
                month: 8,
                year: 2025,
            });
            summary.entity = Some(Entity {
                invoicing_entity: Some(INVOICING_ENTITY.to_string()),
                billing_entity: Some("AWS".to_string()),
            });
            summary.base_currency_amount = Some(InvoiceCurrencyAmount {
                total_amount: Some("1234.56".to_string()),
                currency_code: Some("USD".to_string()),
                ..Default::default()
            });
        }
        StoredInvoice::new(summary).with_supplemental_documents(
            self.supplemental
                .iter()
                .map(|(kind, id)| SupplementalDocument {
                    document_type: Some(kind.clone()),
                    document_id: Some(id.clone()),
                    ..Default::default()
                })
                .collect(),
        )
    }
}

/// The invoices every test below seeds.
fn rows() -> Vec<Row> {
    vec![
        Row::new("an invoice issued with no supplemental document", "INV-001", &[], true),
        Row::new(
            "one supplemental document",
            "INV-002",
            &[("TAX_E_INVOICE", "TEI-002")],
            true,
        ),
        // One document of every type the model declares, which is also the order they are
        // stored in and so the order they must be reported in.
        Row::new(
            "a document of every modelled supplemental type",
            "INV-003",
            &[
                ("GOVERNMENT_INVOICE", "GOV-003"),
                ("PAYMENT_RECEIPT", "PR-003"),
                ("SUPPLEMENT", "SUP-003"),
                ("TAX_E_INVOICE", "TEI-003"),
            ],
            true,
        ),
        // An id carrying the separators a URL path is cut on, and a space, which the
        // address has to survive for the document to resolve back to this invoice.
        Row::new(
            "an invoice id carrying a path separator and a space",
            "INV 2025/08/001",
            &[("SUPPLEMENT", "SUP 2025/08")],
            false,
        ),
        // An id carrying the characters a URL gives its own meaning to, including an
        // already-escaped separator: the address has to carry them through the signer's own
        // encoding and back without naming a different invoice.
        Row::new(
            "an invoice id carrying URL-significant characters",
            "INV%2F009?x#y",
            &[("TAX_E_INVOICE", "TEI%2F009")],
            false,
        ),
        // The model's own minimum length for `InvoiceId` is 0, so the empty id is an id
        // the registry may hold.
        Row::new(
            "an invoice id at the modelled minimum length",
            "",
            &[("PAYMENT_RECEIPT", "PR-EMPTY")],
            false,
        ),
        Row::new(
            "an invoice id at the modelled maximum length",
            &"X".repeat(INVOICE_ID_MAX_LENGTH),
            &[],
            false,
        ),
        // Two pairs of ids differing ONLY in the separators a URL path is cut on: each
        // pair is two invoices, so an address that dropped a separator would serve the
        // other invoice's documents.
        Row::new(
            "an invoice id another id's trailing separator would alias",
            "INV-005",
            &[("SUPPLEMENT", "SUP-005")],
            false,
        ),
        Row::new(
            "an invoice id with a trailing separator",
            "INV-005/",
            &[("SUPPLEMENT", "SUP-005-TRAILING")],
            false,
        ),
        Row::new(
            "an invoice id another id's doubled separator would alias",
            "INV/006",
            &[("GOVERNMENT_INVOICE", "GOV-006")],
            false,
        ),
        Row::new(
            "an invoice id with a doubled separator",
            "INV//006",
            &[("GOVERNMENT_INVOICE", "GOV-006-DOUBLED")],
            false,
        ),
    ]
}

/// The epoch second an ISO-8601 instant names, so the dates above read as dates.
fn secs(instant: &str) -> i64 {
    ::chrono::DateTime::parse_from_rfc3339(instant)
        .unwrap_or_else(|error| panic!("case {instant:?}: a valid instant, got {error}"))
        .timestamp()
}

/// A router carrying only this service, over one shared engine — the same assembly the
/// production router performs, minus the other services. The engine comes back so a test
/// can seed the invoice registry the service reads.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is the router's concern, not this slice's: drive the operations unsigned.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// Put the table's invoices in the service's registry, keyed by invoice id.
fn seed(engine: &engine::InMemoryEngine, rows: &[Row]) {
    let state = service_com_amazonaws_invoicing_invoicing::state::service_state(engine)
        .expect("the shared engine holds this service's state");
    let mut store = state.lock();
    for row in rows {
        store
            .invoices
            .insert(row.invoice_id.clone(), row.to_stored());
    }
}

/// How many invoices the registry holds, so a read can be shown not to change it.
fn invoice_count(engine: &engine::InMemoryEngine) -> usize {
    let state = service_com_amazonaws_invoicing_invoicing::state::service_state(engine)
        .expect("the shared engine holds this service's state");
    let store = state.lock();
    store.invoices.len()
}

/// One `GetInvoicePDF` call, returning the HTTP status and the parsed body.
async fn get_invoice_pdf(router: &MultiServiceRouter, invoice_id: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", "Invoicing.GetInvoicePDF")
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&json!({"InvoiceId": invoice_id}))
                .expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// What a fetch of one announced URL answered.
#[derive(Debug)]
struct Fetched {
    status: u16,
    media_type: String,
    body: Vec<u8>,
}

impl Fetched {
    /// The document's bytes as text, for an assertion about what the document SAYS. The
    /// content stream of the one-page document this service writes is not compressed, so
    /// the text it shows is in these bytes.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// Fetch one URL this emulator handed out, exactly as it was handed out.
async fn fetch(router: &MultiServiceRouter, url: &str) -> Fetched {
    let host = url
        .parse::<http::Uri>()
        .ok()
        .and_then(|uri| uri.authority().map(|authority| authority.to_string()))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method("GET")
        .uri(url)
        .header("host", host)
        .body(bytes::Bytes::new())
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let media_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Fetched {
        status,
        media_type,
        body: response.into_body().to_vec(),
    }
}

/// The wire error code of a failure body (`__type` on the awsJson protocols), without the
/// namespace a body may qualify it with.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `InvoicePDF` structure a success reports.
fn invoice_pdf(body: &Value) -> &Value {
    body.get("InvoicePDF")
        .unwrap_or_else(|| panic!("expected an InvoicePDF member, got {body}"))
}

/// A string member of a reported structure.
fn text_member(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The supplemental documents a success reports, in the reported order.
fn supplemental_documents(pdf: &Value) -> Vec<Value> {
    pdf.get("SupplementalDocuments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The value of one query field of an announced URL, percent-decoded.
fn query_field(url: &str, field: &str) -> Option<String> {
    let query = url.split_once('?').map(|(_, query)| query)?;
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == field).then(|| smithy_server_runtime::percent_decode(value))
    })
}

/// The media type a document is served as.
const PDF_MEDIA_TYPE: &str = "application/pdf";

/// The first bytes of every PDF document, which is how a client knows it got one.
const PDF_MAGIC: &str = "%PDF-";

/// `token` with its first character changed, for the case that a value one character off an
/// authenticator this store minted is not one it minted.
fn flip_first(token: &str) -> String {
    let mut characters: Vec<char> = token.chars().collect();
    if let Some(first) = characters.first_mut() {
        *first = if *first == '0' { '1' } else { '0' };
    }
    characters.into_iter().collect()
}

/// The seeded `DocumentType` values are values the MODEL declares, so the rows above are a
/// catalog read off the model rather than wording invented here.
#[test]
fn seeded_document_types_are_modelled_values() {
    for row in rows() {
        for (kind, _) in &row.supplemental {
            assert!(
                SUPPLEMENTAL_DOCUMENT_TYPE_VALUES.contains(&kind.as_str()),
                "case {}: {kind:?} is not one of the modelled SupplementalDocumentType \
                 values {SUPPLEMENTAL_DOCUMENT_TYPE_VALUES:?}",
                row.case
            );
        }
    }
}

/// RULE: every document of a stored invoice is reported with a pre-signed URL that leads
/// back to this emulator, and the document served there is rendered from that invoice.
#[tokio::test]
async fn every_document_of_a_stored_invoice_is_reported_with_a_fetchable_url() {
    let (router, engine) = emulator();
    let rows = rows();
    seed(&engine, &rows);

    for row in &rows {
        let case = &row.case;
        let (status, body) = get_invoice_pdf(&router, &row.invoice_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        let pdf = invoice_pdf(&body);
        assert_eq!(
            text_member(pdf, "InvoiceId").as_deref(),
            Some(row.invoice_id.as_str()),
            "case {case:?}: expected the stored invoice id, got {pdf}"
        );

        // The invoice document, and then each supplemental document: the address, what it
        // serves, and what the document says.
        let document_url = text_member(pdf, "DocumentUrl")
            .unwrap_or_else(|| panic!("case {case:?}: expected a DocumentUrl, got {pdf}"));
        let reported = supplemental_documents(pdf);
        assert_eq!(
            reported.len(),
            row.supplemental.len(),
            "case {case:?}: expected one entry per stored supplemental document, got \
             {reported:?}"
        );

        let mut addresses = vec![document_url.clone()];
        for (entry, (kind, id)) in reported.iter().zip(&row.supplemental) {
            assert_eq!(
                text_member(entry, "DocumentType").as_deref(),
                Some(kind.as_str()),
                "case {case:?}: expected the stored document type, got {entry}"
            );
            assert_eq!(
                text_member(entry, "DocumentId").as_deref(),
                Some(id.as_str()),
                "case {case:?}: expected the stored document id, got {entry}"
            );
            let url = text_member(entry, "DocumentUrl").unwrap_or_else(|| {
                panic!("case {case:?}: expected a DocumentUrl on {entry}")
            });
            addresses.push(url);
        }
        for (position, url) in addresses.iter().enumerate() {
            assert!(
                url.starts_with("https://") && query_field(url, "X-Amz-Signature").is_some(),
                "case {case:?} document {position}: expected a pre-signed URL on this \
                 emulator, got {url:?}"
            );
            assert_eq!(
                query_field(url, "X-Amz-Credential")
                    .and_then(|credential| credential
                        .split('/')
                        .nth(3)
                        .map(str::to_string)),
                Some("invoicing".to_string()),
                "case {case:?} document {position}: the URL must be signed for this \
                 service, got {url:?}"
            );
            assert!(
                url.contains(&format!("/{}/", engine::emulator_account_id())),
                "case {case:?} document {position}: the address must name the account \
                 whose invoices it serves, got {url:?}"
            );
            assert_eq!(
                addresses.iter().filter(|other| *other == url).count(),
                1,
                "case {case:?}: each document has its own address, got {addresses:?}"
            );

            let fetched = fetch(&router, url).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?} document {position}: expected the document at {url:?}, got \
                 {} {:?}",
                fetched.status,
                fetched.text()
            );
            assert_eq!(
                fetched.media_type, PDF_MEDIA_TYPE,
                "case {case:?} document {position}: expected {PDF_MEDIA_TYPE}, got {:?}",
                fetched.media_type
            );
            let text = fetched.text();
            assert!(
                text.starts_with(PDF_MAGIC),
                "case {case:?} document {position}: expected a PDF document, got {:?}",
                text.chars().take(32).collect::<String>()
            );
            // The document is rendered from the stored invoice, so it names what that
            // invoice holds: the account it was issued to and the invoice's own id, and —
            // for a supplemental document — the stored id of the document itself.
            if position == 0 {
                assert!(
                    text.contains(ACCOUNT),
                    "case {case:?}: the invoice document must name the stored account, got \
                     {text:?}"
                );
                assert!(
                    text.contains(&format!("Invoice {}", row.invoice_id)),
                    "case {case:?}: the invoice document must name the invoice it was \
                     rendered from, got {text:?}"
                );
            }
            if row.detailed && position == 0 {
                assert!(
                    text.contains(INVOICING_ENTITY) && text.contains("1234.56 USD"),
                    "case {case:?}: the invoice document must state the stored entity and \
                     amount, got {text:?}"
                );
            }
            if position > 0 {
                let (kind, id) = &row.supplemental[position - 1];
                assert!(
                    text.contains(kind.as_str()) && text.contains(id.as_str()),
                    "case {case:?} document {position}: the document must name its stored \
                     type and id, got {text:?}"
                );
            }
        }
    }

    assert_eq!(
        invoice_count(&engine),
        rows.len(),
        "a read of a document location changes no registry entry"
    );
}

/// RULE: the expiry a read reports is the one the URL it reported carries, so a client
/// that holds the URL and a client that read the expiry agree about when it stops working.
#[tokio::test]
async fn the_reported_expiry_is_the_expiry_of_the_url_it_was_reported_with() {
    let (router, engine) = emulator();
    let rows = rows();
    seed(&engine, &rows);

    for row in &rows {
        let case = &row.case;
        let (_, body) = get_invoice_pdf(&router, &row.invoice_id).await;
        let pdf = invoice_pdf(&body).clone();
        let mut documents = vec![pdf.clone()];
        documents.extend(supplemental_documents(&pdf));
        for (position, document) in documents.iter().enumerate() {
            let url = text_member(document, "DocumentUrl").unwrap_or_else(|| {
                panic!("case {case:?} document {position}: expected a DocumentUrl")
            });
            let reported = document
                .get("DocumentUrlExpirationDate")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?} document {position}: expected an expiry, got {document}"
                    )
                });
            let expires_in: i64 = query_field(&url, "X-Amz-Expires")
                .and_then(|value| value.parse().ok())
                .unwrap_or_else(|| {
                    panic!("case {case:?} document {position}: expected X-Amz-Expires on {url:?}")
                });
            assert_eq!(
                expires_in, EXPIRY_SECONDS,
                "case {case:?} document {position}: expected the URL to be signed for \
                 {EXPIRY_SECONDS} seconds, got {expires_in}"
            );
            // The instant the signature was made, as the URL states it.
            let signed_at = query_field(&url, "X-Amz-Date")
                .and_then(|date| {
                    ::chrono::NaiveDateTime::parse_from_str(&date, "%Y%m%dT%H%M%SZ").ok()
                })
                .map(|date| date.and_utc().timestamp())
                .unwrap_or_else(|| {
                    panic!("case {case:?} document {position}: expected X-Amz-Date on {url:?}")
                });
            assert_eq!(
                reported as i64,
                signed_at + expires_in,
                "case {case:?} document {position}: the reported expiry must be the \
                 URL's own, got {reported} against {signed_at} + {expires_in}"
            );
        }
    }
}

/// RULE: an `InvoiceId` the registry does not hold is the modelled 404, and the read
/// leaves the registry exactly as it found it.
#[tokio::test]
async fn an_invoice_id_the_registry_does_not_hold_is_the_modelled_404() {
    let (router, engine) = emulator();
    let rows = rows();
    seed(&engine, &rows);
    let before = invoice_count(&engine);

    // Ids no seeded invoice is keyed under: one of another form, one differing from a
    // seeded id by case, by a trailing space, by a path separator, and one at the
    // modelled maximum length.
    let unknown = [
        "INV-404".to_string(),
        "inv-001".to_string(),
        "INV-001 ".to_string(),
        "INV-001/supplement".to_string(),
        "Z".repeat(INVOICE_ID_MAX_LENGTH),
    ];
    for invoice_id in unknown {
        let (status, body) = get_invoice_pdf(&router, &invoice_id).await;
        assert_eq!(
            status, 404,
            "case {invoice_id:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {invoice_id:?}: expected the modelled 404, got {body}"
        );
        assert_eq!(
            text_member(&body, "message").as_deref(),
            Some(service_com_amazonaws_invoicing_invoicing::state::RESOURCE_NOT_FOUND_MESSAGE),
            "case {invoice_id:?}: expected this plane's own not-found wording, got {body}"
        );
        assert!(
            body.get("InvoicePDF").is_none(),
            "case {invoice_id:?}: a refusal reports no document, got {body}"
        );
    }
    assert_eq!(
        invoice_count(&engine),
        before,
        "a refused read mints no invoice"
    );
}

/// RULE: the read reports, it does not write — two reads of one invoice report the same
/// invoice and the same documents, and the listing that shares the registry is unchanged.
#[tokio::test]
async fn a_read_leaves_every_later_caller_seeing_the_same_invoice() {
    let (router, engine) = emulator();
    let rows = rows();
    seed(&engine, &rows);

    for row in &rows {
        let case = &row.case;
        let (_, first) = get_invoice_pdf(&router, &row.invoice_id).await;
        let (_, second) = get_invoice_pdf(&router, &row.invoice_id).await;
        // Everything but the addresses, which carry a fresh signature per read: the
        // identity of the invoice and of each document it was issued with.
        let identity = |body: &Value| -> Vec<Option<String>> {
            let pdf = invoice_pdf(body).clone();
            let mut parts = vec![text_member(&pdf, "InvoiceId")];
            for entry in supplemental_documents(&pdf) {
                parts.push(text_member(&entry, "DocumentType"));
                parts.push(text_member(&entry, "DocumentId"));
            }
            parts
        };
        assert_eq!(
            identity(&first),
            identity(&second),
            "case {case:?}: a second read must report the same invoice and documents"
        );
    }

    assert_eq!(
        invoice_count(&engine),
        rows.len(),
        "a read mints and removes no invoice"
    );

    // The registry the listing shares still reports every seeded invoice.
    let listing = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", "Invoicing.ListInvoiceSummaries")
        .header("content-type", "application/x-amz-json-1.0")
        .body(bytes::Bytes::from(
            serde_json::to_vec(
                &json!({"Selector": {"ResourceType": "ACCOUNT_ID", "Value": ACCOUNT}}),
            )
            .expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(listing).await;
    let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let reported = body
        .get("InvoiceSummaries")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        reported,
        rows.len(),
        "after the reads the listing still reports every seeded invoice, got {body}"
    );
}

/// RULE: a document is served at the address a read handed out, and nowhere else — a
/// request that carries no pre-signed authentication is not served a document at all, an
/// address carrying no authenticator this store minted (or one minted for another document)
/// reaches nothing, and an address naming a document this emulator does not hold is
/// answered as the document it does not hold.
#[tokio::test]
async fn a_document_is_served_only_at_the_address_a_read_handed_out() {
    let (router, engine) = emulator();
    let rows = rows();
    seed(&engine, &rows);

    // The path of a document, without the pre-signed query and the authenticator a read's
    // URL carries.
    let unsigned = [
        service_com_amazonaws_invoicing_invoicing::state::InvoiceDocumentTarget::invoice("INV-001")
            .path(),
        service_com_amazonaws_invoicing_invoicing::state::InvoiceDocumentTarget::supplemental(
            "INV-002", 0,
        )
        .path(),
    ];
    for path in unsigned {
        let fetched = fetch(&router, &path).await;
        assert_ne!(
            fetched.media_type, PDF_MEDIA_TYPE,
            "case {path:?}: a request with no pre-signed authentication is served no \
             document, got {} {:?}",
            fetched.status,
            fetched.text()
        );
        assert!(
            !fetched.text().starts_with(PDF_MAGIC),
            "case {path:?}: a request with no pre-signed authentication is served no \
             document, got {:?}",
            fetched.text()
        );
    }

    // A location no document is at: the supplemental documents of `INV-002` are one, so
    // the second position of that invoice names none, no invoice is keyed `INV-404`, this
    // store is not the account named by any other id, and an authenticator a caller writes
    // itself is not one this store minted — including the one minted for another document
    // of the same invoice.
    let (_, body) = get_invoice_pdf(&router, "INV-002").await;
    let pdf = invoice_pdf(&body).clone();
    let supplemental = text_member(&supplemental_documents(&pdf)[0], "DocumentUrl")
        .expect("the read reports a supplemental document URL");
    let invoice = text_member(&pdf, "DocumentUrl").expect("the read reports a document URL");
    let token_field = service_com_amazonaws_invoicing_invoicing::state::DOCUMENT_TOKEN_FIELD;
    let invoice_token = query_field(&invoice, token_field)
        .expect("a reported address carries this store's authenticator");
    let supplemental_token = query_field(&supplemental, token_field)
        .expect("a reported address carries this store's authenticator");
    let other_account = "999999999999";
    let elsewhere = [
        (
            "a supplemental position the invoice does not carry",
            supplemental.replace("/supplemental/0/", "/supplemental/1/"),
        ),
        (
            "an invoice the registry does not hold",
            invoice.replace("/invoice/INV-002", "/invoice/INV-404"),
        ),
        (
            "another account's invoice documents",
            invoice.replace(
                &format!("/{}/", engine::emulator_account_id()),
                &format!("/{other_account}/"),
            ),
        ),
        (
            "an authenticator of all zeroes",
            invoice.replace(&invoice_token, &"0".repeat(invoice_token.len())),
        ),
        (
            "an authenticator with one character changed",
            invoice.replace(&invoice_token, &flip_first(&invoice_token)),
        ),
        (
            "the authenticator minted for another document of the same invoice",
            invoice.replace(&invoice_token, &supplemental_token),
        ),
        (
            "no authenticator at all",
            invoice.replace(&format!("{token_field}={invoice_token}&"), ""),
        ),
    ];
    for (case, url) in elsewhere {
        let fetched = fetch(&router, &url).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: expected nothing at {url:?}, got {} {:?}",
            fetched.status,
            fetched.text()
        );
        assert!(
            !fetched.text().starts_with(PDF_MAGIC),
            "case {case:?}: expected no document at {url:?}, got {:?}",
            fetched.text()
        );
    }
}
