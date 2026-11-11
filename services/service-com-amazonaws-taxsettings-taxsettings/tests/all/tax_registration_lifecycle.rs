// SPDX-License-Identifier: CC0-1.0

//! The tax-registration (TRN) lifecycle, end to end over the restJson1 wire.
//!
//! Every case drives the same `MultiServiceRouter::dispatch` the production
//! router serves, so the assertions are about HTTP status codes and JSON bodies,
//! not about Rust values a handler happened to return.
//!
//! The tables below span the classes the rules generalise over rather than one
//! recorded request: every `TaxRegistrationType` the model lists, every `Sector`,
//! both `HeritageStatus` values, several page sizes against several store sizes,
//! and each published bound of `maxResults` plus one past it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_organizations_awsorganizationsv20161128::state::{
    ComAmazonawsOrganizationsAwsorganizationsv20161128State,
    DEFAULT_FEATURE_SET as ORGANIZATIONS_DEFAULT_FEATURE_SET,
};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    ComAmazonawsTaxsettingsTaxsettingsState, ERROR_CODE_GENERAL_CONFLICT, MAX_STORED_REGISTRATIONS,
    STATE_CONFLICT_MESSAGE, STATUS_PENDING, TAX_REGISTRATIONS_PAGE_BOUNDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::declared_member_defaults::{address_as_stored, country_block_as_stored};

/// One emulator: this service mounted on one shared engine. Each test builds its
/// own, so no test observes another's writes.
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, with the engine it was built on — for a test that has to
/// move a clock-owed transition rather than wait for it.
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = mount_on(&engine);
    (router, engine)
}

/// Mount this service on an engine the caller already holds — the one place a
/// test builds a router, so a test that mounts the service a second time on an
/// engine that already holds state mounts it exactly as the first was.
pub(crate) fn mount_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_taxsettings_taxsettings::EmulatedService::new(engine.clone()),
    );
    router
}

/// Hold every stored TRN's validation open, so a read reports the status its
/// write reported however long the test takes to get there.
pub(crate) fn hold_validations_open(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    for stored in store.registrations.values_mut() {
        stored.validation_due_at = Some(Instant::now() + Duration::from_secs(3_600));
    }
}

/// Make every stored TRN's validation due, the way the clock would have: the
/// next read must report what the validation reached. Never a sleep — the due
/// instant is state.
pub(crate) fn make_validations_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsTaxsettingsTaxsettingsState>();
    let mut store = state.lock();
    for stored in store.registrations.values_mut() {
        stored.validation_due_at = Some(Instant::now());
    }
}

/// Record which account, if any, manages the organization the presented account
/// belongs to — the standing an organization-level write is reserved to. `None` is
/// the world in which that account is in no organization at all.
///
/// The fact is written where its owner keeps it: the organization record of the
/// AWS Organizations control plane on the same engine, which is what a client that
/// creates an organization writes and what this service reads. Organizations makes
/// the account that CREATES an organization its management account and reaches any
/// other arrangement only by invitation (`InviteAccountToOrganization` then
/// `AcceptHandshake`, both still stubs there), so an organization some OTHER
/// account manages is arranged through that service's state module — the very
/// state its own handlers answer from. The management account's other stored
/// fields are derived from its id, so they follow it rather than being invented
/// here, and the organization keeps a member record for it.
///
/// The one place a test establishes that world, so every suite that needs it
/// establishes the same one.
pub(crate) fn record_management_account(
    engine: &Arc<engine::InMemoryEngine>,
    account_id: Option<&str>,
) {
    let state = engine.state::<ComAmazonawsOrganizationsAwsorganizationsv20161128State>();
    let mut store = state.lock();
    let Some(account_id) = account_id else {
        *store = ComAmazonawsOrganizationsAwsorganizationsv20161128State::default();
        return;
    };
    store.create_organization(ORGANIZATIONS_DEFAULT_FEATURE_SET);
    let organization = store
        .organization
        .as_mut()
        .expect("the organization the create recorded");
    let previous = std::mem::replace(&mut organization.master_account_id, account_id.to_string());
    if previous == account_id {
        return;
    }
    organization.master_account_arn = organization.master_account_arn.replace(&previous, account_id);
    organization.master_account_email = organization
        .master_account_email
        .replace(&previous, account_id);
    let mut management = store
        .accounts
        .remove(&previous)
        .expect("the member record of the account that created the organization");
    management.arn = management.arn.replace(&previous, account_id);
    management.email = management.email.replace(&previous, account_id);
    management.id = account_id.to_string();
    store.accounts.insert(account_id.to_string(), management);
    for children in store.children.values_mut() {
        for child in children.iter_mut().filter(|child| child.id == previous) {
            child.id = account_id.to_string();
        }
    }
}

/// What one TaxSettings call answered: the HTTP status, the restJson1 `__type`
/// error code (empty on success) and the parsed JSON body.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_code: String,
    pub(crate) body: Value,
}

impl Answer {
    /// A body member, or `Value::Null` when the response omitted it.
    pub(crate) fn get(&self, member: &str) -> &Value {
        self.body.get(member).unwrap_or(&Value::Null)
    }
}

/// The `name`s a refusal's `fieldList` carries — the parameters the refusal is
/// about. One reader for every suite that asserts on a refused parameter, so
/// they cannot read the member two different ways.
pub(crate) fn field_list(answer: &Answer) -> Vec<String> {
    answer
        .get("fieldList")
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.get("name").and_then(Value::as_str))
                .map(std::string::ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Call one operation. The SigV4 credential scope names `tax`, this service's
/// signing name, which is how the router picks the service that claims it.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://tax.us-east-1.amazonaws.com/{operation}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/tax/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0",
        )
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the test body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let body: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    let error_code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Answer {
        status,
        error_code,
        body,
    }
}

/// A `TaxRegistrationEntry` to store, and the account to store it under.
struct Case {
    name: &'static str,
    account_id: &'static str,
    entry: Value,
}

/// The entries the round-trip table stores.
///
/// `registrationType` covers every value of the model's `TaxRegistrationType`
/// enum — `VAT | GST | CPF | CNPJ | SST | TIN | NRIC | PAN | NIP`
/// (`smithy_query(query="shape", shape="TaxRegistrationType")`) — except `PAN`,
/// whose legal address AWS derives from the tax number so that an entry may not
/// submit one; `tests/all/fidelity_rules.rs` covers every type against that rule.
/// `sector` covers every value of `Sector` (`Business | Individual |
/// Government`).
fn round_trip_cases() -> Vec<Case> {
    vec![
        Case {
            name: "minimal VAT registration",
            account_id: "000000000001",
            entry: json!({
                "registrationId": "DE136695976",
                "registrationType": "VAT",
                "legalName": "Acme GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            }),
        },
        Case {
            name: "GST registration with sector and certified email",
            account_id: "000000000002",
            entry: json!({
                "registrationId": "29ABCDE1234F1Z5",
                "registrationType": "GST",
                "legalName": "Acme India Private Limited",
                "sector": "Business",
                "certifiedEmailId": "billing.team+trn@example.com",
                "legalAddress": {
                    "addressLine1": "1 Residency Road",
                    "city": "Bengaluru",
                    "stateOrRegion": "Karnataka",
                    "postalCode": "560025",
                    "countryCode": "IN",
                },
            }),
        },
        Case {
            name: "CNPJ registration for a government sector buyer",
            account_id: "000000000003",
            entry: json!({
                "registrationId": "12.345.678/0001-95",
                "registrationType": "CNPJ",
                "legalName": "Municipio de Exemplo",
                "sector": "Government",
                "legalAddress": {
                    "districtOrCounty": "Centro",
                    "city": "Sao Paulo",
                    "stateOrRegion": "SP",
                    "postalCode": "01001-000",
                    "countryCode": "BR",
                },
            }),
        },
        Case {
            name: "CPF registration with a date of birth",
            account_id: "000000000004",
            entry: json!({
                "registrationId": "123.456.789-09",
                "registrationType": "CPF",
                "legalName": "Maria Exemplo",
                "sector": "Individual",
                "legalAddress": {
                    "stateOrRegion": "Rio de Janeiro",
                    "postalCode": "22041-001",
                    "countryCode": "BR",
                },
                "verificationDetails": { "dateOfBirth": "1980-07-15" },
            }),
        },
        Case {
            name: "NRIC registration with Malaysian additional information",
            account_id: "000000000005",
            entry: json!({
                "registrationId": "880101011234",
                "registrationType": "NRIC",
                "legalName": "Siti Example",
                "sector": "Individual",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": { "taxInformationNumber": "C1234567890" },
                },
            }),
        },
        Case {
            name: "SST registration carrying two verification documents",
            account_id: "000000000006",
            entry: json!({
                "registrationId": "W10-1808-32000055",
                "registrationType": "SST",
                "legalName": "Example Services Sdn Bhd",
                "sector": "Business",
                "legalAddress": { "postalCode": "50450", "countryCode": "MY" },
                // Malaysia's documented rule for a business reseller with
                // service codes: businessRegistrationNumber,
                // taxInformationNumber and distinct serviceTaxCodes
                // (`crate::state::COUNTRY_RULES`).
                "additionalTaxInformation": {
                    "malaysiaAdditionalInfo": {
                        "serviceTaxCodes": ["IT Services"],
                        "businessRegistrationNumber": "202001001234",
                        "taxInformationNumber": "C1234567890",
                    },
                },
                "verificationDetails": {
                    "taxRegistrationDocuments": [
                        { "s3Location": { "bucket": "trn-docs", "key": "sst-certificate.pdf" } },
                        { "file": { "fileName": "director-id.png", "fileContent": "aGVsbG8=" } },
                    ],
                },
            }),
        },
        Case {
            name: "TIN registration with Spanish additional information",
            account_id: "000000000007",
            entry: json!({
                "registrationId": "ESA12345674",
                "registrationType": "TIN",
                "legalName": "Ejemplo SA",
                "sector": "Business",
                "legalAddress": {
                    "city": "Madrid",
                    "postalCode": "28001",
                    "countryCode": "ES",
                },
                "additionalTaxInformation": {
                    "spainAdditionalInfo": { "registrationType": "Intra-EU" },
                },
            }),
        },
        Case {
            name: "NIP registration with Polish additional information",
            account_id: "000000000009",
            entry: json!({
                "registrationId": "1234563218",
                "registrationType": "NIP",
                "legalName": "Przyklad Sp. z o.o.",
                "sector": "Business",
                "legalAddress": { "postalCode": "00-001", "countryCode": "PL" },
                "additionalTaxInformation": {
                    "polandAdditionalInfo": {
                        "isGroupVatEnabled": true,
                        "taxRegistrationNumberType": "LocalRegistrationNumber",
                    },
                },
            }),
        },
    ]
}

/// The members a submitted entry and the reported registration share verbatim.
///
/// `legalAddress` is NOT among them: two of its members declare a default the
/// write materialises, so it is asserted below through
/// `crate::declared_member_defaults::address_as_stored`.
const ECHOED_MEMBERS: &[&str] = &[
    "registrationId",
    "registrationType",
    "legalName",
    "sector",
    "certifiedEmailId",
];

/// RULE: a stored tax registration is reported, member for member, as it was
/// submitted, with the status `PutTaxRegistration` returned — for every entry
/// the model's shapes admit, not just the cited one.
///
/// Every TRN's validation is held open here, so the status a read reports is the
/// one the write reported; `tests/all/fidelity_rules.rs` drives the transition
/// the clock owes once that validation falls due.
#[tokio::test]
async fn a_stored_registration_is_reported_exactly_as_submitted() {
    let (router, engine) = emulator_with_engine();
    for case in round_trip_cases() {
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": case.account_id, "taxRegistrationEntry": case.entry }),
        )
        .await;
        assert_eq!(
            put.status, 200,
            "case {:?}: PutTaxRegistration expected 200, got {} {:?}",
            case.name, put.status, put.body
        );
        hold_validations_open(&engine);
        let put_status = put.get("status").as_str().unwrap_or_default().to_string();
        assert_eq!(
            put_status, STATUS_PENDING,
            "case {:?}: PutTaxRegistration must report the status it stored, got {:?}",
            case.name, put.body
        );

        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": case.account_id }),
        )
        .await;
        assert_eq!(
            got.status, 200,
            "case {:?}: GetTaxRegistration expected 200, got {} {:?}",
            case.name, got.status, got.body
        );
        let registration = got.get("taxRegistration").clone();
        assert_eq!(
            registration.get("status").and_then(Value::as_str),
            Some(put_status.as_str()),
            "case {:?}: expected the status PutTaxRegistration returned ({put_status}), got {:?}",
            case.name,
            registration
        );
        for member in ECHOED_MEMBERS {
            let submitted = case.entry.get(member);
            if submitted.is_none() {
                continue;
            }
            assert_eq!(
                registration.get(member),
                submitted,
                "case {:?}: expected {member} to round-trip as {:?}, got {:?}",
                case.name,
                submitted,
                registration.get(member)
            );
        }
        // `legalAddress` round-trips with the defaults the model declares for
        // its members materialised — the members the entry submitted, plus
        // `addressLine1` and `city` where it omitted them.
        if let Some(submitted) = case.entry.get("legalAddress") {
            assert_eq!(
                registration.get("legalAddress"),
                Some(&address_as_stored(submitted)),
                "case {:?}: expected legalAddress to round-trip as {:?}, got {:?}",
                case.name,
                address_as_stored(submitted),
                registration.get("legalAddress")
            );
        }
        // `additionalTaxInformation` is submitted as `AdditionalInfoRequest` and
        // reported as `AdditionalInfoResponse`: the country blocks a client can
        // submit must survive the projection unchanged, but for the declared
        // default one of their members carries.
        if let Some(submitted) = case.entry.get("additionalTaxInformation") {
            let reported = registration
                .get("additionalTaxInformation")
                .cloned()
                .unwrap_or(Value::Null);
            for (country, block) in submitted.as_object().expect("an object") {
                assert_eq!(
                    reported.get(country),
                    Some(&country_block_as_stored(country, block)),
                    "case {:?}: expected additionalTaxInformation.{country} to round-trip as \
                     {block:?}, got {:?}",
                    case.name,
                    reported.get(country)
                );
            }
        }
        // Each submitted document becomes one `TaxDocumentMetadata`, named by its
        // file name or S3 key, with an access token that locates it.
        let submitted_documents: Vec<String> = case
            .entry
            .pointer("/verificationDetails/taxRegistrationDocuments")
            .and_then(Value::as_array)
            .map(|documents| {
                documents
                    .iter()
                    .map(|document| {
                        document
                            .pointer("/file/fileName")
                            .or_else(|| document.pointer("/s3Location/key"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        let reported_documents = registration
            .get("taxDocumentMetadatas")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported_documents.len(),
            submitted_documents.len(),
            "case {:?}: expected {} document metadata entries, got {reported_documents:?}",
            case.name,
            submitted_documents.len()
        );
        for (expected, reported) in submitted_documents.iter().zip(&reported_documents) {
            assert_eq!(
                reported.get("taxDocumentName").and_then(Value::as_str),
                Some(expected.as_str()),
                "case {:?}: expected document named {expected:?}, got {reported:?}",
                case.name
            );
            assert!(
                reported
                    .get("taxDocumentAccessToken")
                    .and_then(Value::as_str)
                    .is_some_and(|token| !token.is_empty()),
                "case {:?}: expected an access token for {expected:?}, got {reported:?}",
                case.name
            );
        }
    }
}

/// RULE: a registration reported by `ListTaxRegistrations` carries the same TRN
/// as the one `GetTaxRegistration` reports, with the legal address reduced to its
/// jurisdiction (country code, and state or region where the address has one).
#[tokio::test]
async fn list_reports_the_same_registration_reduced_to_its_jurisdiction() {
    let router = emulator();
    let cases = round_trip_cases();
    for case in &cases {
        let put = call(
            &router,
            "PutTaxRegistration",
            &json!({ "accountId": case.account_id, "taxRegistrationEntry": case.entry }),
        )
        .await;
        assert_eq!(put.status, 200, "case {:?}: setup put failed: {:?}", case.name, put.body);
    }

    let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
    assert_eq!(
        listed.status, 200,
        "ListTaxRegistrations expected 200, got {} {:?}",
        listed.status, listed.body
    );
    let details = listed
        .get("accountDetails")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        details.len(),
        cases.len(),
        "expected every stored registration to be enumerated ({}), got {}",
        cases.len(),
        details.len()
    );

    for case in &cases {
        let entry = details
            .iter()
            .find(|detail| detail.get("accountId").and_then(Value::as_str) == Some(case.account_id))
            .unwrap_or_else(|| panic!("case {:?}: account missing from the listing", case.name));
        let listed_registration = entry.get("taxRegistration").cloned().unwrap_or(Value::Null);
        for member in ["registrationId", "registrationType", "legalName", "sector"] {
            if let Some(submitted) = case.entry.get(member) {
                assert_eq!(
                    listed_registration.get(member),
                    Some(submitted),
                    "case {:?}: expected listed {member} to be {submitted:?}, got {:?}",
                    case.name,
                    listed_registration.get(member)
                );
            }
        }
        let address = case
            .entry
            .get("legalAddress")
            .expect("every case submits a legal address");
        assert_eq!(
            listed_registration.pointer("/jurisdiction/countryCode"),
            address.get("countryCode"),
            "case {:?}: expected the jurisdiction country code to be {:?}, got {:?}",
            case.name,
            address.get("countryCode"),
            listed_registration.pointer("/jurisdiction/countryCode")
        );
        assert_eq!(
            listed_registration.pointer("/jurisdiction/stateOrRegion"),
            address.get("stateOrRegion"),
            "case {:?}: expected the jurisdiction state or region to be {:?}, got {:?}",
            case.name,
            address.get("stateOrRegion"),
            listed_registration.pointer("/jurisdiction/stateOrRegion")
        );
        assert!(
            listed_registration.get("legalAddress").is_none(),
            "case {:?}: the listing must not carry the full legal address, got {listed_registration:?}",
            case.name
        );
    }
}

/// Store `count` registrations and return their account ids in order.
///
/// Each record is held in a jurisdiction this service publishes no tax
/// registration number structure for, so the synthetic per-account number below
/// is one no published structure judges and the write admits it
/// (`state::check_registration_number`); what this seeding is about is how many
/// records the store holds, not what any one number is.
async fn seed(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 1..=count {
        let account_id = format!("{:012}", index);
        let put = call(
            router,
            "PutTaxRegistration",
            &json!({
                "accountId": account_id,
                "taxRegistrationEntry": {
                    "registrationId": format!("VAT-{index}"),
                    "registrationType": "VAT",
                    "legalName": format!("Account {index} Ltd"),
                    "legalAddress": { "postalCode": "1011AB", "countryCode": "NL" },
                },
            }),
        )
        .await;
        assert_eq!(put.status, 200, "seeding {account_id} failed: {:?}", put.body);
        ids.push(account_id);
    }
    ids
}

/// RULE: paging a listing emits every stored item exactly once, in the store's
/// order, in pages of at most the SERVER-decided page size, and issues a resume
/// cursor on every page but the last.
///
/// The server-decided size is `maxResults` when the request names one (its
/// modelled range is 1..=1000, so 1 and 1000 are its published bounds), and this
/// operation's own documented default when it does not — never the whole store,
/// and never more than its documented ceiling
/// ([`TAX_REGISTRATIONS_PAGE_BOUNDS`]).
#[tokio::test]
async fn paging_emits_every_registration_exactly_once_in_order() {
    // (stored registrations, requested page size).
    let shapes: &[(usize, Option<i64>)] = &[
        (0, None),
        (0, Some(1)),
        (1, Some(1)),
        (5, Some(1)),
        (5, Some(2)),
        (5, Some(4)),
        (5, Some(5)),
        (5, Some(6)),
        (5, Some(1000)),
        (7, Some(3)),
        (7, None),
        // More records than the default page size, so the bound on a request
        // that names none is observable: it must still page rather than return
        // everything at once.
        (TAX_REGISTRATIONS_PAGE_BOUNDS.default_size + 50, None),
        (TAX_REGISTRATIONS_PAGE_BOUNDS.default_size + 50, Some(1000)),
    ];
    for (count, page_size) in shapes.iter().copied() {
        let case = format!("{count} registrations, page size {page_size:?}");
        let router = emulator();
        let expected = seed(&router, count).await;

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        // Whatever the request asks for, the page the server builds is bounded:
        // by `maxResults` when it named one, by the default otherwise, and by
        // the ceiling in both cases.
        let bound = TAX_REGISTRATIONS_PAGE_BOUNDS
            .page_size(page_size.and_then(|size| i32::try_from(size).ok()));
        loop {
            let mut request = serde_json::Map::new();
            if let Some(size) = page_size {
                request.insert("maxResults".to_string(), json!(size));
            }
            if let Some(ref cursor) = token {
                request.insert("nextToken".to_string(), json!(cursor));
            }
            let page = call(&router, "ListTaxRegistrations", &Value::Object(request)).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: page {pages} expected 200, got {} {:?}",
                page.status, page.body
            );
            let details = page
                .get("accountDetails")
                .as_array()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: accountDetails is required, got {:?}", page.body));
            // Whatever the request asked for, the page the server builds is
            // bounded: by `maxResults` when it named one, by the default
            // otherwise, and by the ceiling in both cases.
            assert!(
                details.len() <= bound,
                "case {case:?}: page {pages} returned {} items, more than the {bound} the server allows",
                details.len()
            );
            for detail in &details {
                seen.push(
                    detail
                        .get("accountId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            token = page
                .get("nextToken")
                .as_str()
                .map(std::string::ToString::to_string);
            if token.is_none() {
                assert_eq!(
                    seen.len(),
                    expected.len(),
                    "case {case:?}: the last page dropped the cursor with {} of {} items emitted",
                    seen.len(),
                    expected.len()
                );
                break;
            }
            assert!(
                pages <= count + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: expected {expected:?} in order, got {seen:?}"
        );
        assert_eq!(
            seen.iter().collect::<BTreeSet<_>>().len(),
            seen.len(),
            "case {case:?}: paging emitted a duplicate: {seen:?}"
        );
        // The page size is what decides how many pages the walk takes, so a
        // store larger than the bound is necessarily walked in several pages.
        let expected_pages = expected.len().div_ceil(bound).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages of at most {bound}, took {pages}"
        );
    }
}

/// RULE: deleting a registration removes it — the account's reads answer
/// `ResourceNotFoundException` (404) and the listing no longer carries it — and
/// deleting an account that has none is itself a 404.
#[tokio::test]
async fn deleting_a_registration_removes_it_from_every_read() {
    let router = emulator();
    let seeded = seed(&router, 4).await;

    for account_id in &seeded {
        let deleted = call(
            &router,
            "DeleteTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {account_id:?}: DeleteTaxRegistration expected 200, got {} {:?}",
            deleted.status, deleted.body
        );

        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(
            (got.status, got.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {account_id:?}: expected 404 ResourceNotFoundException after delete, got {:?}",
            got
        );

        let again = call(
            &router,
            "DeleteTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(
            (again.status, again.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {account_id:?}: expected 404 ResourceNotFoundException deleting it twice, got {:?}",
            again
        );

        let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
        let remaining: Vec<&str> = listed
            .get("accountDetails")
            .as_array()
            .map(|details| {
                details
                    .iter()
                    .filter_map(|detail| detail.get("accountId").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !remaining.contains(&account_id.as_str()),
            "case {account_id:?}: still listed after delete: {remaining:?}"
        );
    }
}

/// RULE: a reporting call must not change what a later caller sees, and writing a
/// registration again replaces the account's record rather than adding one.
#[tokio::test]
async fn reads_are_repeatable_and_a_rewrite_replaces_the_record() {
    let (router, engine) = emulator_with_engine();
    let account_id = "000000000042";
    let first = json!({
        "registrationId": "DE111111117",
        "registrationType": "VAT",
        "legalName": "Acme GmbH",
        "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
    });
    let second = json!({
        "registrationId": "FR22222222222",
        "registrationType": "TIN",
        "legalName": "Acme SARL",
        "sector": "Business",
        "legalAddress": { "postalCode": "75001", "countryCode": "FR" },
        // France's documented rule: the sirenNumber must be specified
        // (`crate::state::COUNTRY_RULES`).
        "additionalTaxInformation": { "franceAdditionalInfo": { "sirenNumber": "123456789" } },
    });

    let put = call(
        &router,
        "PutTaxRegistration",
        &json!({ "accountId": account_id, "taxRegistrationEntry": first }),
    )
    .await;
    assert_eq!(put.status, 200, "setup put failed: {:?}", put.body);
    // A read settles the TRN validation the clock has made due, which is a
    // change the clock owed, not one the read made: hold it open so that what is
    // left to observe is only what the reads themselves do.
    hold_validations_open(&engine);

    // Three identical reads, plus a listing between them: every read must answer
    // the same thing.
    let mut reads = Vec::new();
    for _ in 0..3 {
        let got = call(
            &router,
            "GetTaxRegistration",
            &json!({ "accountId": account_id }),
        )
        .await;
        assert_eq!(got.status, 200, "repeat read failed: {:?}", got.body);
        reads.push(got.get("taxRegistration").clone());
        let listed = call(&router, "ListTaxRegistrations", &json!({ "maxResults": 1 })).await;
        assert_eq!(listed.status, 200, "repeat listing failed: {:?}", listed.body);
    }
    assert!(
        reads.windows(2).all(|pair| pair[0] == pair[1]),
        "case {account_id:?}: a read changed what the next read saw: {reads:?}"
    );

    // The first write's validation falls due, the way the clock would have made
    // it: a TRN that is still outstanding may not be rewritten at all (that rule
    // is `tests/all/pending_trn_conflict.rs`), so what this case is about — that a
    // rewrite REPLACES the record rather than adding one — is observable only
    // once the record it replaces has settled.
    make_validations_due(&engine);

    let rewrite = call(
        &router,
        "PutTaxRegistration",
        &json!({ "accountId": account_id, "taxRegistrationEntry": second }),
    )
    .await;
    assert_eq!(rewrite.status, 200, "rewrite failed: {:?}", rewrite.body);
    hold_validations_open(&engine);
    let got = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": account_id }),
    )
    .await;
    let registration = got.get("taxRegistration").clone();
    assert_eq!(
        registration.get("registrationId").and_then(Value::as_str),
        Some("FR22222222222"),
        "case {account_id:?}: expected the rewritten TRN, got {registration:?}"
    );
    let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
    let count = listed
        .get("accountDetails")
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        count, 1,
        "case {account_id:?}: a rewrite must replace the record, not add one: {:?}",
        listed.body
    );
}

/// RULE: an `accountId` a request omits is the caller's own account.
#[tokio::test]
async fn an_omitted_account_id_is_the_callers_own_account() {
    let router = emulator();
    let caller = engine::emulator_account_id();
    let put = call(
        &router,
        "PutTaxRegistration",
        &json!({
            "taxRegistrationEntry": {
                "registrationId": "DE999999995",
                "registrationType": "VAT",
                "legalName": "Caller GmbH",
                "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
            },
        }),
    )
    .await;
    assert_eq!(put.status, 200, "PutTaxRegistration failed: {:?}", put.body);

    let listed = call(&router, "ListTaxRegistrations", &json!({})).await;
    let ids: Vec<&str> = listed
        .get("accountDetails")
        .as_array()
        .map(|details| {
            details
                .iter()
                .filter_map(|detail| detail.get("accountId").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![caller],
        "expected the registration to be stored under the caller's account {caller}, got {ids:?}"
    );

    // Reached with no `accountId` at all, and under the caller's account id: the
    // same record either way.
    let implicit = call(&router, "GetTaxRegistration", &json!({})).await;
    let explicit = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": caller }),
    )
    .await;
    assert_eq!(implicit.status, 200, "implicit read failed: {:?}", implicit.body);
    assert_eq!(
        implicit.get("taxRegistration"),
        explicit.get("taxRegistration"),
        "expected the implicit and explicit account to name one record, got {:?} and {:?}",
        implicit.body,
        explicit.body
    );
    let deleted = call(&router, "DeleteTaxRegistration", &json!({})).await;
    assert_eq!(deleted.status, 200, "implicit delete failed: {:?}", deleted.body);
    let after = call(&router, "GetTaxRegistration", &json!({})).await;
    assert_eq!(
        (after.status, after.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "expected the implicit delete to remove the caller's record, got {after:?}"
    );
}

// The tax-inheritance election — what a write records and what
// `GetTaxInheritance` then reports — is covered by
// `tests/all/tax_inheritance_election.rs`; the default it reports while none is
// recorded, by `tests/all/fidelity_rules.rs`.

/// One refused (or accepted) request: the operation, the body, and the answer it
/// must get.
///
/// `modelled_error_code` is set only where the refusal comes from the handler and
/// so carries this service's own `ValidationException`, whose `errorCode` member
/// is `@required`. A refusal the boundary validator raises from a modelled
/// constraint is rendered from the framework's own shape, which carries only a
/// message, so those cases leave it empty.
struct Refusal {
    name: &'static str,
    operation: &'static str,
    body: Value,
    status: u16,
    error_code: &'static str,
    modelled_error_code: &'static str,
}

/// A `TaxRegistrationEntry` that satisfies every constraint, for cases that vary
/// exactly one member away from valid.
fn valid_entry() -> Value {
    json!({
        "registrationId": "DE136695976",
        "registrationType": "VAT",
        "legalName": "Acme GmbH",
        "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
    })
}

/// The same, held in a jurisdiction this service publishes no tax registration
/// number structure for, so no value this table puts in `registrationId` is
/// judged as a number of a published kind.
fn entry_in_an_unjudged_jurisdiction() -> Value {
    json!({
        "registrationId": "VAT-0000000001",
        "registrationType": "VAT",
        "legalName": "Voorbeeld BV",
        "legalAddress": { "postalCode": "1011AB", "countryCode": "NL" },
    })
}

/// RULE: a request whose value lies outside what the model admits for that
/// member is refused with HTTP 400 `ValidationException`; one inside it is
/// served.
#[tokio::test]
async fn values_outside_the_model_are_refused_and_values_inside_it_are_served() {
    let mut cases = vec![
        // `accountId`: length 12..12, pattern ^\d+$.
        Refusal {
            name: "accountId that is not digits",
            operation: "GetTaxRegistration",
            body: json!({ "accountId": "abc" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "",
        },
        Refusal {
            name: "accountId of 12 characters with a non-digit",
            operation: "GetTaxRegistration",
            body: json!({ "accountId": "00000000000a" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "",
        },
        Refusal {
            name: "accountId one digit short",
            operation: "GetTaxRegistration",
            body: json!({ "accountId": "00000000001" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "",
        },
        Refusal {
            name: "accountId one digit long",
            operation: "GetTaxRegistration",
            body: json!({ "accountId": "0000000000001" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "",
        },
        // Required members.
        Refusal {
            name: "taxRegistrationEntry omitted",
            operation: "PutTaxRegistration",
            body: json!({ "accountId": "000000000001" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "",
        },
        Refusal {
            name: "registrationId omitted",
            operation: "PutTaxRegistration",
            body: json!({
                "accountId": "000000000001",
                "taxRegistrationEntry": {
                    "registrationType": "VAT",
                    "legalName": "Acme GmbH",
                    "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
                },
            }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "",
        },
        Refusal {
            name: "registrationType omitted",
            operation: "PutTaxRegistration",
            body: json!({
                "accountId": "000000000001",
                "taxRegistrationEntry": {
                    "registrationId": "DE136695976",
                    "legalName": "Acme GmbH",
                    "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
                },
            }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "",
        },
        // `legalName` / `legalAddress` are omittable only where the service fills
        // the member in from the tax registration number (Brazil, Mexico, and the
        // types whose address AWS derives); a German VAT entry is in neither
        // carve-out, and an entry carrying no address at all identifies no
        // jurisdiction to read one from — see
        // `tests/all/derived_registration_members.rs`.
        Refusal {
            name: "legalName omitted",
            operation: "PutTaxRegistration",
            body: json!({
                "accountId": "000000000001",
                "taxRegistrationEntry": {
                    "registrationId": "DE136695976",
                    "registrationType": "VAT",
                    "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
                },
            }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "MissingInput",
        },
        Refusal {
            name: "legalAddress omitted",
            operation: "PutTaxRegistration",
            body: json!({
                "accountId": "000000000001",
                "taxRegistrationEntry": {
                    "registrationId": "DE136695976",
                    "registrationType": "VAT",
                    "legalName": "Acme GmbH",
                },
            }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "MissingInput",
        },
        // `nextToken`: length 1..2000, pattern ^[-A-Za-z0-9_+\=\/]+$, and the
        // cursor must be one this service issued.
        Refusal {
            name: "nextToken outside the modelled pattern",
            operation: "ListTaxRegistrations",
            body: json!({ "nextToken": "not a token!" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "",
        },
        Refusal {
            name: "nextToken that is not decodable",
            operation: "ListTaxRegistrations",
            body: json!({ "nextToken": "A" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "MalformedToken",
        },
        Refusal {
            name: "nextToken naming something that is not an account",
            operation: "ListTaxRegistrations",
            body: json!({ "nextToken": "QUJD" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "InvalidToken",
        },
        // `heritageStatus` is required to record an election, and closed. The
        // value set of this top-level enum member is carried into the input
        // schema, so the boundary validator refuses an unlisted value before the
        // handler's own enum check sees it.
        Refusal {
            name: "heritageStatus omitted",
            operation: "PutTaxInheritance",
            body: json!({}),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "MissingInput",
        },
        Refusal {
            name: "heritageStatus outside the HeritageStatus enum",
            operation: "PutTaxInheritance",
            body: json!({ "heritageStatus": "Maybe" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "",
        },
        // Both `HeritageStatus` values the model lists are inside the member's
        // value set, so both clear the boundary validator — and are then refused
        // by the standing recording an election is reserved to, because this
        // emulator presents a standalone account that is nobody's organization
        // management account. See `tests/all/tax_inheritance_election.rs` for
        // what each value records in the world where the caller IS that account.
        Refusal {
            name: "heritageStatus OptIn",
            operation: "PutTaxInheritance",
            body: json!({ "heritageStatus": "OptIn" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "FieldValidationFailed",
        },
        Refusal {
            name: "heritageStatus OptOut",
            operation: "PutTaxInheritance",
            body: json!({ "heritageStatus": "OptOut" }),
            status: 400,
            error_code: "ValidationException",
            modelled_error_code: "FieldValidationFailed",
        },
    ];

    // `registrationId` / `legalName`: length 1..200, pattern ^(?!\s*$)[\s\S]+$.
    // The length bounds are refused by the boundary validator; the non-blank
    // pattern is a negative lookahead its regex engine cannot compile, so the
    // handler refuses a blank value and names the member.
    for (name, member, value, status, modelled_error_code) in [
        (
            "blank registrationId",
            "registrationId",
            json!("   "),
            400,
            "FieldValidationFailed",
        ),
        ("empty registrationId", "registrationId", json!(""), 400, ""),
        (
            "registrationId at the 200-character bound",
            "registrationId",
            json!("D".repeat(200)),
            200,
            "",
        ),
        (
            "registrationId one past the 200-character bound",
            "registrationId",
            json!("D".repeat(201)),
            400,
            "",
        ),
        (
            "blank legalName",
            "legalName",
            json!("\t\n "),
            400,
            "FieldValidationFailed",
        ),
        (
            "legalName at the 200-character bound",
            "legalName",
            json!("A".repeat(200)),
            200,
            "",
        ),
        (
            "legalName one past the 200-character bound",
            "legalName",
            json!("A".repeat(201)),
            400,
            "",
        ),
        (
            "certifiedEmailId that is not an email address",
            "certifiedEmailId",
            json!("not-an-email"),
            400,
            "",
        ),
        (
            "certifiedEmailId that is an email address",
            "certifiedEmailId",
            json!("vat.invoices@example.co.uk"),
            200,
            "",
        ),
        // `registrationType` / `sector` target closed enum shapes; the boundary
        // validator carries their value sets.
        (
            "registrationType outside the TaxRegistrationType enum",
            "registrationType",
            json!("VATT"),
            400,
            "",
        ),
        (
            "sector outside the Sector enum",
            "sector",
            json!("Charity"),
            400,
            "",
        ),
        (
            "sector inside the Sector enum",
            "sector",
            json!("Government"),
            200,
            "",
        ),
        // The same non-blank pattern constrains every line of the legal address.
        (
            "blank postalCode in the legal address",
            "legalAddress",
            json!({ "postalCode": " ", "countryCode": "DE" }),
            400,
            "FieldValidationFailed",
        ),
        (
            "blank city in the legal address",
            "legalAddress",
            json!({ "city": "  ", "postalCode": "10115", "countryCode": "DE" }),
            400,
            "FieldValidationFailed",
        ),
        (
            "blank stateOrRegion in the legal address",
            "legalAddress",
            json!({ "stateOrRegion": "\t", "postalCode": "10115", "countryCode": "DE" }),
            400,
            "FieldValidationFailed",
        ),
        (
            "legal address whose country code is not two letters",
            "legalAddress",
            json!({ "postalCode": "10115", "countryCode": "DEU" }),
            400,
            "",
        ),
        (
            "legal address with every line this jurisdiction accepts filled",
            "legalAddress",
            json!({
                "addressLine1": "1 Example Street",
                "addressLine2": "Floor 3",
                "districtOrCounty": "Mitte",
                "city": "Berlin",
                "stateOrRegion": "Berlin",
                "postalCode": "10115",
                "countryCode": "DE",
            }),
            200,
            "",
        ),
    ] {
        // A case that varies the `registrationId` is judged by the model's own
        // constraints on that member and by nothing else, so it is submitted in a
        // jurisdiction this service publishes no number structure for: a
        // 200-character value is inside the member's 1..200 bound and is no
        // German VAT number (`state::check_registration_number`, pinned in
        // `tests/all/trn_validation_outcome.rs`).
        let mut entry = if member == "registrationId" {
            entry_in_an_unjudged_jurisdiction()
        } else {
            valid_entry()
        };
        entry
            .as_object_mut()
            .expect("an object")
            .insert(member.to_string(), value);
        cases.push(Refusal {
            name,
            operation: "PutTaxRegistration",
            body: json!({ "accountId": "000000000001", "taxRegistrationEntry": entry }),
            status,
            error_code: if status == 200 {
                ""
            } else {
                "ValidationException"
            },
            modelled_error_code,
        });
    }

    // `maxResults`: range 1..=1000 — each published bound, and one past each.
    for (name, max_results, status) in [
        ("maxResults below the range", json!(0), 400),
        ("maxResults at the low bound", json!(1), 200),
        ("maxResults at the high bound", json!(1000), 200),
        ("maxResults above the range", json!(1001), 400),
        ("negative maxResults", json!(-1), 400),
    ] {
        cases.push(Refusal {
            name,
            operation: "ListTaxRegistrations",
            body: json!({ "maxResults": max_results }),
            status,
            error_code: if status == 200 {
                ""
            } else {
                "ValidationException"
            },
            modelled_error_code: "",
        });
    }

    for case in cases {
        let router = emulator();
        let answer = call(&router, case.operation, &case.body).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected HTTP {} from {}, got {} {:?}",
            case.name, case.status, case.operation, answer.status, answer.body
        );
        if !case.error_code.is_empty() {
            assert_eq!(
                answer.error_code, case.error_code,
                "case {:?}: expected error code {:?}, got {:?}",
                case.name, case.error_code, answer.error_code
            );
        }
        if !case.modelled_error_code.is_empty() {
            assert_eq!(
                answer.get("errorCode").as_str(),
                Some(case.modelled_error_code),
                "case {:?}: expected the modelled errorCode {:?}, got {:?}",
                case.name,
                case.modelled_error_code,
                answer.body
            );
        }
    }
}

/// RULE: a read of an account that has no stored registration is a 404
/// `ResourceNotFoundException`, never an empty 200.
#[tokio::test]
async fn reading_an_account_with_no_registration_is_a_404() {
    let router = emulator();
    seed(&router, 1).await;
    for account_id in ["000000000002", "999999999999", engine::emulator_account_id()] {
        for operation in ["GetTaxRegistration", "DeleteTaxRegistration"] {
            let answer = call(&router, operation, &json!({ "accountId": account_id })).await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {account_id:?}/{operation}: expected 404 ResourceNotFoundException, got {answer:?}"
            );
        }
    }
}

/// RULE: the store the emulator keeps resident is bounded — a write that would
/// ADD a record past the bound is refused with the modelled `ConflictException`
/// (409), while rewriting an account that already has one, and writing again
/// after a delete has released a slot, both still succeed.
#[tokio::test]
async fn the_resident_registration_store_is_bounded() {
    let (router, engine) = emulator_with_engine();
    let seeded = seed(&router, MAX_STORED_REGISTRATIONS).await;

    let entry = json!({
        "registrationId": "DE136695976",
        "registrationType": "VAT",
        "legalName": "One Too Many GmbH",
        "legalAddress": { "postalCode": "10115", "countryCode": "DE" },
    });
    let overflow_account = format!("{:012}", MAX_STORED_REGISTRATIONS + 1);
    let refused = call(
        &router,
        "PutTaxRegistration",
        &json!({ "accountId": overflow_account, "taxRegistrationEntry": entry }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (409, "ConflictException"),
        "case {overflow_account:?}: expected 409 ConflictException past the \
         {MAX_STORED_REGISTRATIONS}-record bound, got {refused:?}"
    );
    // The refusal body carries the envelope the conflict class publishes: the
    // service's own code for a state conflict and that class's fixed message.
    assert_eq!(
        (
            refused.get("errorCode").as_str(),
            refused.get("message").as_str(),
        ),
        (
            Some(ERROR_CODE_GENERAL_CONFLICT),
            Some(STATE_CONFLICT_MESSAGE),
        ),
        "case {overflow_account:?}: expected the conflict class's own envelope, got {:?}",
        refused.body
    );
    let not_stored = call(
        &router,
        "GetTaxRegistration",
        &json!({ "accountId": overflow_account }),
    )
    .await;
    assert_eq!(
        (not_stored.status, not_stored.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case {overflow_account:?}: a refused write must store nothing, got {not_stored:?}"
    );

    // A rewrite adds no record, so the bound does not refuse it — once the
    // record it replaces is no longer an outstanding TRN, which the clock settles
    // and `tests/all/pending_trn_conflict.rs` covers.
    make_validations_due(&engine);
    let rewritten = call(
        &router,
        "PutTaxRegistration",
        &json!({ "accountId": seeded[0], "taxRegistrationEntry": entry }),
    )
    .await;
    assert_eq!(
        rewritten.status, 200,
        "case {:?}: a rewrite adds no record and must be served, got {rewritten:?}",
        seeded[0]
    );

    // Deleting one releases a slot.
    let deleted = call(
        &router,
        "DeleteTaxRegistration",
        &json!({ "accountId": seeded[0] }),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "release delete failed: {:?}",
        deleted.body
    );
    let accepted = call(
        &router,
        "PutTaxRegistration",
        &json!({ "accountId": overflow_account, "taxRegistrationEntry": entry }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {overflow_account:?}: expected the released slot to be usable, got {accepted:?}"
    );
}
