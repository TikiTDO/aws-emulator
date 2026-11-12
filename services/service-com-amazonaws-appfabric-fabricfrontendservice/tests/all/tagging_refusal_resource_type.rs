// SPDX-License-Identifier: CC0-1.0

//! The `resourceType` the three tagging operations report when the
//! `resourceArn` they were given names no resource this service holds, driven
//! over the wire (restJson1) through the same router the emulator boots.
//!
//! The RULE under test: a refusal reports a kind of resource only when the ARN
//! is in THIS service's namespace (its third field is the service's ARN
//! namespace) and the last `<type>/<id>` pair of its resource field names a kind
//! this service mints; every other value — another service's ARN, a value that
//! is not a well-formed ARN, or one of this service's own naming a kind it does
//! not have — names no resource here and is reported as the generic `Resource`,
//! whatever text it happens to carry.
//!
//! The table therefore spans the namespace field, the type segment, the depth of
//! the chain, the pair separator and the slot a type NAME sits in, and runs every
//! row through all three operations: they share one refusal builder, so the rule
//! holds whichever of them was called.

use serde_json::Value;

use crate::app_bundle_ingestion_lifecycle::{unknown_uuid, Answer, Emulator};
use crate::resource_tags::{list_tags, tag_list, tag_resource, untag_resource};

/// An `appfabric` ARN for `resource`, in the account and partition every ARN a
/// generated emulator mints carries.
fn appfabric_arn(resource: &str) -> String {
    format!("arn:aws:appfabric:us-east-1:000000000000:{resource}")
}

/// The `resourceType` member of a not-found refusal, after checking the refusal
/// itself is the modelled one (HTTP 404 `ResourceNotFoundException`) and that it
/// reports the ARN the caller supplied.
fn refused_resource_type(answer: &Answer, case: &str, arn: &str) -> String {
    assert_eq!(
        answer.status, 404,
        "case {case:?}: expected HTTP 404, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().ends_with("ResourceNotFoundException"),
        "case {case:?}: expected ResourceNotFoundException, got {}",
        answer.body
    );
    assert_eq!(
        answer.body.get("resourceId").and_then(Value::as_str),
        Some(arn),
        "case {case:?}: expected the supplied ARN as resourceId, got {}",
        answer.body
    );
    answer
        .body
        .get("resourceType")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a resourceType member, got {}",
                answer.body
            )
        })
        .to_string()
}

/// RULE: the kind a tagging refusal reports is read from the ARN only when the
/// ARN is this service's own and names one of its kinds — anything else is the
/// generic `Resource`.
///
/// The type segments in the rows are the ones this service's create operations
/// build their ARNs from (`appbundle/<id>`, `…/ingestion/<id>`,
/// `…/ingestiondestination/<id>`, `…/appauthorization/<id>`); the generic value
/// is the one `ResourceNotFoundException$resourceType` carries for an ARN that
/// names nothing here. Every id below is one no create call ever minted, so each
/// row reaches the refusal path.
#[tokio::test]
async fn a_refusal_reports_a_kind_only_for_this_services_own_arns() {
    let bundle = unknown_uuid();
    let child = "11111111-1111-4111-8111-111111111111";
    let grandchild = "22222222-2222-4222-8222-222222222222";

    // (case, resourceArn, the resourceType the refusal must report)
    let cases: Vec<(&str, String, &str)> = vec![
        // This service's own ARNs, one per kind it mints, at every depth of the
        // ownership chain.
        (
            "an app bundle ARN",
            appfabric_arn(&format!("appbundle/{bundle}")),
            "AppBundle",
        ),
        (
            "an ingestion ARN",
            appfabric_arn(&format!("appbundle/{bundle}/ingestion/{child}")),
            "Ingestion",
        ),
        (
            "an ingestion destination ARN",
            appfabric_arn(&format!(
                "appbundle/{bundle}/ingestion/{child}/ingestiondestination/{grandchild}"
            )),
            "IngestionDestination",
        ),
        (
            "an app authorization ARN",
            appfabric_arn(&format!("appbundle/{bundle}/appauthorization/{child}")),
            "AppAuthorization",
        ),
        // The pair written with a colon rather than a slash: AWS writes a
        // resource in either form.
        (
            "an app bundle ARN whose type and id are colon-separated",
            appfabric_arn(&format!("appbundle:{bundle}")),
            "AppBundle",
        ),
        // Region and account may be empty in a well-formed ARN, and neither
        // decides which kind the ARN names.
        (
            "an app bundle ARN with no region and no account",
            "arn:aws:appfabric:::appbundle/".to_string() + &bundle,
            "AppBundle",
        ),
        // A type name in an ID slot is an identifier, not a type.
        (
            "an app bundle whose id reads like a child type name",
            appfabric_arn("appbundle/ingestion"),
            "AppBundle",
        ),
        (
            "an ingestion whose id reads like the destination type name",
            appfabric_arn(&format!("appbundle/{bundle}/ingestion/ingestiondestination")),
            "Ingestion",
        ),
        // A type segment with no id: the ARN still names that KIND of resource
        // (the type segment is what says which), so that is what is reported.
        (
            "an ARN naming the app bundle type with no id",
            appfabric_arn("appbundle"),
            "AppBundle",
        ),
        (
            "an ARN naming the ingestion type with no id",
            appfabric_arn(&format!("appbundle/{bundle}/ingestion")),
            "Ingestion",
        ),
        // A separator doubled or trailing spells the same chain: an empty segment
        // is no segment, so it takes no slot in the type/id alternation.
        (
            "an app bundle ARN written with a doubled separator",
            appfabric_arn(&format!("appbundle//{bundle}")),
            "AppBundle",
        ),
        (
            "an ingestion ARN written with a trailing separator",
            appfabric_arn(&format!("appbundle/{bundle}/ingestion/{child}/")),
            "Ingestion",
        ),
        // This service's namespace, a kind it does not have.
        (
            "an appfabric ARN of a kind this service does not have",
            appfabric_arn("widget/1"),
            "Resource",
        ),
        (
            "an appfabric ARN whose deepest kind this service does not have",
            appfabric_arn(&format!("appbundle/{bundle}/widget/1")),
            "Resource",
        ),
        // Another service's ARN, including ones whose text carries this
        // service's type names.
        (
            "another service's ARN carrying no type name at all",
            "arn:aws:s3:::some-bucket".to_string(),
            "Resource",
        ),
        (
            "another service's ARN whose whole resource reads like a bundle",
            "arn:aws:s3:::appbundle".to_string(),
            "Resource",
        ),
        (
            "another service's object key carrying the ingestion type name",
            "arn:aws:s3:::my-bucket/ingestion/2024-01.json".to_string(),
            "Resource",
        ),
        (
            "another service's object key carrying the destination type name",
            "arn:aws:s3:::my-bucket/ingestiondestination/2024-01.json".to_string(),
            "Resource",
        ),
        (
            "another service's ARN carrying the authorization type name",
            "arn:aws:iam::000000000000:appauthorization/reader".to_string(),
            "Resource",
        ),
        (
            "a namespace differing from this service's only in case",
            format!("arn:aws:AppFabric:us-east-1:000000000000:appbundle/{bundle}"),
            "Resource",
        ),
        (
            "an ARN of a namespace this service's name is a prefix of",
            format!("arn:aws:appfabricx:us-east-1:000000000000:appbundle/{bundle}"),
            "Resource",
        ),
        // Values that are not well-formed ARNs: too few fields, no namespace,
        // no resource.
        (
            "a value with too few fields to be an ARN",
            "arn:aws:appfabric:appbundle".to_string(),
            "Resource",
        ),
        (
            "an ARN with an empty namespace field",
            format!("arn:aws::us-east-1:000000000000:appbundle/{bundle}"),
            "Resource",
        ),
        (
            "an ARN with an empty resource field",
            "arn:aws:appfabric:us-east-1:000000000000:".to_string(),
            "Resource",
        ),
        (
            "an ARN with an empty partition field",
            format!("arn::appfabric:us-east-1:000000000000:appbundle/{bundle}"),
            "Resource",
        ),
    ];

    let emulator = Emulator::new();
    for (case, arn, expected) in cases {
        for (operation, answer) in [
            (
                "TagResource",
                tag_resource(&emulator, &arn, tag_list(&[("env", "prod")])).await,
            ),
            ("ListTagsForResource", list_tags(&emulator, &arn).await),
            (
                "UntagResource",
                untag_resource(&emulator, &arn, &["env"]).await,
            ),
        ] {
            let case = format!("{operation} on {case}");
            let reported = refused_resource_type(&answer, &case, &arn);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected resourceType {expected:?} for {arn:?}, got {reported:?}"
            );
        }
    }
}
