// SPDX-License-Identifier: CC0-1.0

//! The three S3 Access Grants LISTINGS and the Identity Center dissociation, driven end to
//! end over HTTP.
//!
//! The rules under test:
//!
//! * Every one of these calls is about THE INSTANCE of the account the request names, in
//!   the Region it was signed for: `ListAccessGrants` "returns the list of access grants in
//!   your S3 Access Grants instance", `ListAccessGrantsLocations` "a list of the locations
//!   registered in your S3 Access Grants instance", `ListCallerAccessGrants` "the access
//!   grants that grant the caller access", and `DissociateAccessGrantsIdentityCenter`
//!   "dissociates the Amazon Web Services IAM Identity Center instance from the S3 Access
//!   Grants instance" (each operation's own documentation). So an account with no instance
//!   in that Region is REFUSED rather than answered with an empty page — with the fixed
//!   `AccessGrantsInstanceNotExistsError` the reads of that surface share, and with
//!   `InvalidRequest` for the dissociation — and an instance in one Region says nothing
//!   about another.
//! * Once the instance exists, each listing reports exactly the records that instance
//!   holds, in the order the service scans them, narrowed by every filter the request
//!   carries and paged by `MaxResults` / `NextToken`.
//! * `ListCallerAccessGrants` reports the grants whose grantee IS the caller — "the caller
//!   (grantee) can be an Identity and Access Management (IAM) identity" — and no other
//!   account's or principal's grants.
//!
//! The tables below differ along what those rules generalise over: which listing is being
//! asked, which Region the request is signed for, how many records the instance holds, how
//! big a page is asked for, which filter narrows it, and whose grantee a grant names.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router. The router, the request builder
//! and the XML readers are the sibling suites', reused rather than copied.

use std::collections::BTreeSet;

use crate::access_grants_data_plane::{create_instance, create_location, query_escape};
use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, xml_escape, Emulator, Reply, ACCOUNT,
    NO_CONTENT, OTHER_ACCOUNT, REGION, XMLNS,
};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    access_grants_listing::MAX_PAGE_SIZE, NO_ACCESS_GRANTS_INSTANCE_TO_DISSOCIATE_MESSAGE,
    NO_SUCH_ACCESS_GRANTS_INSTANCE_MESSAGE, PERMISSION_READ, PERMISSION_READWRITE,
    PERMISSION_WRITE,
};

/// A second signing Region: an instance is Region-scoped, so a listing signed here says
/// nothing about the instance in [`REGION`].
const OTHER_REGION: &str = "eu-central-1";

/// The code every read of this surface refuses an account with no instance with, and its
/// status (the message is the service's own fixed text).
const NO_INSTANCE_CODE: &str = "AccessGrantsInstanceNotExistsError";
/// See [`NO_INSTANCE_CODE`].
const NO_INSTANCE_STATUS: u16 = 404;

/// The code and status `DissociateAccessGrantsIdentityCenter` refuses the same precondition
/// with — the answer this service gives a well-formed request it cannot carry out.
const DISSOCIATE_WITHOUT_INSTANCE_CODE: &str = "InvalidRequest";
/// See [`DISSOCIATE_WITHOUT_INSTANCE_CODE`].
const DISSOCIATE_WITHOUT_INSTANCE_STATUS: u16 = 400;

/// The IAM Identity Center instance a create may associate — the shape
/// `IdentityCenterArn`'s pattern (`^arn:[^:]+:sso::(\d{12}){0,1}:instance/.*$`) requires.
const IDENTITY_CENTER_INSTANCE: &str = "arn:aws:sso::000000000000:instance/ssoins-0123456789abcdef";

/// An Identity Center application ARN a grant may be authorized for — the shape
/// `IdentityCenterApplicationArn`'s pattern
/// (`^arn:[^:]+:sso::\d{12}:application/.*$`) requires.
const APPLICATION_ARN: &str = "arn:aws:sso::000000000000:application/ssoins-0123456789abcdef/apl-01";

/// One listing of this surface: the path it is bound to, and the element its entries are
/// carried in.
struct Listing {
    case: &'static str,
    uri: &'static str,
    /// The element a reply reports one entry's identity in, so a row can read what a page
    /// carried.
    entry_element: &'static str,
}

/// The listing bound to `/grants`.
const GRANTS: Listing = Listing {
    case: "ListAccessGrants",
    uri: "/v20180820/accessgrantsinstance/grants",
    entry_element: "AccessGrantId",
};

/// The listing bound to `/locations`.
const LOCATIONS: Listing = Listing {
    case: "ListAccessGrantsLocations",
    uri: "/v20180820/accessgrantsinstance/locations",
    entry_element: "AccessGrantsLocationId",
};

/// The listing bound to `/caller/grants`, whose entries carry no id at all — a caller's
/// grant is reported by its permission and scope — so its rows read the scope.
const CALLER_GRANTS: Listing = Listing {
    case: "ListCallerAccessGrants",
    uri: "/v20180820/accessgrantsinstance/caller/grants",
    entry_element: "GrantScope",
};

/// Every listing of the surface, which the precondition rows below run over.
fn every_listing() -> [Listing; 3] {
    [GRANTS, LOCATIONS, CALLER_GRANTS]
}

/// Ask `listing` in `region`, with `query` appended to its path.
async fn list(
    emulator: &Emulator,
    listing: &Listing,
    account_id: &str,
    region: &str,
    query: &str,
) -> Reply {
    let uri = match query.is_empty() {
        true => listing.uri.to_string(),
        false => format!("{}?{query}", listing.uri),
    };
    call_in_region(&emulator.router, region, "GET", &uri, account_id, "").await
}

/// `DissociateAccessGrantsIdentityCenter` for the account's instance in `region`.
async fn dissociate(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "DELETE",
        "/v20180820/accessgrantsinstance/identitycenter",
        account_id,
        "",
    )
    .await
}

/// Read the account's instance in `region`.
async fn get_instance(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        "/v20180820/accessgrantsinstance",
        account_id,
        "",
    )
    .await
}

/// Create the account's instance in `region` with an IAM Identity Center instance
/// associated, which is what gives it both Identity Center ARNs to dissociate.
async fn create_instance_with_identity_center(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
) -> Reply {
    let body = format!(
        "<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\"><IdentityCenterArn>{}\
         </IdentityCenterArn></CreateAccessGrantsInstanceRequest>",
        xml_escape(IDENTITY_CENTER_INSTANCE)
    );
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/accessgrantsinstance",
        account_id,
        &body,
    )
    .await
}

/// One grant to create: the location it narrows, who it is granted to, what it grants, and
/// the Identity Center application it is authorized for, if any.
struct GrantCase {
    location_id: String,
    sub_prefix: Option<String>,
    grantee_type: &'static str,
    grantee_identifier: String,
    permission: &'static str,
    application_arn: Option<&'static str>,
}

/// Create the grant `case` describes in the account's instance in `region`.
///
/// The sibling data-plane suite's helper fixes its grantee and names no application, which
/// is exactly what the caller listing and the application filter below have to vary.
async fn create_grant(emulator: &Emulator, region: &str, case: &GrantCase) -> Reply {
    let mut body = format!(
        "<CreateAccessGrantRequest xmlns=\"{XMLNS}\">\
         <AccessGrantsLocationId>{}</AccessGrantsLocationId>",
        xml_escape(&case.location_id)
    );
    if let Some(sub_prefix) = &case.sub_prefix {
        body.push_str(&format!(
            "<AccessGrantsLocationConfiguration><S3SubPrefix>{}</S3SubPrefix>\
             </AccessGrantsLocationConfiguration>",
            xml_escape(sub_prefix)
        ));
    }
    body.push_str(&format!(
        "<Grantee><GranteeType>{}</GranteeType><GranteeIdentifier>{}</GranteeIdentifier>\
         </Grantee><Permission>{}</Permission>",
        case.grantee_type,
        xml_escape(&case.grantee_identifier),
        case.permission
    ));
    if let Some(application_arn) = case.application_arn {
        body.push_str(&format!(
            "<ApplicationArn>{}</ApplicationArn>",
            xml_escape(application_arn)
        ));
    }
    body.push_str("</CreateAccessGrantRequest>");
    call_in_region(
        &emulator.router,
        region,
        "POST",
        "/v20180820/accessgrantsinstance/grant",
        ACCOUNT,
        &body,
    )
    .await
}

/// Register `location_scope` and answer the id the service assigned it.
async fn register_location(emulator: &Emulator, region: &str, location_scope: &str) -> String {
    let registered = create_location(emulator, ACCOUNT, region, location_scope).await;
    assert_eq!(
        registered.status, 200,
        "the location {location_scope} must be registered first, got {} with body {}",
        registered.status, registered.body
    );
    registered
        .text("AccessGrantsLocationId")
        .unwrap_or_else(|| panic!("no AccessGrantsLocationId in {}", registered.body))
}

/// The IAM identity the requests of this suite speak for: the principal this emulator's
/// own long-term credential belongs to, which is what the service resolves the caller to.
fn caller_arn() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// Another IAM role, which is NOT the caller: a grant to it grants the caller nothing.
const OTHER_GRANTEE: &str = "arn:aws:iam::123456789012:role/data-reader";

#[tokio::test]
async fn every_call_of_the_surface_requires_the_instance_it_is_about() {
    let emulator = Emulator::start();

    // With no instance anywhere, every listing is refused with the fixed refusal of this
    // surface — in both Regions, and whichever page size or filter the request carries.
    for listing in every_listing() {
        for (case, query) in [
            ("no query at all", String::new()),
            ("a page size", "maxResults=1".to_string()),
            (
                "a filter",
                format!("grantscope={}", query_escape("s3://reports/*")),
            ),
        ] {
            for region in [REGION, OTHER_REGION] {
                let refused = list(&emulator, &listing, ACCOUNT, region, &query).await;
                assert_eq!(
                    (refused.status, refused.code().as_str()),
                    (NO_INSTANCE_STATUS, NO_INSTANCE_CODE),
                    "case {} with {case} in {region}: expected the no-instance refusal, got {} \
                     with body {}",
                    listing.case,
                    refused.status,
                    refused.body
                );
                assert_eq!(
                    refused.text("Message").as_deref(),
                    Some(NO_SUCH_ACCESS_GRANTS_INSTANCE_MESSAGE),
                    "case {} with {case} in {region}: expected the service's fixed message, got \
                     body {}",
                    listing.case,
                    refused.body
                );
            }
        }
    }
    // The dissociation answers the same precondition with its own code.
    for region in [REGION, OTHER_REGION] {
        let refused = dissociate(&emulator, ACCOUNT, region).await;
        assert_eq!(
            (refused.status, refused.code().as_str()),
            (
                DISSOCIATE_WITHOUT_INSTANCE_STATUS,
                DISSOCIATE_WITHOUT_INSTANCE_CODE
            ),
            "case DissociateAccessGrantsIdentityCenter in {region}: expected the no-instance \
             refusal, got {} with body {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.text("Message").as_deref(),
            Some(NO_ACCESS_GRANTS_INSTANCE_TO_DISSOCIATE_MESSAGE),
            "case DissociateAccessGrantsIdentityCenter in {region}: expected the fixed message, \
             got body {}",
            refused.body
        );
    }

    // The instance in ONE Region is what makes the listings answer THERE: an empty instance
    // reports an empty page, which is a different answer from the refusal above, and the
    // other Region is still refused.
    assert_eq!(
        create_instance(&emulator, ACCOUNT, REGION).await.status,
        200,
        "the instance must be created first"
    );
    for listing in every_listing() {
        let answered = list(&emulator, &listing, ACCOUNT, REGION, "").await;
        assert_eq!(
            answered.status, 200,
            "case {}: expected the listing to answer once the instance exists, got {} with body \
             {}",
            listing.case, answered.status, answered.body
        );
        assert!(
            answered.texts(listing.entry_element).is_empty()
                && answered.text("NextToken").is_none(),
            "case {}: expected an empty page with no token for an instance holding nothing, got \
             body {}",
            listing.case,
            answered.body
        );
        let elsewhere = list(&emulator, &listing, ACCOUNT, OTHER_REGION, "").await;
        assert_eq!(
            (elsewhere.status, elsewhere.code().as_str()),
            (NO_INSTANCE_STATUS, NO_INSTANCE_CODE),
            "case {} in {OTHER_REGION}: expected an instance in {REGION} to say nothing about \
             another Region, got {} with body {}",
            listing.case,
            elsewhere.status,
            elsewhere.body
        );
        // And a request addressed in another account is refused before any instance is
        // looked up, so the answer says nothing about what that account holds.
        assert_foreign_account_refused(
            listing.case,
            OTHER_ACCOUNT,
            &list(&emulator, &listing, OTHER_ACCOUNT, REGION, "").await,
        );
    }
}

#[tokio::test]
async fn a_listing_reports_its_instances_records_and_pages_through_them() {
    let emulator = Emulator::start();
    assert_eq!(
        create_instance(&emulator, ACCOUNT, REGION).await.status,
        200,
        "the instance must be created first"
    );
    // An instance in another Region holding a record of its own, so every assertion below
    // also pins that a listing reports ONE instance's records.
    assert_eq!(
        create_instance(&emulator, ACCOUNT, OTHER_REGION).await.status,
        200,
        "the second Region's instance must be created too"
    );
    let elsewhere = register_location(&emulator, OTHER_REGION, "s3://elsewhere-bucket").await;

    // Five locations, each with one grant on it: enough records that a page size of two
    // leaves a token behind on three pages.
    let mut location_ids = BTreeSet::new();
    let mut grant_ids = BTreeSet::new();
    for index in 0..5 {
        let location_id =
            register_location(&emulator, REGION, &format!("s3://reports-{index:02}")).await;
        let grant = create_grant(
            &emulator,
            REGION,
            &GrantCase {
                location_id: location_id.clone(),
                sub_prefix: Some("data/*".to_string()),
                grantee_type: "IAM",
                grantee_identifier: OTHER_GRANTEE.to_string(),
                permission: PERMISSION_READ,
                application_arn: None,
            },
        )
        .await;
        assert_eq!(
            grant.status, 200,
            "the grant on {location_id} must be created, got {} with body {}",
            grant.status, grant.body
        );
        grant_ids.insert(
            grant
                .text("AccessGrantId")
                .unwrap_or_else(|| panic!("no AccessGrantId in {}", grant.body)),
        );
        location_ids.insert(location_id);
    }

    // (the listing, every id its instance holds)
    for (listing, held) in [(LOCATIONS, &location_ids), (GRANTS, &grant_ids)] {
        // One page holds everything when the request names no size: the ids reported are
        // exactly the ones the creates returned, and the record of the other Region's
        // instance is not among them.
        let whole = list(&emulator, &listing, ACCOUNT, REGION, "").await;
        assert_eq!(
            whole.texts(listing.entry_element),
            held.iter().cloned().collect::<Vec<_>>(),
            "case {}: expected every record of this instance, in scan order, got body {}",
            listing.case,
            whole.body
        );
        assert!(
            whole.text("NextToken").is_none(),
            "case {}: expected no token on a page that reached the end, got body {}",
            listing.case,
            whole.body
        );
        assert!(
            !whole.texts(listing.entry_element).contains(&elsewhere),
            "case {}: expected the other Region's record to be left out, got body {}",
            listing.case,
            whole.body
        );

        // Each page size is a row: the pages it produces, followed through the tokens,
        // report every record exactly once and in the same order.
        for page_size in [1usize, 2, 4, 5, 6] {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let query = match &token {
                    Some(token) => format!(
                        "maxResults={page_size}&nextToken={}",
                        query_escape(token)
                    ),
                    None => format!("maxResults={page_size}"),
                };
                let page = list(&emulator, &listing, ACCOUNT, REGION, &query).await;
                assert_eq!(
                    page.status, 200,
                    "case {} with maxResults={page_size}: expected a page, got {} with body {}",
                    listing.case, page.status, page.body
                );
                let entries = page.texts(listing.entry_element);
                assert!(
                    entries.len() <= page_size,
                    "case {} with maxResults={page_size}: expected at most {page_size} entries, \
                     got {} in body {}",
                    listing.case,
                    entries.len(),
                    page.body
                );
                seen.extend(entries);
                token = page.text("NextToken");
                pages += 1;
                assert!(
                    pages <= held.len() + 1,
                    "case {} with maxResults={page_size}: expected the paging to end, got {pages} \
                     pages",
                    listing.case
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen,
                held.iter().cloned().collect::<Vec<_>>(),
                "case {} with maxResults={page_size}: expected every record once, in scan order, \
                 got {seen:?}",
                listing.case
            );
        }

        // A token this service did not issue names no position in the scan.
        for (case, token) in [
            ("a token that is not even hex", "not-a-token".to_string()),
            ("a hex token naming a record that is not there", {
                let mut encoded = String::new();
                for byte in b"zzzzzzzz" {
                    encoded.push_str(&format!("{byte:02x}"));
                }
                // This one IS a token the service could have issued, so it must be
                // ACCEPTED and simply resume past every record.
                encoded
            }),
        ] {
            let answered = list(
                &emulator,
                &listing,
                ACCOUNT,
                REGION,
                &format!("nextToken={}", query_escape(&token)),
            )
            .await;
            match case.starts_with("a token that is not even hex") {
                true => assert_eq!(
                    answered.status, 400,
                    "case {} with {case}: expected a token this service did not issue to be \
                     refused, got {} with body {}",
                    listing.case, answered.status, answered.body
                ),
                false => assert_eq!(
                    (answered.status, answered.texts(listing.entry_element).len()),
                    (200, 0),
                    "case {} with {case}: expected a page past the last record, got {} with body \
                     {}",
                    listing.case,
                    answered.status,
                    answered.body
                ),
            }
        }

        // The page size the model admits at its upper bound is served, and one past it is
        // refused rather than clamped.
        let at_the_bound = list(
            &emulator,
            &listing,
            ACCOUNT,
            REGION,
            &format!("maxResults={MAX_PAGE_SIZE}"),
        )
        .await;
        assert_eq!(
            (
                at_the_bound.status,
                at_the_bound.texts(listing.entry_element).len()
            ),
            (200, held.len()),
            "case {} with maxResults={MAX_PAGE_SIZE}: expected the whole set, got {} with body {}",
            listing.case,
            at_the_bound.status,
            at_the_bound.body
        );
        let past_the_bound = list(
            &emulator,
            &listing,
            ACCOUNT,
            REGION,
            &format!("maxResults={}", MAX_PAGE_SIZE + 1),
        )
        .await;
        assert_eq!(
            past_the_bound.status, 400,
            "case {} with maxResults={}: expected a page size past the modelled range to be \
             refused, got {} with body {}",
            listing.case,
            MAX_PAGE_SIZE + 1,
            past_the_bound.status,
            past_the_bound.body
        );
    }
}

#[tokio::test]
async fn a_listing_narrows_to_what_each_filter_names() {
    let emulator = Emulator::start();
    assert_eq!(
        create_instance(&emulator, ACCOUNT, REGION).await.status,
        200,
        "the instance must be created first"
    );
    let reports = register_location(&emulator, REGION, "s3://reports").await;
    let archive = register_location(&emulator, REGION, "s3://archive").await;

    // Three grants differing in grantee, permission, scope and application: enough that
    // every filter below selects a different subset.
    let rows = [
        GrantCase {
            location_id: reports.clone(),
            sub_prefix: Some("daily/*".to_string()),
            grantee_type: "IAM",
            grantee_identifier: OTHER_GRANTEE.to_string(),
            permission: PERMISSION_READ,
            application_arn: None,
        },
        GrantCase {
            location_id: reports.clone(),
            sub_prefix: Some("weekly/*".to_string()),
            grantee_type: "IAM",
            grantee_identifier: caller_arn(),
            permission: PERMISSION_WRITE,
            application_arn: Some(APPLICATION_ARN),
        },
        GrantCase {
            location_id: archive.clone(),
            sub_prefix: Some("cold/*".to_string()),
            grantee_type: "DIRECTORY_USER",
            grantee_identifier: "a1b2c3d4-5678-90ab-cdef-EXAMPLE11111".to_string(),
            permission: PERMISSION_READWRITE,
            application_arn: None,
        },
    ];
    let mut scopes = Vec::new();
    for case in &rows {
        let created = create_grant(&emulator, REGION, case).await;
        assert_eq!(
            created.status, 200,
            "every grant must be created first, got {} with body {}",
            created.status, created.body
        );
        scopes.push(
            created
                .text("GrantScope")
                .unwrap_or_else(|| panic!("no GrantScope in {}", created.body)),
        );
    }

    // (case, the query, how many grants it must report)
    let grant_filters: Vec<(String, String, usize)> = vec![
        (
            "GranteeType=IAM".to_string(),
            "granteetype=IAM".to_string(),
            2,
        ),
        (
            "GranteeType=DIRECTORY_USER".to_string(),
            "granteetype=DIRECTORY_USER".to_string(),
            1,
        ),
        (
            "the caller's own grantee identifier".to_string(),
            format!("granteeidentifier={}", query_escape(&caller_arn())),
            1,
        ),
        (
            "a grantee identifier nothing names".to_string(),
            format!(
                "granteeidentifier={}",
                query_escape("arn:aws:iam::123456789012:role/nobody")
            ),
            0,
        ),
        (
            format!("Permission={PERMISSION_READ}"),
            format!("permission={PERMISSION_READ}"),
            1,
        ),
        (
            "one grant's whole scope".to_string(),
            format!("grantscope={}", query_escape(&scopes[0])),
            1,
        ),
        (
            "a scope no grant covers".to_string(),
            format!("grantscope={}", query_escape("s3://reports/hourly/*")),
            0,
        ),
        (
            "the application one grant is authorized for".to_string(),
            format!("application_arn={}", query_escape(APPLICATION_ARN)),
            1,
        ),
        (
            "two filters that select the same grant".to_string(),
            format!(
                "granteetype=IAM&permission={PERMISSION_WRITE}&granteeidentifier={}",
                query_escape(&caller_arn())
            ),
            1,
        ),
        (
            "two filters no one grant satisfies".to_string(),
            format!("granteetype=IAM&permission={PERMISSION_READWRITE}"),
            0,
        ),
    ];
    for (case, query, expected) in grant_filters {
        let narrowed = list(&emulator, &GRANTS, ACCOUNT, REGION, &query).await;
        assert_eq!(
            (narrowed.status, narrowed.texts("AccessGrantId").len()),
            (200, expected),
            "case {case:?}: expected {expected} grant(s), got body {}",
            narrowed.body
        );
    }

    // The locations listing narrows on the S3 path a location was registered under.
    for (case, query, expected) in [
        (
            "the scope of one registered location",
            format!("locationscope={}", query_escape("s3://reports")),
            vec![reports.clone()],
        ),
        (
            "the scope of the other",
            format!("locationscope={}", query_escape("s3://archive")),
            vec![archive.clone()],
        ),
        (
            "a scope nothing is registered under",
            format!("locationscope={}", query_escape("s3://nowhere")),
            Vec::new(),
        ),
    ] {
        let narrowed = list(&emulator, &LOCATIONS, ACCOUNT, REGION, &query).await;
        assert_eq!(
            (narrowed.status, narrowed.texts("AccessGrantsLocationId")),
            (200, expected.clone()),
            "case {case:?}: expected {expected:?}, got body {}",
            narrowed.body
        );
    }
}

#[tokio::test]
async fn the_caller_listing_reports_the_grants_made_to_the_caller() {
    let emulator = Emulator::start();
    assert_eq!(
        create_instance(&emulator, ACCOUNT, REGION).await.status,
        200,
        "the instance must be created first"
    );
    let location = register_location(&emulator, REGION, "s3://reports").await;

    // Four grants: two to the caller (one of them authorized for an Identity Center
    // application), one to another IAM role, and one to a corporate directory user — which
    // signs no AWS request, so it is nobody's caller.
    let rows = [
        (
            "to the caller, under daily/",
            GrantCase {
                location_id: location.clone(),
                sub_prefix: Some("daily/*".to_string()),
                grantee_type: "IAM",
                grantee_identifier: caller_arn(),
                permission: PERMISSION_READ,
                application_arn: None,
            },
            true,
        ),
        (
            "to the caller, through an application",
            GrantCase {
                location_id: location.clone(),
                sub_prefix: Some("weekly/*".to_string()),
                grantee_type: "IAM",
                grantee_identifier: caller_arn(),
                permission: PERMISSION_READWRITE,
                application_arn: Some(APPLICATION_ARN),
            },
            true,
        ),
        (
            "to another IAM role",
            GrantCase {
                location_id: location.clone(),
                sub_prefix: Some("other/*".to_string()),
                grantee_type: "IAM",
                grantee_identifier: OTHER_GRANTEE.to_string(),
                permission: PERMISSION_READ,
                application_arn: None,
            },
            false,
        ),
        (
            "to a corporate directory user",
            GrantCase {
                location_id: location.clone(),
                sub_prefix: Some("directory/*".to_string()),
                grantee_type: "DIRECTORY_USER",
                grantee_identifier: "a1b2c3d4-5678-90ab-cdef-EXAMPLE11111".to_string(),
                permission: PERMISSION_READ,
                application_arn: None,
            },
            false,
        ),
    ];
    let mut granted_to_the_caller: Vec<String> = Vec::new();
    for (case, grant, to_the_caller) in &rows {
        let created = create_grant(&emulator, REGION, grant).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the grant must be created first, got {} with body {}",
            created.status, created.body
        );
        let scope = created
            .text("GrantScope")
            .unwrap_or_else(|| panic!("case {case:?}: no GrantScope in {}", created.body));
        if *to_the_caller {
            granted_to_the_caller.push(scope);
        }
    }
    granted_to_the_caller.sort();

    let reported = list(&emulator, &CALLER_GRANTS, ACCOUNT, REGION, "").await;
    let mut scopes = reported.texts("GrantScope");
    scopes.sort();
    assert_eq!(
        (reported.status, scopes),
        (200, granted_to_the_caller.clone()),
        "expected only the grants made to the caller, got body {}",
        reported.body
    );
    // The whole-instance listing, in contrast, reports every grant — so the filtering above
    // is about the caller and not about what was stored.
    assert_eq!(
        list(&emulator, &GRANTS, ACCOUNT, REGION, "")
            .await
            .texts("AccessGrantId")
            .len(),
        rows.len(),
        "expected ListAccessGrants to report every grant of the instance"
    );

    // (case, the query, the scopes it must report)
    for (case, query, expected) in [
        (
            "narrowed by a fragment of the path",
            format!("grantscope={}", query_escape("daily")),
            vec![granted_to_the_caller[0].clone()],
        ),
        (
            "narrowed by a fragment no grant's path contains",
            format!("grantscope={}", query_escape("hourly")),
            Vec::new(),
        ),
        (
            "narrowed by a fragment every grant's path contains",
            format!("grantscope={}", query_escape("s3://reports")),
            granted_to_the_caller.clone(),
        ),
        (
            "only the grants authorized for an application",
            "allowedByApplication=true".to_string(),
            vec![granted_to_the_caller[1].clone()],
        ),
        (
            "with the application filter off",
            "allowedByApplication=false".to_string(),
            granted_to_the_caller.clone(),
        ),
    ] {
        let narrowed = list(&emulator, &CALLER_GRANTS, ACCOUNT, REGION, &query).await;
        let mut scopes = narrowed.texts("GrantScope");
        scopes.sort();
        assert_eq!(
            (narrowed.status, scopes),
            (200, expected.clone()),
            "case {case:?}: expected {expected:?}, got body {}",
            narrowed.body
        );
    }

    // Paged one grant at a time, the two pages report the caller's two grants between
    // them — in grant id order, which is the order the instance's grants are scanned in,
    // so neither page's content is asserted on its own.
    let first = list(&emulator, &CALLER_GRANTS, ACCOUNT, REGION, "maxResults=1").await;
    let token = first
        .text("NextToken")
        .unwrap_or_else(|| panic!("expected a token on a page of one, got body {}", first.body));
    let second = list(
        &emulator,
        &CALLER_GRANTS,
        ACCOUNT,
        REGION,
        &format!("maxResults=1&nextToken={}", query_escape(&token)),
    )
    .await;
    let mut paged = first.texts("GrantScope");
    paged.extend(second.texts("GrantScope"));
    paged.sort();
    assert_eq!(
        (first.texts("GrantScope").len(), paged),
        (1, granted_to_the_caller.clone()),
        "expected the two pages to report the caller's grants exactly once between them, got \
         bodies {} and {}",
        first.body,
        second.body
    );
}

#[tokio::test]
async fn dissociating_identity_center_takes_both_arns_off_the_instance() {
    let emulator = Emulator::start();
    assert_eq!(
        create_instance_with_identity_center(&emulator, ACCOUNT, REGION)
            .await
            .status,
        200,
        "the instance must be created with an Identity Center instance associated"
    );
    // The instance in another Region is created WITHOUT one, so the rows below also pin
    // that the dissociation is about the instance of the Region the request was signed for.
    assert_eq!(
        create_instance(&emulator, ACCOUNT, OTHER_REGION).await.status,
        200,
        "the second Region's instance must be created too"
    );

    let associated = get_instance(&emulator, ACCOUNT, REGION).await;
    assert_eq!(
        associated.text("IdentityCenterInstanceArn").as_deref(),
        Some(IDENTITY_CENTER_INSTANCE),
        "expected the created instance to report the Identity Center instance, got body {}",
        associated.body
    );
    let application = associated
        .text("IdentityCenterApplicationArn")
        .unwrap_or_else(|| panic!("no IdentityCenterApplicationArn in {}", associated.body));

    let removed = dissociate(&emulator, ACCOUNT, REGION).await;
    assert_eq!(
        removed.status, NO_CONTENT,
        "expected the dissociation to answer {NO_CONTENT}, got {} with body {}",
        removed.status, removed.body
    );
    let after = get_instance(&emulator, ACCOUNT, REGION).await;
    for member in [
        "IdentityCenterArn",
        "IdentityCenterInstanceArn",
        "IdentityCenterApplicationArn",
    ] {
        assert_eq!(
            after.text(member),
            None,
            "case {member}: expected the dissociated instance to report none, got body {}",
            after.body
        );
    }
    assert!(
        !after.body.contains(&application),
        "expected the application S3 Access Grants had created to be gone, got body {}",
        after.body
    );
    // The instance itself is untouched: it is still reported, and still has its id.
    assert_eq!(
        after.status, 200,
        "expected the instance to be reported after the dissociation, got {} with body {}",
        after.status, after.body
    );

    // A repeat dissociation is served: the instance is already in the state the call asks
    // for, and so is an instance created without an association at all.
    for (case, region) in [
        ("a repeat dissociation", REGION),
        ("an instance that never had an association", OTHER_REGION),
    ] {
        let repeat = dissociate(&emulator, ACCOUNT, region).await;
        assert_eq!(
            repeat.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT}, got {} with body {}",
            repeat.status, repeat.body
        );
        let read = get_instance(&emulator, ACCOUNT, region).await;
        assert_eq!(
            (read.status, read.text("IdentityCenterInstanceArn")),
            (200, None),
            "case {case:?}: expected the instance to carry no association, got body {}",
            read.body
        );
    }

    // And a dissociation addressed in another account is refused before any instance is
    // reached.
    assert_foreign_account_refused(
        "DissociateAccessGrantsIdentityCenter",
        OTHER_ACCOUNT,
        &dissociate(&emulator, OTHER_ACCOUNT, REGION).await,
    );
}
