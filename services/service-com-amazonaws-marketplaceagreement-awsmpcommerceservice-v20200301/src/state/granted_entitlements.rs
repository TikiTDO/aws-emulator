// SPDX-License-Identifier: CC0-1.0

//! The entitlements an accepted agreement grants, in the services that OWN
//! them.
//!
//! Accepting an agreement request is what grants the acceptor the entitlements
//! the agreement carries ([`AgreementEntitlementRecord`]), and such an
//! entitlement is a resource of two OTHER services:
//!
//! * the product entitlement the seller reads through the AWS Marketplace
//!   Entitlement Service — "an entitlement represents capacity in a product
//!   owned by the customer", keyed by the product code the entitlement is
//!   provisioned to and the buyer's account, carrying the capacity each product
//!   dimension was bought at and, as `Entitlement$ExpirationDate`, "the date at
//!   which the customer will renew or cancel their contract";
//! * the AWS License Manager licence `AgreementEntitlement$licenseArn` names —
//!   "the Amazon Resource Name (ARN) of the AWS License Manager license
//!   associated with the entitlement".
//!
//! So an entitlement this service reports is PUBLISHED into those two stores:
//! the seller's `GetEntitlements` answers with the entitlement the agreement
//! granted, and the licence ARN this service reports resolves to a licence
//! `GetLicense` returns, instead of naming a record no other service holds.
//!
//! [`publish`] is the one place that happens, and it RECONCILES rather than
//! appends: the grants an agreement makes follow the agreement's own status and
//! the clock — an acceptance grants them, an amendment restates what they
//! entitle, a cancellation, a replacement, a renewal or the end date takes them
//! away — so it is run after every call this service answers
//! ([`crate::service`]) and publishes exactly the grants the store holds at that
//! instant. Publishing twice at one instant writes the same records, and nothing
//! here reads either owning store to decide what this service answers.

use super::*;

use ::service_com_amazonaws_licensemanager_awslicensemanager::ops::types as license_types;
use ::service_com_amazonaws_licensemanager_awslicensemanager::state as license_manager;
use ::service_com_amazonaws_marketplaceentitlementservice_awsmpentitlementservice::state as entitlement_service;

/// The unit a granted dimension's capacity is counted in, from License
/// Manager's own `EntitlementUnit` value list
/// (`license_types::ENTITLEMENT_UNIT_VALUES`): a marketplace dimension is "a
/// category of capacity in a product" bought as a whole number of units — users,
/// seats, hosts — so the count is the unit it is granted in.
const LICENSE_ENTITLEMENT_UNIT: &str = "Count";

/// `ConsumptionConfiguration$renewType` for a licence an agreement granted,
/// from License Manager's `RenewType` value list: an agreement that renews is
/// renewed by this service, which grants the renewal its own licences, so the
/// licence itself never renews a consumption of its own.
const LICENSE_RENEW_TYPE: &str = "None";

/// The `LicenseStatus` a licence of a live entitlement carries, from License
/// Manager's own value list: the entitlement is granted, so the licence can be
/// checked out.
const LICENSE_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `LicenseStatus` of the licence of an entitlement whose agreement ran to
/// its end date: the licence "has expired" with the agreement that granted it.
const LICENSE_STATUS_EXPIRED: &str = "EXPIRED";

/// The `LicenseStatus` of the licence of an entitlement taken away before that
/// date — the agreement was cancelled, replaced or renewed into another — so the
/// licence is no longer usable although nothing expired.
const LICENSE_STATUS_DEACTIVATED: &str = "DEACTIVATED";

/// The `Metadata` name under which a published licence carries the agreement
/// that granted it, so a licence read out of License Manager says which
/// agreement entitles its beneficiary.
const LICENSE_METADATA_AGREEMENT_ID: &str = "AgreementId";

/// The `Metadata` name under which it carries the TYPE of the resource the
/// entitlement is provisioned to (`Resource$type`: "the product (for example,
/// `SaaSProduct`, `AmiProduct`, `ContainerProduct`)").
const LICENSE_METADATA_RESOURCE_TYPE: &str = "ResourceType";

/// The instant an ISO8601-UTC member falls back to when a stored instant cannot
/// be rendered at all, which `DateTime::fmt` admits as its only failure.
const EPOCH_ISO8601: &str = "1970-01-01T00:00:00Z";

/// The most dimensions one grant publishes a record for.
///
/// A ceiling this emulator sets on what it HOLDS, on the same footing as the
/// quotas beside it ([`AGREEMENT_REQUEST_QUOTA`]): `DimensionList` publishes no
/// maximum, so the dimension keys an acceptor configures are its own, and one
/// record per key is what the Entitlement Service would hold for each product of
/// each agreement. The agreement itself keeps reporting every dimension it was
/// accepted over (`GetAgreementTerms`); this bounds only how many of them one
/// grant publishes.
const MAX_PUBLISHED_DIMENSIONS: usize = 64;

/// The most records one publication writes into the Entitlement Service, which
/// bounds the PRODUCT of its factors — agreements, the resources each is
/// involved with, and the dimensions each was accepted over — rather than each
/// of them alone. Past it the remaining grants are left unpublished and the next
/// publication tries again.
const MAX_PUBLISHED_ENTITLEMENTS: usize = 10_000;

/// What a publication of this store's grants was made from: enough of the store
/// to tell whether the records it would write have changed at all.
///
/// The entitlements an agreement grants are DERIVED from the agreement and the
/// clock, so two publications made from the same summary write the same records.
/// It is deliberately conservative — every field is cheap to read and any
/// difference republishes — and it moves for each thing that can change a grant:
/// a minted or stored record (`agreements`, `sequence`), a status transition
/// (`active`, and `latest_update`, which every transition and every amendment
/// sets), and the clock crossing an agreement's start or end date (`started`,
/// `ended`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantSummary {
    /// How many agreements this store holds.
    agreements: usize,
    /// How many of them are `ACTIVE`.
    active: usize,
    /// How many of them have started by the instant this was taken.
    started: usize,
    /// How many of them have reached their own end date by it.
    ended: usize,
    /// The counter every id this service mints is drawn from.
    sequence: u64,
    /// The latest `lastUpdateTime` any of them carries.
    latest_update: Option<SystemTime>,
}

/// One entitlement of one agreement, flattened out of the stored agreement:
/// everything the two owning stores need, so neither is written while this
/// service's own store is locked.
#[derive(Debug)]
struct Grant {
    /// The agreement that granted this entitlement.
    agreement_id: String,
    /// The licence it was granted under, which is what identifies the grant in
    /// both owning stores.
    license_arn: String,
    /// The product the entitlement is provisioned to, as the product code
    /// `GetEntitlements` selects on: `Resource$id` is "the identifier of the
    /// resource", and the model states that identity — "we mention the term
    /// resource, which is most commonly a product, so a `resourceId` is also a
    /// `productId`" — so the resource this emulator's offer names IS the product
    /// the entitlement is for. It is the only product identity this emulator
    /// holds: the product's listing, which is where AWS mints a separate product
    /// code, belongs to no API this emulator serves.
    product_code: String,
    /// `Resource$type`, absent when the resource names none.
    resource_type: Option<String>,
    /// The buyer: the account the agreement records as its acceptor, which is
    /// the customer the entitlement belongs to.
    customer_account: String,
    /// The seller: the account the agreement records as its proposer, which is
    /// the licence's issuer.
    issuer_account: String,
    /// The Region the agreement — and so the licence it granted — belongs to.
    region: String,
    /// The capacity each dimension of the agreement entitles the buyer to, by
    /// dimension key ([`entitled_dimensions`]), SHARED by every entitlement of
    /// one agreement — the agreement's terms are the same for each resource it
    /// is involved with, so the list is built once per agreement and not copied
    /// per entitlement.
    dimensions: ::std::sync::Arc<Vec<(String, i32)>>,
    /// When the agreement started, which is when the licence became valid.
    start_time: SystemTime,
    /// When it ends, and `None` for an agreement that has no end date.
    end_time: Option<SystemTime>,
    /// Whether the entitlement is one the acceptor holds right now: its
    /// agreement is active and has started.
    granted: bool,
    /// The `LicenseStatus` the licence of this entitlement carries
    /// ([`license_status`]).
    license_status: &'static str,
}

/// Whether a publication made at `now` would write anything this store has not
/// already published ([`GrantSummary`]) — read with no record built and nothing
/// written, so the caller can decide whether the work below is owed at all.
pub fn publication_due<Eng: Send + Sync + 'static>(engine: &Eng, now: SystemTime) -> bool {
    let Some(state) = super::service_state(engine) else {
        return false;
    };
    let store = state.lock();
    store.published_grants.as_ref() != Some(&summarize(&store, now))
}

/// Publish the entitlements the agreements this store holds grant at `now` to
/// the services that own them.
///
/// Silent on an engine that holds no such store — there is then nothing to
/// publish into, and this service's own answers do not depend on it. Silent too
/// when this store has not moved since the last publication ([`GrantSummary`]):
/// the grants are derived from the agreements and the clock, so there is nothing
/// to restate until one of them changes, and a call that changed neither does no
/// work beyond reading that summary.
///
/// What one publication writes is bounded, and by the store rather than by how
/// often it is called: the records are RESTATED rather than appended to, so
/// nothing accumulates per request; the agreement count is bounded by
/// [`AGREEMENT_REQUEST_QUOTA`] — which the create path checks before it stores a
/// quote — the dimensions one grant publishes by [`MAX_PUBLISHED_DIMENSIONS`],
/// their product by [`MAX_PUBLISHED_ENTITLEMENTS`], and the licences by License
/// Manager's own ceiling, enforced where it takes one. Only a publication that
/// wrote EVERY grant records what it was made from, so one a ceiling cut short
/// is made again by the next call rather than waiting for the store to move.
///
/// Synchronous store work throughout, which is why its one caller runs it off
/// the async worker it was called on (`crate::service`).
///
/// LOCK ORDER: this service's guard is taken FIRST and dropped before either
/// owning store is taken, and neither of those services reads this store, so the
/// order cannot invert. Each owning store's guard is held only to APPLY records
/// built outside it.
pub fn publish<Eng: Send + Sync + 'static>(engine: &Eng, now: SystemTime) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let (grants, summary) = {
        let store = state.lock();
        let summary = summarize(&store, now);
        if store.published_grants.as_ref() == Some(&summary) {
            return;
        }
        (grants(&store, now), summary)
    };
    let live = grants.iter().filter(|grant| grant.granted).count();
    let licensed = publish_licenses(engine, &grants);
    let entitled = publish_entitlements(engine, &grants);
    if licensed && entitled {
        state.lock().published_grants = Some(summary);
    }
    // Counts only: an entitlement's registration token is a capability and is
    // never published or traced, and nothing of a caller's request is logged
    // here.
    ::tracing::debug!(
        target: "marketplace_agreement.audit",
        granted = live,
        withdrawn = grants.len() - live,
        complete = licensed && entitled,
        "the entitlements accepted agreements grant were published to the services that own them"
    );
}

/// What a publication made from `store` at `now` would depend on, read without
/// building a single record: see [`GrantSummary`] for why these fields are
/// enough.
fn summarize(
    store: &ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State,
    now: SystemTime,
) -> GrantSummary {
    let mut summary = GrantSummary {
        agreements: store.agreements.len(),
        active: 0,
        started: 0,
        ended: 0,
        sequence: store.id_sequence,
        latest_update: None,
    };
    for agreement in store.agreements.values() {
        summary.active += usize::from(agreement.status == STATUS_ACTIVE);
        summary.started += usize::from(now >= agreement.start_time);
        summary.ended += usize::from(agreement.reached_end_date(now));
        summary.latest_update = summary
            .latest_update
            .map_or(Some(agreement.updated_at), |latest| {
                Some(latest.max(agreement.updated_at))
            });
    }
    summary
}

/// Every entitlement every agreement of `store` granted, as the owning stores
/// need it, in the order the agreements are held in — so two publications of the
/// same grants write the same records in the same order and the page cursors the
/// Entitlement Service issues over them stay meaningful.
fn grants(
    store: &ComAmazonawsMarketplaceagreementAwsmpcommerceserviceV20200301State,
    now: SystemTime,
) -> Vec<Grant> {
    let mut grants = Vec::new();
    for agreement in store.agreements.values() {
        let dimensions = ::std::sync::Arc::new(entitled_dimensions(&agreement.accepted_terms));
        // An agreement past its own end date is one the clock has already ended
        // — expired there, or renewed into the agreement that continues its
        // chain, which this service materialises when it next settles that chain
        // — so it entitles its acceptor to nothing from that instant, whatever
        // status the record still carries.
        let ended = agreement.reached_end_date(now);
        for record in &agreement.entitlements {
            // An entitlement whose resource names no product cannot be keyed by
            // one: `GetEntitlements` selects on the product code, and a licence
            // is a licence FOR a product.
            let Some(product_code) = record.resource.id.clone() else {
                continue;
            };
            let (status, _) = agreement.entitlement_state(record, now);
            // The acceptor holds the entitlement from the acceptance on: the
            // provisioning window `PENDING` reports is this service's own view of
            // the licence being provisioned, while `SCHEDULED` is an agreement
            // that has not started yet and `DEPROVISIONED` one that entitles
            // nobody to anything.
            let granted = !ended && matches!(status, ENTITLEMENT_PENDING | ENTITLEMENT_PROVISIONED);
            grants.push(Grant {
                agreement_id: agreement.id.clone(),
                license_arn: record.license_arn.clone(),
                product_code,
                resource_type: record.resource.r#type.clone(),
                customer_account: party_account(agreement.acceptor.account_id.as_deref()),
                issuer_account: party_account(agreement.proposer.account_id.as_deref()),
                region: agreement.region.clone(),
                dimensions: ::std::sync::Arc::clone(&dimensions),
                start_time: agreement.start_time,
                end_time: agreement.end_time,
                granted,
                license_status: license_status(agreement, granted, ended),
            });
        }
    }
    grants
}

/// The account one party of an agreement names, or this emulator's own account
/// when the record carries none — both members are reported for every agreement
/// this service stores ([`agreement_parties`]), and a licence's beneficiary and
/// an entitlement's customer are not optional in the stores they go to.
fn party_account(account_id: Option<&str>) -> String {
    account_id
        .unwrap_or_else(|| ::engine::emulator_account_id())
        .to_string()
}

/// The `LicenseStatus` the licence of one entitlement carries: available while
/// the entitlement is granted, and otherwise what took it away — the end date
/// the agreement ran to, or the transition that ended it before that date.
///
/// The end date is read from the clock as well as from the status, so the answer
/// does not depend on whether the call that is publishing had already settled
/// that chain: an agreement still recorded `ACTIVE` past an end date that
/// expires it there has expired, while one whose end date RENEWS it is
/// superseded by the renewal, which grants licences of its own.
fn license_status(agreement: &Agreement, granted: bool, ended: bool) -> &'static str {
    match agreement.status.as_str() {
        _ if granted => LICENSE_STATUS_AVAILABLE,
        STATUS_ACTIVE if ended && !agreement.renews_at_end_date() => LICENSE_STATUS_EXPIRED,
        STATUS_EXPIRED => LICENSE_STATUS_EXPIRED,
        _ => LICENSE_STATUS_DEACTIVATED,
    }
}

/// The capacity each dimension of `terms` entitles the acceptor to, by
/// dimension key.
///
/// A dimension is bought at the quantity the accepted configurable-upfront term
/// configured for it ([`dimension_quantities`], the same resolution the quote
/// prices by), and a dimension a prepaid or free-trial term GRANTS instead is
/// entitled up to the capacity that grant allows (`GrantItem$maxQuantity`) — so
/// a configured quantity governs where a term carries both. Keyed in sorted
/// order, so the records built from it do not depend on a map's iteration order,
/// and cut at [`MAX_PUBLISHED_DIMENSIONS`], so one accepted configuration cannot
/// decide how much the publication below writes.
fn entitled_dimensions(terms: &[AcceptedTerm]) -> Vec<(String, i32)> {
    let mut capacity: BTreeMap<String, i32> = dimension_quantities(terms).into_iter().collect();
    for term in terms {
        let granted = match term {
            AcceptedTerm::fixedUpfrontPricingTerm(pricing) => pricing.grants.as_deref(),
            AcceptedTerm::freeTrialPricingTerm(pricing) => pricing.grants.as_deref(),
            _ => None,
        };
        for item in granted.unwrap_or_default() {
            if let Some(key) = item.dimension_key.as_deref() {
                capacity
                    .entry(key.to_string())
                    .or_insert(item.max_quantity);
            }
        }
    }
    capacity.into_iter().take(MAX_PUBLISHED_DIMENSIONS).collect()
}

/// Make the Entitlement Service's records for these grants' licences exactly
/// what the grants now entitle: one record per product dimension of each
/// entitlement the acceptor holds, and none at all for one it no longer does.
///
/// Bounded by [`MAX_PUBLISHED_ENTITLEMENTS`]: past that many records the
/// remaining grants are left for the next publication rather than taking that
/// store further, exactly as the stores of this emulator bound their own growth.
/// Answers whether every grant was published, which is what decides if this
/// publication is recorded as the one the store now stands at.
fn publish_entitlements<Eng: Send + Sync + 'static>(engine: &Eng, grants: &[Grant]) -> bool {
    let mut licenses: BTreeSet<String> = BTreeSet::new();
    let mut records: Vec<entitlement_service::StoredEntitlement> = Vec::new();
    let mut complete = true;
    for grant in grants {
        if records.len() >= MAX_PUBLISHED_ENTITLEMENTS {
            complete = false;
            break;
        }
        licenses.insert(grant.license_arn.clone());
        if grant.granted {
            records.extend(entitlement_records(grant));
        }
    }
    let published = entitlement_service::publish_license_entitlements(engine, &licenses, records);
    complete && published.is_some()
}

/// One grant as the Entitlement Service holds it: a record per dimension the
/// agreement entitles the buyer to, carrying that dimension's capacity, and a
/// single record carrying no dimension where the accepted terms grant capacity
/// in none — the buyer is entitled to the product either way, and `Dimension`
/// and `Value` are optional members of `Entitlement`.
fn entitlement_records(grant: &Grant) -> Vec<entitlement_service::StoredEntitlement> {
    // "The expiration date represents the minimum date through which this
    // entitlement is expected to remain valid… the date at which the customer
    // will renew or cancel their contract": the end date of the agreement that
    // granted it, absent for one that has no end date.
    let expiration = grant.end_time.map(::aws_smithy_types::DateTime::from);
    let record = |dimension: Option<&(String, i32)>| {
        let mut record = entitlement_service::StoredEntitlement::new(grant.product_code.as_str())
            // The two members that identify the buyer: "CustomerIdentifier is a
            // handle to each unique customer in an application" and
            // "CustomerAWSAccountId… specifies the AWS account ID of the buyer".
            // This emulator holds ONE identity for the acceptor — its account —
            // so both name it and either filter finds the entitlement.
            .with_customer_identifier(grant.customer_account.as_str())
            .with_customer_aws_account_id(grant.customer_account.as_str())
            .with_license_arn(grant.license_arn.as_str());
        if let Some(expiration) = expiration {
            record = record.with_expiration_date(expiration);
        }
        if let Some((key, quantity)) = dimension {
            record = record
                .with_dimension(key.as_str())
                // "The EntitlementValue represents the amount of capacity that
                // the customer is entitled to for the product", and a dimension
                // is bought in whole units, so the integer member is the one
                // populated.
                .with_value(entitlement_service::StoredEntitlementValue::integer(
                    *quantity,
                ));
        }
        record
    };
    if grant.dimensions.is_empty() {
        return vec![record(None)];
    }
    grant.dimensions.iter().map(|d| record(Some(d))).collect()
}

/// Create in License Manager the licence each grant names, and keep the status
/// and the window of one it already holds in step with the agreement that
/// granted it.
///
/// Every record is built BEFORE that store is locked, so its guard is held only
/// for the inserts and the field writes themselves and no other service waits on
/// this one's work.
///
/// Nothing a caller of License Manager set is touched: a licence an agreement
/// granted is created here and only ever restated from the agreement, and the
/// instant it was created at and the version it is stay as they were minted.
///
/// Answers whether every licence is now held, so a store that would not take one
/// leaves this publication unrecorded and the next call tries again.
fn publish_licenses<Eng: Send + Sync + 'static>(engine: &Eng, grants: &[Grant]) -> bool {
    let Some(state) = license_manager::service_state(engine) else {
        return false;
    };
    let published: Vec<license_manager::StoredLicense> =
        grants.iter().map(license_record).collect();
    let mut store = state.lock();
    let mut complete = true;
    for published in published {
        match store.licenses.get_mut(&published.arn) {
            Some(held) => {
                held.status = published.status;
                held.validity = published.validity;
                held.entitlements = published.entitlements;
                held.license_metadata = published.license_metadata;
            }
            // The store enforces its own ceiling, so a licence it will not take
            // is simply not published: this service reports the entitlement it
            // granted either way, and the next publication tries again.
            None => complete &= store.insert_license(published),
        }
    }
    complete
}

/// One grant as License Manager holds the licence it was granted under.
fn license_record(grant: &Grant) -> license_manager::StoredLicense {
    let mut metadata = vec![license_types::Metadata {
        name: Some(LICENSE_METADATA_AGREEMENT_ID.to_string()),
        value: Some(grant.agreement_id.clone()),
    }];
    if let Some(resource_type) = grant.resource_type.clone() {
        metadata.push(license_types::Metadata {
            name: Some(LICENSE_METADATA_RESOURCE_TYPE.to_string()),
            value: Some(resource_type),
        });
    }
    license_manager::StoredLicense {
        id: license_id(&grant.license_arn),
        arn: grant.license_arn.clone(),
        // The product the licence entitles its beneficiary to is the only
        // product identity this emulator holds (see [`Grant::product_code`]):
        // there is no listing to read a title or a SKU of its own from.
        license_name: grant.product_code.clone(),
        product_name: grant.product_code.clone(),
        product_sku: grant.product_code.clone(),
        // The seller proposed the agreement, so the seller issued its licences;
        // the key fingerprint is derived by License Manager itself, as it is for
        // a licence created through its own API.
        issuer: license_manager::issuer_details(license_types::Issuer {
            name: grant.issuer_account.clone(),
            sign_key: None,
        }),
        home_region: grant.region.clone(),
        status: grant.license_status.to_string(),
        // The licence is valid for exactly the window the agreement commits:
        // from the agreement's start date to its end date, and open-ended for an
        // agreement that has none.
        validity: license_types::DatetimeRange {
            begin: iso8601(grant.start_time),
            end: grant.end_time.map(iso8601),
        },
        beneficiary: grant.customer_account.clone(),
        entitlements: grant
            .dimensions
            .iter()
            .map(|(key, quantity)| license_types::Entitlement {
                name: key.clone(),
                value: None,
                max_count: Some(i64::from(*quantity)),
                // The licence entitles the beneficiary to the capacity it was
                // bought at and no more, and a checkout of it is not checked
                // back in by the buyer: the agreement is what holds it.
                overage: Some(false),
                unit: LICENSE_ENTITLEMENT_UNIT.to_string(),
                allow_check_in: Some(false),
            })
            .collect(),
        consumption_configuration: license_types::ConsumptionConfiguration {
            renew_type: Some(LICENSE_RENEW_TYPE.to_string()),
            provisional_configuration: None,
            borrow_configuration: None,
        },
        license_metadata: Some(metadata),
        // The licence was granted when the agreement it belongs to started, not
        // when some later call published it, so repeated publications report the
        // same create time.
        create_time: iso8601(grant.start_time),
        version: license_manager::FIRST_LICENSE_VERSION.to_string(),
        // No caller sent this licence, so it carries no idempotency token of
        // one; the store keys its token map only on a non-empty one.
        client_token: String::new(),
    }
}

/// The licence id inside a licence ARN: the resource segment's last field,
/// which is the `l-…` id this service minted the ARN around.
fn license_id(license_arn: &str) -> String {
    license_arn
        .split(':')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// `instant` as the ISO8601-UTC text License Manager's string-valued instants
/// carry (`License$createTime`, `DatetimeRange`): RFC 3339 in UTC, which is what
/// those members' modelled pattern admits.
fn iso8601(instant: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(instant)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| EPOCH_ISO8601.to_string())
}
