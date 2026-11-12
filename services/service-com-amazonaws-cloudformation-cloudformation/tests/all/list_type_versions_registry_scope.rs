// SPDX-License-Identifier: CC0-1.0

//! What `ListTypeVersions` is served from, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **`ListTypeVersions` summarises the VERSION
//! records this account and Region's registry holds for the identified extension — the records
//! `RegisterType` writes for a private extension and `ActivateType` writes for a public one
//! this account activated — so an extension the registry holds no version record of is
//! refused with the operation's ONLY modelled error, `CFNRegistryException` (HTTP 400),
//! whatever else the registry reports about the extension itself.**
//!
//! The public extensions that are in every account and Region's registry without anybody
//! registering them are the class this distinguishes: `DescribeType` describes one and
//! `ListTypes` lists it, because the extension IS in the public registry, while nothing ever
//! added a version of it to THIS account and Region — so the version listing has nothing to
//! report and refuses, and the same listing answers as soon as an activation writes a version
//! of it.
//!
//! The table spans what the rule generalises over: every source of a public record the
//! registry reports without a registration (the resource types this emulator provisions, the
//! ones no resource provider stands behind, and the extensions the registry's own public
//! catalogue carries — Hooks and third-party resource types alike), each of the two ways a
//! request may identify an extension (`Type` + `TypeName`, and the ARN the registry itself
//! reports for it), a name no registry record of any kind carries, a private registration
//! another Region holds, and — on the listing side — a registration with more than one
//! version, a registration whose versions are all deprecated, and a publisher the extension
//! was not published by.
//!
//! Every catalogue a row is drawn from is READ from the source that defines it
//! (`state::template::IMPLEMENTED_RESOURCE_TYPES`,
//! `service_common::cfn_provisioning::NON_PROVISIONABLE_RESOURCE_TYPES`,
//! `state::registry::PUBLIC_EXTENSION_CATALOG`) rather than restated here, so a row cannot
//! drift from what the registry holds.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::registry::{
    self, deprecated_status, provisioning_type, registry_type, type_version_arn,
    PUBLIC_EXTENSION_CATALOG,
};
use service_com_amazonaws_cloudformation_cloudformation::state::template::IMPLEMENTED_RESOURCE_TYPES;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, call_in, member, router_with_engine, store, xml_blocks, xml_text, REGION};

/// The error code the registry's own modelled refusal carries on the wire
/// (`CFNRegistryException`'s `awsQueryError`), which is the only error `ListTypeVersions`
/// models.
const REGISTRY_REFUSAL_CODE: &str = "CFNRegistryException";

/// The message the registry answers a request for the versions of an extension it holds no
/// version record of. Entirely FIXED text: it names neither the `TypeName` nor the `Arn` the
/// request carried.
const TYPE_DOES_NOT_EXIST: &str = "The type does not exist";

/// A Region other than the one the requests are signed for, so a record it holds is not
/// addressable from [`REGION`].
const OTHER_REGION: &str = "eu-west-1";

/// One `ListTypeVersions` read: the HTTP status, the body, and the version ids the summaries
/// report in wire order.
async fn list_type_versions(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String, Vec<String>) {
    let (status, body) = call(router, "ListTypeVersions", members).await;
    // The summaries are read INSIDE their own block, so an element of another list the
    // envelope carries cannot be swept up with them.
    let version_ids = match xml_blocks(&body, "TypeVersionSummaries").into_iter().next() {
        Some(block) => xml_blocks(&block, "member")
            .into_iter()
            .map(|summary| xml_text(&summary, "VersionId").unwrap_or_default())
            .collect(),
        None => Vec::new(),
    };
    (status, body, version_ids)
}

/// Hold one `ListTypeVersions` request to the registry's modelled refusal: HTTP 400, the
/// registry's own code, the fixed message, and no summary at all.
async fn assert_refused(case: &str, router: &MultiServiceRouter, members: &[(String, String)]) {
    let (status, body, listed) = list_type_versions(router, members).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the version listing to be refused, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some(REGISTRY_REFUSAL_CODE),
        "case {case:?}: expected the registry's own refusal code, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some(TYPE_DOES_NOT_EXIST),
        "case {case:?}: expected {TYPE_DOES_NOT_EXIST:?}, got {body}"
    );
    assert!(
        listed.is_empty(),
        "case {case:?}: a refusal reports no summaries, got {listed:?}"
    );
}

/// One public extension the registry reports without anybody registering it: its kind, its
/// name, and where the row was drawn from.
#[derive(Debug, Clone)]
struct PublicRecord {
    case: String,
    type_kind: &'static str,
    type_name: &'static str,
}

/// Every such extension a row is drawn from, read out of the catalogues the registry resolves
/// them from.
fn public_records() -> Vec<PublicRecord> {
    let provisioned = IMPLEMENTED_RESOURCE_TYPES;
    let mut rows = Vec::new();
    // The resource types this emulator provisions: the FIRST and the LAST of the catalogue,
    // so no row is the one a single trace happened to name.
    for (case, type_name) in [
        ("the first resource type the registry provides", provisioned.first()),
        ("the last resource type the registry provides", provisioned.last()),
    ] {
        let type_name = type_name.expect("the registry provides at least one resource type");
        rows.push(PublicRecord {
            case: format!("{case} ({type_name})"),
            type_kind: registry_type::RESOURCE,
            type_name,
        });
    }
    // A resource type no resource provider stands behind, which the registry still reports —
    // in the provisioning class that says so.
    for type_name in service_common::cfn_provisioning::NON_PROVISIONABLE_RESOURCE_TYPES
        .iter()
        .take(1)
    {
        rows.push(PublicRecord {
            case: format!("a type no resource provider stands behind ({type_name})"),
            type_kind: registry_type::RESOURCE,
            type_name,
        });
    }
    // The registry's own public catalogue: Hooks and third-party resource types alike.
    for published in PUBLIC_EXTENSION_CATALOG {
        rows.push(PublicRecord {
            case: format!(
                "a catalogued public extension ({} {})",
                published.type_kind, published.type_name
            ),
            type_kind: published.type_kind,
            type_name: published.type_name,
        });
    }
    rows
}

/// The ARN the registry itself reports for `row`, read back through `DescribeType` — which
/// also pins that the extension IS in this registry, so the version listing's refusal is
/// about the VERSION records and not about an unknown name.
async fn described_arn(router: &MultiServiceRouter, row: &PublicRecord) -> String {
    let members = [
        member("Type", row.type_kind),
        member("TypeName", row.type_name),
    ];
    let (status, body) = call(router, "DescribeType", &members).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the public extension to be in the registry, got {status} {body}",
        row.case
    );
    xml_text(&body, "Arn").unwrap_or_else(|| {
        panic!(
            "case {:?}: expected the described extension to report its ARN, got {body}",
            row.case
        )
    })
}

#[tokio::test]
async fn an_extension_the_registry_holds_no_version_of_is_not_listable() {
    let (router, _engine) = router_with_engine();

    for row in public_records() {
        let arn = described_arn(&router, &row).await;
        // Named by its kind and name …
        assert_refused(
            &format!("{} by kind and name", row.case),
            &router,
            &[
                member("Type", row.type_kind),
                member("TypeName", row.type_name),
            ],
        )
        .await;
        // … and by the ARN the registry reports for it.
        assert_refused(
            &format!("{} by its own ARN", row.case),
            &router,
            &[member("Arn", &arn)],
        )
        .await;
        // The refusal does not echo what the request named.
        let (_, body, _) = list_type_versions(
            &router,
            &[
                member("Type", row.type_kind),
                member("TypeName", row.type_name),
            ],
        )
        .await;
        assert!(
            !body.contains(row.type_name),
            "case {:?}: the refusal must not echo the name the request carried, got {body}",
            row.case
        );
    }

    // A name no catalogue and no registration carries at all is the same refusal: the rule is
    // about the version records, so it cannot distinguish the two.
    assert_refused(
        "a name no registry record carries",
        &router,
        &[
            member("Type", registry_type::RESOURCE),
            member("TypeName", "No::Such::Resource"),
        ],
    )
    .await;
    assert_refused(
        "a module name nothing registered",
        &router,
        &[
            member("Type", registry_type::MODULE),
            member("TypeName", "No::Such::Fragment::MODULE"),
        ],
    )
    .await;
}

#[tokio::test]
async fn an_activation_is_what_makes_a_public_extension_listable() {
    // Every kind the public catalogue carries, so the rule is not asserted on one of them.
    for published in PUBLIC_EXTENSION_CATALOG {
        let (router, _engine) = router_with_engine();
        let case = format!("{} {}", published.type_kind, published.type_name);
        let publisher_id = registry::published_publisher_id(published.publisher_name);
        let named = [
            member("Type", published.type_kind),
            member("TypeName", published.type_name),
        ];

        // Before the activation: the registry holds no version of it in this account.
        assert_refused(&format!("{case}, before the activation"), &router, &named).await;

        let mut activation = vec![
            member("Type", published.type_kind),
            member("TypeName", published.type_name),
            member("PublisherId", &publisher_id),
        ];
        // "Please provide execution role to activate the Resource type": a resource type's
        // handlers are invoked on the activating account's behalf, so its activation names
        // one, while a Hook's does not.
        if published.type_kind == registry_type::RESOURCE {
            activation.push(member(
                "ExecutionRoleArn",
                "arn:aws:iam::000000000000:role/extension-execution",
            ));
        }
        let (status, body) = call(&router, "ActivateType", &activation).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the activation to succeed, got {status} {body}"
        );
        let activated_arn = xml_text(&body, "Arn")
            .unwrap_or_else(|| panic!("case {case:?}: expected the activation's ARN, got {body}"));

        // After it: the activation wrote a version of the extension, and that version is what
        // the listing reports — by the name and by the ARN the activation answered.
        for (form, members) in [
            ("by kind and name", named.to_vec()),
            ("by the activation's ARN", vec![member("Arn", &activated_arn)]),
        ] {
            let (status, body, listed) = list_type_versions(&router, &members).await;
            assert_eq!(
                status, 200,
                "case {case:?} {form}: expected the activated extension to be listable, got \
                 {status} {body}"
            );
            assert_eq!(
                listed.len(),
                1,
                "case {case:?} {form}: expected the one version the activation wrote, got \
                 {listed:?} in {body}"
            );
            // The summary names that version's own ARN, built on the extension's.
            let summary_arns = match xml_blocks(&body, "TypeVersionSummaries").into_iter().next() {
                Some(block) => xml_blocks(&block, "member")
                    .into_iter()
                    .map(|summary| xml_text(&summary, "Arn").unwrap_or_default())
                    .collect::<Vec<_>>(),
                None => Vec::new(),
            };
            assert_eq!(
                summary_arns,
                vec![type_version_arn(&activated_arn, &listed[0])],
                "case {case:?} {form}: expected the version's own ARN, got {summary_arns:?} in \
                 {body}"
            );
        }
    }
}

/// A private registration of `region` holding `version_ids`, the first its default version:
/// the record a registration writes, seeded straight into the store — the device
/// `type_default_version.rs` and `type_deactivation.rs` use for a registry a case needs more
/// records in than a bound's worth of wire calls would create.
fn registration(
    region: &str,
    type_kind: &str,
    type_name: &str,
    version_ids: &[&str],
) -> registry::StoredExtension {
    let mut extension = registry::StoredExtension::new(region, type_kind, type_name);
    let arn = extension.arn.clone();
    extension.default_version_id = version_ids.first().map(|id| (*id).to_string());
    extension.versions = version_ids
        .iter()
        .enumerate()
        .map(|(position, version_id)| {
            let registered_at = SystemTime::UNIX_EPOCH
                + Duration::from_secs(1_700_000_000 + position as u64 * 60);
            let mut version =
                registry::StoredTypeVersion::new(&arn, version_id, registered_at);
            version.provisioning_type = Some(provisioning_type::FULLY_MUTABLE.to_string());
            version
        })
        .collect();
    extension
}

#[tokio::test]
async fn the_versions_a_registration_holds_are_what_the_listing_reports() {
    let (router, engine) = router_with_engine();

    // A registration with three versions, one with a single version, one whose versions are
    // all deprecated, and one ANOTHER Region holds.
    let three = registration(
        REGION,
        registry_type::RESOURCE,
        "My::Scoped::Resource",
        &["00000001", "00000002", "00000003"],
    );
    let one = registration(
        REGION,
        registry_type::HOOK,
        "My::Scoped::Hook",
        &["00000007"],
    );
    let mut deprecated = registration(
        REGION,
        registry_type::MODULE,
        "My::Scoped::Fragment::MODULE",
        &["00000001"],
    );
    deprecated.deprecate_every_version();
    let elsewhere = registration(
        OTHER_REGION,
        registry_type::RESOURCE,
        "My::Elsewhere::Resource",
        &["00000001"],
    );
    let elsewhere_name = elsewhere.type_name.clone();
    {
        let state = store(&engine);
        state.lock().extensions = vec![
            three.clone(),
            one.clone(),
            deprecated.clone(),
            elsewhere.clone(),
        ];
    }

    // Every version the record holds, by kind and name and by the record's own ARN.
    for (case, extension, expected) in [
        (
            "a registration holding three versions",
            &three,
            vec![
                "00000001".to_string(),
                "00000002".to_string(),
                "00000003".to_string(),
            ],
        ),
        (
            "a registration holding one version",
            &one,
            vec!["00000007".to_string()],
        ),
    ] {
        for (form, members) in [
            (
                "by kind and name",
                vec![
                    member("Type", &extension.type_kind),
                    member("TypeName", &extension.type_name),
                ],
            ),
            ("by its own ARN", vec![member("Arn", &extension.arn)]),
        ] {
            let (status, body, listed) = list_type_versions(&router, &members).await;
            assert_eq!(
                status, 200,
                "case {case:?} {form}: expected the listing to succeed, got {status} {body}"
            );
            assert_eq!(
                listed, expected,
                "case {case:?} {form}: expected {expected:?}, got {listed:?} in {body}"
            );
        }
    }

    // `DeprecatedStatus` NARROWS a listing — it is not a refusal: the record holds a version
    // record, so the read answers, with the versions in the status asked for.
    for (case, status_filter, expected) in [
        (
            "a fully deprecated registration, asked for the live versions",
            deprecated_status::LIVE,
            Vec::new(),
        ),
        (
            "a fully deprecated registration, asked for the deprecated ones",
            deprecated_status::DEPRECATED,
            vec!["00000001".to_string()],
        ),
    ] {
        let (status, body, listed) = list_type_versions(
            &router,
            &[
                member("Type", &deprecated.type_kind),
                member("TypeName", &deprecated.type_name),
                member("DeprecatedStatus", status_filter),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the listing to succeed, got {status} {body}"
        );
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?}, got {listed:?} in {body}"
        );
    }

    // The registry is REGIONAL: a record another Region holds is no record here, while the
    // same request signed for that Region lists it.
    assert_refused(
        "a registration another Region holds",
        &router,
        &[
            member("Type", registry_type::RESOURCE),
            member("TypeName", &elsewhere_name),
        ],
    )
    .await;
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListTypeVersions",
        &[
            member("Type", registry_type::RESOURCE),
            member("TypeName", &elsewhere_name),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the Region that holds it\": expected the listing to succeed, got {status} {body}"
    );

    // "Extensions published by Amazon aren't assigned a publisher ID": a publisher the record
    // was not published by names no extension of this registry.
    assert_refused(
        "a publisher the registration was not published by",
        &router,
        &[
            member("Type", &three.type_kind),
            member("TypeName", &three.type_name),
            member("PublisherId", "abcdef0123456789abcd"),
        ],
    )
    .await;
}
