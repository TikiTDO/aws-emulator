// SPDX-License-Identifier: CC0-1.0

//! `ListTypes` over the awsQuery wire: the extension registry LISTING.
//!
//! The rule every case here holds the service to: **`ListTypes` reports one bounded page
//! of the extension records the registry of the request's account and Region holds — the
//! ones this account registered or activated and the public extensions Amazon Web Services
//! and third-party publishers publish — each at its default version, keeping only the records
//! every filter the request gives admits, in an order that is a property of the records, with
//! `MaxResults` bounding the page and the `NextToken` resuming immediately past it.
//! `Visibility` narrows by the SCOPE a record is in: `PRIVATE` is what this account and
//! Region can use ("private extensions you have registered in this account and Region.
//! Public extensions that you have activated in this account and Region"), `PUBLIC` is what
//! is publicly visible and activatable.**
//!
//! The tables below are one row per way a request can narrow the listing (each visibility,
//! each extension kind, each deprecation status, each provisioning behaviour, each
//! `Category`, a publisher id, a name prefix, and a pair the model itself names as
//! incompatible) and one row per page size, over the registry fixture
//! `extension_registry.rs` seeds — records spanning every `RegistryType` and every
//! visibility a record can have — plus the Amazon-Web-Services-published catalogue every
//! account's registry holds without anybody registering it. The recorded trace this suite
//! was written for is ONE row of the paging table: a walk of the listing in pages.
//!
//! The registry's WRITERS (`RegisterType`, `ActivateType`, `DeregisterType`) are still
//! scaffold stubs with their own issues, so the stored records are seeded straight into the
//! store, exactly as `extension_registry.rs` seeds them. Every READ goes through the real
//! request path (form-encoded body in, XML envelope out), so what is asserted is what a
//! client sees.

use std::sync::Arc;

use service_com_amazonaws_cloudformation_cloudformation::state::publisher::{
    identity_provider, StoredPublisher,
};
use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    aws_published_resource_types, deprecated_status, provisioning_type, public_type_arn,
    published_provisioning_type, registry_type, type_arn, visibility, PUBLIC_EXTENSION_CATALOG,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{encode_next_token, PageOf};
use smithy_server_runtime::MultiServiceRouter;

use crate::extension_registry::{router_with_registry, OTHER_REGION, PUBLISHER_ID};
use crate::stack_lifecycle::{call, call_in, member, store, xml_blocks, xml_text, REGION};

/// The seeded private resource type, whose DEFAULT version is the deprecated one.
const PRIVATE_RESOURCE: &str = "My::Sample::Resource";
/// The seeded private module.
const PRIVATE_MODULE: &str = "My::Sample::Thing::MODULE";
/// The seeded private Hook.
const PRIVATE_HOOK: &str = "My::Sample::Hook";
/// The seeded public THIRD-PARTY extension, activated in this account under an alias.
const ACTIVATED_PUBLIC: &str = "MyAlias::Sample::Resource";
/// The seeded public extension that no third party published.
const PUBLIC_NO_PUBLISHER: &str = "AWS::Sample::Resource";
/// The seeded record of ANOTHER Region's registry.
const OTHER_REGION_RESOURCE: &str = "Other::Region::Resource";

/// A publisher id no seeded record was published under.
const OTHER_PUBLISHER_ID: &str = "fedcba654321";

/// How many pages a walk of one listing may take before the case gives up: a page carries
/// at least one item, so a walk longer than this is a cursor that is not advancing.
const PAGE_LIMIT: usize = 512;

/// The largest page `ListTypes` admits: `MaxResults` is `@range(min: 1, max: 100)`.
const MAX_RESULTS: usize = 100;

/// A router over the seeded registry, plus the publisher registration the seeded
/// third-party record was published under — `PublisherIdentity` is "the type of account
/// used as the identity provider when registering this publisher", which that registration
/// is what carries.
fn router_with_registry_and_publisher() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let (router, engine) = router_with_registry();
    store(&engine).lock().publisher = Some(StoredPublisher::new(PUBLISHER_ID.to_string(), None));
    (router, engine)
}

async fn list_types(router: &MultiServiceRouter, members: &[(&str, &str)]) -> (u16, String) {
    list_types_in(router, REGION, members).await
}

/// [`list_types`], signed for `region`: the registry is regional, so the credential scope
/// of the request is what decides which registry it reads.
async fn list_types_in(
    router: &MultiServiceRouter,
    region: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let members: Vec<(String, String)> = members
        .iter()
        .map(|(key, value)| member(key, value))
        .collect();
    call_in(router, region, "ListTypes", &members).await
}

/// Every summary the response carries, as its own XML block, in the order the response
/// carries them.
fn summaries(body: &str) -> Vec<String> {
    xml_blocks(body, "member")
}

/// The `TypeName` of every summary in a response, in the order the response carries them.
fn listed_names(body: &str) -> Vec<String> {
    summaries(body)
        .iter()
        .filter_map(|summary| xml_text(summary, "TypeName"))
        .collect()
}

/// Walk the whole listing the way a client does: one call, then one per `NextToken`, until
/// the response carries none. Answers every name in the order the pages carried them.
async fn walk_names(
    router: &MultiServiceRouter,
    region: &str,
    members: &[(&str, &str)],
) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..PAGE_LIMIT {
        let mut with_token: Vec<(&str, &str)> = members.to_vec();
        if let Some(token) = token.as_deref() {
            with_token.push(("NextToken", token));
        }
        let (status, body) = list_types_in(router, region, &with_token).await;
        assert_eq!(
            status, 200,
            "case {members:?}: expected HTTP 200 on page {page}, got {status} with {body:?}"
        );
        names.extend(listed_names(&body));
        token = xml_text(&body, "NextToken");
        if token.is_none() {
            return names;
        }
    }
    panic!("case {members:?}: the listing did not end within {PAGE_LIMIT} pages");
}

/// The Amazon-Web-Services-published extensions of the registry, in the order the listing
/// must report them (by type name).
///
/// Source: `crate::state::registry::aws_published_resource_types` (every resource type
/// Amazon Web Services provides a record for here — the ones this emulator provisions and
/// the ones no resource provider stands behind) and
/// `crate::state::registry::PUBLIC_EXTENSION_CATALOG` (the extensions PUBLISHED to the
/// registry, Amazon Web Services' own Hooks and the resource types third-party publishers
/// published) — the two catalogues
/// `crate::state::registry::aws_published` answers single reads from, so this suite and
/// `extension_registry.rs` cannot disagree about which published extensions exist.
fn published_catalog() -> Vec<String> {
    let mut names: Vec<String> = aws_published_resource_types()
        .map(|name| name.to_string())
        .chain(
            PUBLIC_EXTENSION_CATALOG
                .iter()
                .map(|published| published.type_name.to_string()),
        )
        .collect();
    names.sort();
    names
}

/// The resource types Amazon Web Services PROVIDES, which carry no publisher id: what a
/// listing narrowed to `AWS_TYPES` ("extensions available for use from Amazon") must report
/// of the published catalogue, since every extension PUBLISHED to the registry belongs to the
/// publisher that published it.
fn aws_provided_resource_types() -> Vec<String> {
    let mut names: Vec<String> = aws_published_resource_types()
        .map(|name| name.to_string())
        .collect();
    names.sort();
    names
}

/// Every extension PUBLISHED to the registry, whatever its kind: what a listing narrowed to
/// `THIRD_PARTY` reports of the catalogue, since each of those records carries the publisher
/// id of the publisher that published it.
fn published_catalog_by_a_publisher() -> Vec<String> {
    let mut names: Vec<String> = PUBLIC_EXTENSION_CATALOG
        .iter()
        .map(|published| published.type_name.to_string())
        .collect();
    names.sort();
    names
}

/// The published catalogue, narrowed to the extensions of ONE kind: what a listing filtered
/// by that `Type` must report of it.
fn published_catalog_of_kind(type_kind: &str) -> Vec<String> {
    let mut names: Vec<String> = aws_published_resource_types()
        .filter(|_| type_kind == registry_type::RESOURCE)
        .map(|name| name.to_string())
        .chain(
            PUBLIC_EXTENSION_CATALOG
                .iter()
                .filter(|published| published.type_kind == type_kind)
                .map(|published| published.type_name.to_string()),
        )
        .collect();
    names.sort();
    names
}

/// The published catalogue of RESOURCE types, narrowed to ONE provisioning behaviour: what
/// a listing filtered by that `ProvisioningType` must report of it.
///
/// Source: `crate::state::registry::published_provisioning_type`, the one place the class a
/// published resource type is reported in is decided, and the class each catalogued extension
/// carries itself (`PublishedExtension::provisioning_type`), so this suite and the records the
/// registry builds cannot disagree about which class a type is in.
fn published_catalog_provisioned(provisioning: &str) -> Vec<String> {
    let mut names: Vec<String> = aws_published_resource_types()
        .filter(|type_name| published_provisioning_type(type_name) == provisioning)
        .map(|name| name.to_string())
        .chain(
            PUBLIC_EXTENSION_CATALOG
                .iter()
                .filter(|published| published.provisioning_type == Some(provisioning))
                .map(|published| published.type_name.to_string()),
        )
        .collect();
    names.sort();
    names
}

/// Every name a `PUBLIC` listing of this Region must carry: the published catalogue plus
/// the seeded public record no third party published.
///
/// The seeded ACTIVATION is not among them: an activation is visible and usable within the
/// account that made it, which is the `PRIVATE` scope.
fn public_names() -> Vec<String> {
    let mut names = published_catalog();
    names.push(PUBLIC_NO_PUBLISHER.to_string());
    names.sort();
    names
}

/// The published catalogue, narrowed to the names a prefix admits.
fn published_catalog_under(prefix: &str) -> Vec<String> {
    published_catalog()
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .collect()
}

fn names(listed: &[&str]) -> Vec<String> {
    listed.iter().map(|name| name.to_string()).collect()
}

/// One row of the narrowing table: a request, and exactly the extensions it must report.
struct ListCase {
    case: &'static str,
    members: Vec<(&'static str, &'static str)>,
    listed: Vec<String>,
}

/// Every way a request narrows the listing.
fn list_cases() -> Vec<ListCase> {
    vec![
        ListCase {
            // "The default is PRIVATE" for `Visibility`, and the registry's deprecation
            // filter defaults to LIVE — so the seeded private resource type, whose DEFAULT
            // version is deprecated, is not among them. The activated public extension IS:
            // the private scope is what this account and Region can use, and "public
            // extensions that you have activated in this account and Region" are in it.
            case: "no filters at all: the account's own LIVE extensions",
            members: vec![],
            listed: names(&[PRIVATE_HOOK, PRIVATE_MODULE, ACTIVATED_PUBLIC]),
        },
        ListCase {
            case: "DeprecatedStatus=DEPRECATED: the record deregistered at its default version",
            members: vec![("DeprecatedStatus", deprecated_status::DEPRECATED)],
            listed: names(&[PRIVATE_RESOURCE]),
        },
        ListCase {
            case: "Visibility=PRIVATE, stated rather than defaulted",
            members: vec![("Visibility", visibility::PRIVATE)],
            listed: names(&[PRIVATE_HOOK, PRIVATE_MODULE, ACTIVATED_PUBLIC]),
        },
        ListCase {
            case: "Type=MODULE",
            members: vec![("Type", registry_type::MODULE)],
            listed: names(&[PRIVATE_MODULE]),
        },
        ListCase {
            case: "Type=HOOK",
            members: vec![("Type", registry_type::HOOK)],
            listed: names(&[PRIVATE_HOOK]),
        },
        ListCase {
            // The one private resource type is deprecated at its default version, so what
            // this kind filter and the deprecation default admit together is the activation,
            // which the private scope includes.
            case: "Type=RESOURCE, whose only private registration is deprecated",
            members: vec![("Type", registry_type::RESOURCE)],
            listed: names(&[ACTIVATED_PUBLIC]),
        },
        ListCase {
            case: "Type=RESOURCE with DeprecatedStatus=DEPRECATED",
            members: vec![
                ("Type", registry_type::RESOURCE),
                ("DeprecatedStatus", deprecated_status::DEPRECATED),
            ],
            listed: names(&[PRIVATE_RESOURCE]),
        },
        ListCase {
            case: "ProvisioningType=NON_PROVISIONABLE",
            members: vec![("ProvisioningType", provisioning_type::NON_PROVISIONABLE)],
            listed: names(&[PRIVATE_HOOK]),
        },
        ListCase {
            case: "ProvisioningType=IMMUTABLE, on the deprecated default version",
            members: vec![
                ("ProvisioningType", provisioning_type::IMMUTABLE),
                ("DeprecatedStatus", deprecated_status::DEPRECATED),
            ],
            listed: names(&[PRIVATE_RESOURCE]),
        },
        ListCase {
            // The module's version carries no provisioning behaviour, so a request that
            // asks for one does not report it; the activation's version carries this one.
            case: "ProvisioningType=FULLY_MUTABLE, which only the activation carries",
            members: vec![("ProvisioningType", provisioning_type::FULLY_MUTABLE)],
            listed: names(&[ACTIVATED_PUBLIC]),
        },
        ListCase {
            case: "Visibility=PUBLIC: the activated, the published and the catalogue",
            members: vec![("Visibility", visibility::PUBLIC)],
            listed: public_names(),
        },
        ListCase {
            // The public records of the seeded registry are resource types, as are the
            // resource types Amazon Web Services provides; the extensions it PUBLISHES are
            // of another kind, so this narrowing leaves them out.
            case: "Visibility=PUBLIC with Type=RESOURCE",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Type", registry_type::RESOURCE),
            ],
            listed: {
                let mut listed = published_catalog_of_kind(registry_type::RESOURCE);
                listed.push(PUBLIC_NO_PUBLISHER.to_string());
                listed.sort();
                listed
            },
        },
        ListCase {
            // The other side of the same narrowing: the Hooks the registry publishes are
            // public records too, and they are the only public records of their kind.
            case: "Visibility=PUBLIC with Type=HOOK: the Hooks the registry publishes",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Type", registry_type::HOOK),
            ],
            listed: published_catalog_of_kind(registry_type::HOOK),
        },
        ListCase {
            // Nothing publishes a public MODULE here — a module is authored by its
            // publisher — so the kind narrows the public listing to nothing.
            case: "Visibility=PUBLIC with Type=MODULE, which no public record is",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Type", registry_type::MODULE),
            ],
            listed: published_catalog_of_kind(registry_type::MODULE),
        },
        ListCase {
            // The provisioning behaviour narrows the PUBLIC resource types the same way:
            // a public resource type no resource provider stands behind is in the registry
            // as a `NON_PROVISIONABLE` record, which is how a client enumerating the
            // registry discovers one.
            case: "Visibility=PUBLIC, Type=RESOURCE, ProvisioningType=NON_PROVISIONABLE",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Type", registry_type::RESOURCE),
                ("ProvisioningType", provisioning_type::NON_PROVISIONABLE),
                ("DeprecatedStatus", deprecated_status::LIVE),
            ],
            listed: published_catalog_provisioned(provisioning_type::NON_PROVISIONABLE),
        },
        ListCase {
            // The other side of that narrowing: the published types a provider does
            // provision, plus the seeded public resource record, whose own version carries
            // that behaviour.
            case: "Visibility=PUBLIC, Type=RESOURCE, ProvisioningType=FULLY_MUTABLE",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Type", registry_type::RESOURCE),
                ("ProvisioningType", provisioning_type::FULLY_MUTABLE),
            ],
            listed: {
                let mut listed = published_catalog_provisioned(provisioning_type::FULLY_MUTABLE);
                listed.push(PUBLIC_NO_PUBLISHER.to_string());
                listed.sort();
                listed
            },
        },
        ListCase {
            // "AWS_TYPES: extensions available for use from Amazon" — the public records
            // no third party published.
            case: "Category=AWS_TYPES under PUBLIC",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Filters.Category", "AWS_TYPES"),
            ],
            listed: {
                // The resource types Amazon Web Services PROVIDES carry no publisher id,
                // which is what puts them in this category; the extensions PUBLISHED to the
                // registry belong to the publisher that published them, so they are in
                // THIRD_PARTY below.
                let mut listed = aws_provided_resource_types();
                listed.push(PUBLIC_NO_PUBLISHER.to_string());
                listed.sort();
                listed
            },
        },
        ListCase {
            // "ACTIVATED: public extensions that have been activated for this account and
            // Region" — which are in the PRIVATE scope, so the public listing holds none of
            // them even though the extension they were activated from is published.
            case: "Category=ACTIVATED under PUBLIC",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Filters.Category", "ACTIVATED"),
            ],
            listed: Vec::new(),
        },
        ListCase {
            // "THIRD_PARTY: extensions available for use from publishers other than
            // Amazon … public extensions from publishers other than Amazon, whether
            // activated or not."
            case: "Category=THIRD_PARTY under PUBLIC",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Filters.Category", "THIRD_PARTY"),
            ],
            // Every public record a PUBLISHER published, which is what carrying a publisher
            // id states: the extensions published to this registry, of either kind.
            listed: published_catalog_by_a_publisher(),
        },
        ListCase {
            // The same category under PRIVATE: "this includes private extensions
            // registered in the account", and the account's activation of a third party's
            // extension is in the private scope too.
            case: "Category=THIRD_PARTY under PRIVATE",
            members: vec![("Filters.Category", "THIRD_PARTY")],
            listed: names(&[PRIVATE_HOOK, PRIVATE_MODULE, ACTIVATED_PUBLIC]),
        },
        ListCase {
            // "REGISTERED: private extensions that have been registered for this account
            // and Region."
            case: "Category=REGISTERED under PRIVATE",
            members: vec![("Filters.Category", "REGISTERED")],
            listed: names(&[PRIVATE_HOOK, PRIVATE_MODULE]),
        },
        ListCase {
            case: "Category=REGISTERED under PUBLIC, which no public record can be",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Filters.Category", "REGISTERED"),
            ],
            listed: Vec::new(),
        },
        ListCase {
            // "ACTIVATED: public extensions that have been activated for this account and
            // Region" — reported in the private scope, which is the scope an activation puts
            // the extension into.
            case: "Category=ACTIVATED under PRIVATE",
            members: vec![("Filters.Category", "ACTIVATED")],
            listed: names(&[ACTIVATED_PUBLIC]),
        },
        ListCase {
            // The model's own example of an incompatible pair: "specifying AWS_TYPES for
            // Category and PRIVATE for Visibility returns an empty list of types, but
            // specifying PUBLIC for Visibility returns the desired list".
            case: "Category=AWS_TYPES under PRIVATE, the pair the model names",
            members: vec![("Filters.Category", "AWS_TYPES")],
            listed: Vec::new(),
        },
        ListCase {
            case: "Filters.PublisherId naming the publisher of the activated record",
            members: vec![
                ("Visibility", visibility::PRIVATE),
                ("Filters.PublisherId", PUBLISHER_ID),
            ],
            listed: names(&[ACTIVATED_PUBLIC]),
        },
        ListCase {
            case: "Filters.PublisherId naming a publisher that published nothing here",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Filters.PublisherId", OTHER_PUBLISHER_ID),
            ],
            listed: Vec::new(),
        },
        ListCase {
            case: "Filters.TypeNamePrefix over the account's own names",
            members: vec![("Filters.TypeNamePrefix", "My::")],
            listed: names(&[PRIVATE_HOOK, PRIVATE_MODULE]),
        },
        ListCase {
            case: "Filters.TypeNamePrefix over a service's published types",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Filters.TypeNamePrefix", "AWS::S3"),
            ],
            listed: published_catalog_under("AWS::S3"),
        },
        ListCase {
            case: "Filters.TypeNamePrefix over one published type",
            members: vec![
                ("Visibility", visibility::PUBLIC),
                ("Filters.TypeNamePrefix", "AWS::Logs"),
            ],
            listed: published_catalog_under("AWS::Logs"),
        },
        ListCase {
            // The record exists, in ANOTHER Region's registry.
            case: "Filters.TypeNamePrefix over a name only another Region holds",
            members: vec![("Filters.TypeNamePrefix", "Other::")],
            listed: Vec::new(),
        },
    ]
}

/// Each narrowing reports exactly the records the registry holds under it.
#[tokio::test]
async fn list_types_reports_the_records_every_filter_admits() {
    let (router, _engine) = router_with_registry_and_publisher();

    for ListCase {
        case,
        members,
        listed,
    } in list_cases()
    {
        let walked = walk_names(&router, REGION, &members).await;
        assert_eq!(
            walked, listed,
            "case {case:?}: expected the listing {listed:?}, got {walked:?}"
        );
    }
}

/// The registry is regional: the same read signed for another Region reports that
/// Region's records and none of this one's.
#[tokio::test]
async fn a_listing_reports_only_the_registry_of_the_region_it_was_signed_for() {
    let (router, _engine) = router_with_registry_and_publisher();

    let elsewhere = walk_names(&router, OTHER_REGION, &[]).await;
    assert_eq!(
        elsewhere,
        names(&[OTHER_REGION_RESOURCE]),
        "case \"another Region's registry\": expected only that Region's record, got \
         {elsewhere:?}"
    );

    // The published catalogue is in EVERY Region's registry, under that Region's own ARNs.
    let (status, body) = list_types_in(
        &router,
        OTHER_REGION,
        &[
            ("Visibility", visibility::PUBLIC),
            ("Filters.TypeNamePrefix", "AWS::Logs::LogGroup"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"another Region's published catalogue\": expected HTTP 200, got {status} with \
         {body:?}"
    );
    let expected = public_type_arn(OTHER_REGION, registry_type::RESOURCE, "AWS::Logs::LogGroup");
    let actual = xml_text(&body, "TypeArn");
    assert_eq!(
        actual.as_deref(),
        Some(expected.as_str()),
        "case \"another Region's published catalogue\": expected the ARN {expected:?}, got \
         {actual:?} from {body:?}"
    );
}

/// One row of the reporting table: the record to list, and the members its summary must
/// carry (`Some`) or must NOT carry at all (`None`).
struct SummaryCase {
    case: &'static str,
    members: Vec<(&'static str, String)>,
    reported: Vec<(&'static str, Option<String>)>,
}

fn present(value: &str) -> Option<String> {
    Some(value.to_string())
}

/// What a summary reports for a record of each visibility class. The activated
/// third-party record — the one row that carries every public-only member — has a case of
/// its own below.
fn summary_cases() -> Vec<SummaryCase> {
    vec![
        SummaryCase {
            case: "a private record, at its default version",
            members: vec![
                ("Filters.TypeNamePrefix", PRIVATE_RESOURCE.to_string()),
                ("DeprecatedStatus", deprecated_status::DEPRECATED.to_string()),
            ],
            reported: vec![
                ("Type", present(registry_type::RESOURCE)),
                ("TypeName", present(PRIVATE_RESOURCE)),
                (
                    "TypeArn",
                    present(&type_arn(REGION, registry_type::RESOURCE, PRIVATE_RESOURCE)),
                ),
                // "This applies only to private extensions you have registered in your
                // account."
                ("DefaultVersionId", present("00000002")),
                (
                    "Description",
                    present("Sample resource type, version 00000002"),
                ),
                // Public-only members, withheld from a private record.
                ("PublisherId", None),
                ("PublisherIdentity", None),
                ("IsActivated", None),
                ("OriginalTypeName", None),
                ("PublicVersionNumber", None),
                ("LatestPublicVersion", None),
            ],
        },
        SummaryCase {
            case: "a public record no third party published",
            members: vec![
                ("Visibility", visibility::PUBLIC.to_string()),
                ("Filters.TypeNamePrefix", PUBLIC_NO_PUBLISHER.to_string()),
            ],
            reported: vec![
                ("TypeName", present(PUBLIC_NO_PUBLISHER)),
                (
                    "TypeArn",
                    present(&type_arn(
                        REGION,
                        registry_type::RESOURCE,
                        PUBLIC_NO_PUBLISHER,
                    )),
                ),
                // "For public extensions that have been activated for this account and
                // Region, the version of the public extension to be used."
                ("PublicVersionNumber", present("1.0.0")),
                // Not a THIRD-PARTY extension: no publisher, no activation, and no latest
                // available version ("for any extensions other than activated third-party
                // extensions, CloudFormation returns null").
                ("PublisherId", None),
                ("PublisherIdentity", None),
                ("IsActivated", None),
                ("LatestPublicVersion", None),
                // Public, so no default version id; and neither private nor
                // auto-updating, so no registration instant.
                ("DefaultVersionId", None),
                ("LastUpdated", None),
            ],
        },
        SummaryCase {
            case: "an Amazon-Web-Services-published record of the catalogue",
            members: vec![
                ("Visibility", visibility::PUBLIC.to_string()),
                ("Filters.TypeNamePrefix", "AWS::Logs::LogGroup".to_string()),
            ],
            reported: vec![
                ("Type", present(registry_type::RESOURCE)),
                ("TypeName", present("AWS::Logs::LogGroup")),
                (
                    "TypeArn",
                    present(&public_type_arn(
                        REGION,
                        registry_type::RESOURCE,
                        "AWS::Logs::LogGroup",
                    )),
                ),
                ("DefaultVersionId", None),
                ("PublisherId", None),
                ("PublisherIdentity", None),
                ("IsActivated", None),
                ("LatestPublicVersion", None),
                ("PublicVersionNumber", None),
                ("LastUpdated", None),
            ],
        },
    ]
}

/// A summary reports the record's own members, gated on its visibility exactly as the
/// model documents each one.
#[tokio::test]
async fn a_summary_reports_the_members_the_records_visibility_admits() {
    let (router, _engine) = router_with_registry_and_publisher();

    for SummaryCase {
        case,
        members,
        reported,
    } in summary_cases()
    {
        let members: Vec<(&str, &str)> = members
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        let (status, body) = list_types(&router, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let summaries = summaries(&body);
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: expected exactly one summary, got {summaries:?} from {body:?}"
        );
        for (tag, expected) in reported {
            let actual = xml_text(&summaries[0], tag);
            assert_eq!(
                actual.as_deref(),
                expected.as_deref(),
                "case {case:?}: expected <{tag}> to be {expected:?}, got {actual:?} from \
                 {body:?}"
            );
        }
    }
}

/// The activated third-party record is the one row that carries every public-only member,
/// including the identity its publisher's own registration was verified by.
#[tokio::test]
async fn an_activated_third_party_summary_reports_its_publisher_and_versions() {
    let (router, _engine) = router_with_registry_and_publisher();

    let (status, body) = list_types(
        &router,
        &[
            // The activation is in the PRIVATE scope — "public extensions that you have
            // activated in this account and Region" — and still carries every member the
            // model reports for a public third-party extension.
            ("Visibility", visibility::PRIVATE),
            ("Filters.TypeNamePrefix", ACTIVATED_PUBLIC),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"activated third-party\": expected HTTP 200, got {status} with {body:?}"
    );
    let summaries = summaries(&body);
    assert_eq!(
        summaries.len(),
        1,
        "case \"activated third-party\": expected exactly one summary, got {summaries:?}"
    );
    for (tag, expected) in [
        ("TypeName", present(ACTIVATED_PUBLIC)),
        (
            "TypeArn",
            present(&type_arn(REGION, registry_type::RESOURCE, ACTIVATED_PUBLIC)),
        ),
        ("PublisherId", present(PUBLISHER_ID)),
        ("PublisherIdentity", present(identity_provider::AWS_MARKETPLACE)),
        ("IsActivated", present("true")),
        ("OriginalTypeName", present("Publisher::Sample::Resource")),
        ("PublicVersionNumber", present("1.4.0")),
        ("LatestPublicVersion", present("2.1.0")),
        ("Description", present("A third party's resource type")),
        // Public, so no default version id however the store holds one.
        ("DefaultVersionId", None),
    ] {
        let actual = xml_text(&summaries[0], tag);
        assert_eq!(
            actual.as_deref(),
            expected.as_deref(),
            "case \"activated third-party\": expected <{tag}> to be {expected:?}, got \
             {actual:?} from {body:?}"
        );
    }
    // "Public extensions you have activated in your account with auto-update specified"
    // report when the version was registered; the seeded record activated one that way.
    let last_updated = xml_text(&summaries[0], "LastUpdated");
    assert!(
        last_updated.is_some_and(|instant| instant.starts_with("2023-11-14T22:")),
        "case \"activated third-party\": expected the seeded registration instant, got \
         {body:?}"
    );
}

/// `MaxResults` bounds one page, and the `NextToken` resumes immediately past it: whatever
/// page size a caller chooses, the pages partition the same listing in the same order.
#[tokio::test]
async fn a_page_carries_at_most_max_results_and_the_token_resumes_after_it() {
    let (router, _engine) = router_with_registry_and_publisher();
    let whole = public_names();
    assert!(
        whole.len() > 2,
        "case \"paging\": the fixture must hold more records than a page size, got {whole:?}"
    );

    // The modelled `MaxResults` range is 1..100: both published bounds, and sizes between
    // them that do and do not divide the listing. A listing LONGER than the largest page
    // the model admits is paged by that bound rather than by its own length, which is what
    // the `MAX_RESULTS` row below exercises.
    let whole_page = whole.len().clamp(2, MAX_RESULTS);
    for max_results in [1usize, 2, 7, whole_page - 1, whole_page, MAX_RESULTS] {
        let max = max_results.to_string();
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut members: Vec<(&str, &str)> =
                vec![("Visibility", visibility::PUBLIC), ("MaxResults", &max)];
            if let Some(token) = token.as_deref() {
                members.push(("NextToken", token));
            }
            let (status, body) = list_types(&router, &members).await;
            assert_eq!(
                status, 200,
                "case {max_results}: expected HTTP 200 on page {pages}, got {status} with \
                 {body:?}"
            );
            let page = listed_names(&body);
            assert!(
                page.len() <= max_results,
                "case {max_results}: a page carries at most {max_results} summaries, got \
                 {page:?}"
            );
            walked.extend(page.clone());
            token = xml_text(&body, "NextToken");
            pages += 1;
            match &token {
                // Every page but the last is FULL: a token means more records remain, so a
                // short page beside one would lose or repeat records.
                Some(_) => assert_eq!(
                    page.len(),
                    max_results,
                    "case {max_results}: expected a full page before a NextToken, got \
                     {page:?}"
                ),
                None => break,
            }
            assert!(
                pages < PAGE_LIMIT,
                "case {max_results}: the listing did not end within {PAGE_LIMIT} pages"
            );
        }
        assert_eq!(
            walked, whole,
            "case {max_results}: expected the pages to partition the listing {whole:?}, got \
             {walked:?}"
        );
        let expected_pages = whole.len().div_ceil(max_results);
        assert_eq!(
            pages, expected_pages,
            "case {max_results}: expected {expected_pages} pages over {} records, got {pages}",
            whole.len()
        );
    }
}

/// A `MaxResults` outside the modelled range is refused, at each published bound and one
/// past it.
#[tokio::test]
async fn max_results_is_held_to_its_modelled_range() {
    let (router, _engine) = router_with_registry_and_publisher();

    // `MaxResults` is `@range(min: 1, max: 100)`.
    for (case, max_results, accepted) in [
        ("the published minimum", "1", true),
        ("below the published minimum", "0", false),
        ("the published maximum", "100", true),
        ("past the published maximum", "101", false),
    ] {
        let (status, body) = list_types(
            &router,
            &[
                ("Visibility", visibility::PUBLIC),
                ("MaxResults", max_results),
            ],
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: MaxResults={max_results} is within the range, got {status} \
                 with {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: MaxResults={max_results} is outside the range, got {status} \
                 with {body:?}"
            );
        }
    }
}

/// A `NextToken` this listing never issued is a bad request, not an empty page — including
/// one this service issued for ANOTHER collection.
#[tokio::test]
async fn a_next_token_this_listing_never_issued_is_refused() {
    let (router, _engine) = router_with_registry_and_publisher();

    let another_collection = encode_next_token(PageOf::StackSummaries, 0);
    let past_the_end = encode_next_token(PageOf::TypeSummaries, 10_000);
    for (case, token) in [
        ("a token this service never issued", "not-a-token-we-issued"),
        ("a token issued for another collection", &another_collection),
        ("a cursor past the end of the listing", &past_the_end),
    ] {
        let (status, body) = list_types(
            &router,
            &[("Visibility", visibility::PUBLIC), ("NextToken", token)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected <Code>ValidationError</Code>, got {body:?}"
        );
    }
}

/// Every record the listing reports is addressable through `DescribeType`, at the same
/// extension: the listing and the detailed read are two views of ONE registry.
#[tokio::test]
async fn every_listed_record_is_describable_at_the_arn_it_reported() {
    let (router, _engine) = router_with_registry_and_publisher();

    for (case, members) in [
        (
            "the activated third-party record",
            vec![
                ("Visibility", visibility::PRIVATE),
                ("Filters.TypeNamePrefix", ACTIVATED_PUBLIC),
            ],
        ),
        (
            "a published record of the catalogue",
            vec![
                ("Visibility", visibility::PUBLIC),
                ("Filters.TypeNamePrefix", "AWS::Logs::LogGroup"),
            ],
        ),
        (
            "the deprecated private record",
            vec![
                ("DeprecatedStatus", deprecated_status::DEPRECATED),
                ("Filters.TypeNamePrefix", PRIVATE_RESOURCE),
            ],
        ),
        ("a private Hook", vec![("Type", registry_type::HOOK)]),
    ] {
        let (status, body) = list_types(&router, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let summary = summaries(&body)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("case {case:?}: expected one summary, got {body:?}"));
        let type_kind = xml_text(&summary, "Type")
            .unwrap_or_else(|| panic!("case {case:?}: expected a <Type>, got {summary:?}"));
        let type_name = xml_text(&summary, "TypeName")
            .unwrap_or_else(|| panic!("case {case:?}: expected a <TypeName>, got {summary:?}"));
        let type_arn = xml_text(&summary, "TypeArn")
            .unwrap_or_else(|| panic!("case {case:?}: expected a <TypeArn>, got {summary:?}"));

        let (status, described) = call(
            &router,
            "DescribeType",
            &[member("Type", &type_kind), member("TypeName", &type_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeType of {type_name:?} to answer HTTP 200, got \
             {status} with {described:?}"
        );
        // `DescribeType` reports the ARN of the VERSION it describes, which is the
        // extension's own ARN with the version id appended where the record carries one.
        let described_arn = xml_text(&described, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: expected an <Arn>, got {described:?}"));
        assert!(
            described_arn == type_arn
                || described_arn.starts_with(&format!("{type_arn}/")),
            "case {case:?}: expected DescribeType to describe the listed extension \
             {type_arn:?}, got {described_arn:?}"
        );
        assert_eq!(
            xml_text(&described, "TypeName").as_deref(),
            Some(type_name.as_str()),
            "case {case:?}: expected DescribeType to report the listed name, got \
             {described:?}"
        );
    }
}

/// `ListTypes` is a report: no listing — narrowed, paged, empty or refused — changes what a
/// later caller sees.
#[tokio::test]
async fn a_listing_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_registry_and_publisher();
    let before = format!("{:?}", store(&engine).lock().extensions);

    let reads: Vec<Vec<(&str, &str)>> = vec![
        vec![],
        vec![("Visibility", visibility::PUBLIC), ("MaxResults", "1")],
        vec![("Filters.Category", "AWS_TYPES")],
        vec![("NextToken", "not-a-token-we-issued")],
        vec![("Type", registry_type::HOOK)],
    ];
    // Twice over, so a read that consumed or moved something would show on the second.
    for _ in 0..2 {
        for members in &reads {
            list_types(&router, members).await;
        }
    }

    let after = format!("{:?}", store(&engine).lock().extensions);
    assert_eq!(
        before, after,
        "case \"listings are reports\": expected the registry to be untouched, got {after:?} \
         from {before:?}"
    );
    let first = walk_names(&router, REGION, &[("Visibility", visibility::PUBLIC)]).await;
    let second = walk_names(&router, REGION, &[("Visibility", visibility::PUBLIC)]).await;
    assert_eq!(
        first, second,
        "case \"listings are reports\": expected two walks of one listing to agree, got \
         {first:?} and {second:?}"
    );
}
