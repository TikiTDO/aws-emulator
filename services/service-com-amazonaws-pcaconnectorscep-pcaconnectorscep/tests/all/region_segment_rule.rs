// SPDX-License-Identifier: CC0-1.0

//! The Region a record is filed under, and the Region an ARN of this service may name, are
//! ONE rule.
//!
//! The rule these cases assert: a Region segment of one of this service's ARNs is two or more
//! lower-case words joined by `-` followed by a number with no leading zero
//! (`ConnectorArn`/`ChallengeArn`'s own `@pattern`, smithy_query shape=ConnectorArn), within
//! the length this emulator files a record under — and that ONE rule decides both halves of a
//! round trip: which Region a request's credential scope files its record in, and which ARNs
//! the tagging operations accept. So whatever Region a connector ends up in, its own ARN is
//! one those operations resolve rather than refuse; and a Region name the rule rejects is
//! both served in the default Region and refused inside an ARN.
//!
//! Each row therefore drives one operation on each side of the rule: `CreateConnector`, which
//! reads the Region out of the request's SigV4 credential scope, and `TagResource` /
//! `ListTagsForResource`, which parse a whole `ResourceArn` against the pattern that embeds
//! the same Region expression. The rows differ along what the rule generalises over: how many
//! words the name has, how many digits its number has, case, word emptiness, the alphabet a
//! word uses, a leading zero, a missing number, and the length bound — at it and one past it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};
use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

/// The canonical emulator account id every ARN of this service carries.
const ACCOUNT: &str = "000000000000";

/// The Region a request whose credential scope names none this service can file a record
/// under is served in.
const DEFAULT_REGION: &str = scep::state::DEFAULT_REGION;

/// How long a Region name this service will file a record under may be — the bound that is
/// deliberately NOT part of the Region expression, so the rows can pin it separately.
const MAX_REGION_LENGTH: usize = scep::state::MAX_REGION_LENGTH;

/// The requirement a refusal quotes back for a `ResourceArn`, taken from the service rather
/// than respelled: the one expression the Region rule is embedded in.
const RESOURCE_ARN_PATTERN: &str = scep::state::RESOURCE_ARN_PATTERN;

/// A connector id in the UUID form the ARN patterns require. Nothing is ever created under
/// it, so an ARN carrying it can only be answered by how the value itself reads.
const CONNECTOR_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// One request of this suite: every case addresses the default Region's endpoint, so the
/// host never decides anything the credential scope is being tested for, and carries
/// `scope_region` in its SigV4 credential scope — which is how a handler learns the Region
/// the caller is addressing.
async fn call_in_scope(
    router: &MultiServiceRouter,
    scope_region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    harness::call_with_scope(router, DEFAULT_REGION, scope_region, method, path, body).await
}

async fn create_connector(router: &MultiServiceRouter, scope_region: &str, ca: &str) -> Answer {
    call_in_scope(
        router,
        scope_region,
        "POST",
        "/connectors",
        Some(json!({ "CertificateAuthorityArn": ca })),
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, scope_region: &str, arn: &str) -> Answer {
    call_in_scope(
        router,
        scope_region,
        "GET",
        &format!("/tags/{}", label(arn)),
        None,
    )
    .await
}

async fn tag_resource(
    router: &MultiServiceRouter,
    scope_region: &str,
    arn: &str,
    tags: Value,
) -> Answer {
    call_in_scope(
        router,
        scope_region,
        "POST",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// A connector ARN of this service, in the published form.
fn connector_arn(region: &str, connector_id: &str) -> String {
    format!("arn:aws:pca-connector-scep:{region}:{ACCOUNT}:connector/{connector_id}")
}

/// A private-CA ARN matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`), in the
/// default Region so no row's own Region name has to be a legal one here too.
fn ca_arn(suffix: &str) -> String {
    format!("arn:aws:acm-pca:{DEFAULT_REGION}:{ACCOUNT}:certificate-authority/{suffix}")
}

/// A Region-shaped name of exactly `total` characters: `<letters>-<letters>-1`, the two letter
/// runs sharing whatever is left after the separators and the trailing `1`.
fn region_of_length(total: usize) -> String {
    let letters = total - 3;
    let first = letters / 2;
    format!("{}-{}-1", "u".repeat(first), "e".repeat(letters - first))
}

/// One Region name, and what the one rule says about it on each side.
#[derive(Debug)]
struct Row {
    name: &'static str,
    /// The Region name a request's credential scope carries.
    region: String,
    /// Whether a record signed for it is filed under it; a name the rule rejects is served in
    /// the default Region instead.
    filed_under_itself: bool,
    /// Whether an ARN carrying it as its Region segment satisfies the published ARN pattern.
    /// This is the SAME expression, so it agrees with `filed_under_itself` except where the
    /// separate length bound is the only thing that rejected the name.
    arn_form_accepted: bool,
}

fn rows() -> Vec<Row> {
    let accepted = |name: &'static str, region: &str| Row {
        name,
        region: region.to_string(),
        filed_under_itself: true,
        arn_form_accepted: true,
    };
    let rejected = |name: &'static str, region: &str| Row {
        name,
        region: region.to_string(),
        filed_under_itself: false,
        arn_form_accepted: false,
    };
    vec![
        accepted("two words and a single digit", "eu-west-3"),
        accepted("three words", "us-gov-east-1"),
        accepted("a two-digit number", "ap-southeast-12"),
        Row {
            name: "at the length bound this emulator files a record under",
            region: region_of_length(MAX_REGION_LENGTH),
            filed_under_itself: true,
            arn_form_accepted: true,
        },
        rejected("upper case", "US-EAST-1"),
        rejected("a single word", "useast1"),
        rejected("no trailing number", "us-east"),
        rejected("a number that is zero", "us-east-0"),
        rejected("a leading zero in the number", "us-east-01"),
        rejected("an empty word", "us--east-1"),
        rejected("a digit inside a word", "us-east1-x"),
        rejected("a character outside the alphabet", "us-e@st-1"),
        rejected("a name that is only a number", "1"),
        Row {
            // The length is the SEPARATE bound: this name satisfies the Region expression, so
            // an ARN carrying it is well formed, yet no record is filed under a name past the
            // bound.
            name: "one character past the length bound",
            region: region_of_length(MAX_REGION_LENGTH + 1),
            filed_under_itself: false,
            arn_form_accepted: true,
        },
    ]
}

/// The whole `Message` a refusal of `resource_arn` carries: AWS's validation envelope over the
/// one clause the pattern constraint states.
fn expected_pattern_refusal(resource_arn: &str) -> String {
    format!(
        "1 validation error detected: Value '{resource_arn}' at 'resourceArn' failed to \
         satisfy constraint: Member must satisfy regular expression pattern: \
         {RESOURCE_ARN_PATTERN}"
    )
}

/// The Region a record signed for this row is filed under is always one an ARN of this service
/// can name: the ARN the create answers with is accepted, tagged and read back by the
/// operations that parse a `ResourceArn` against the published pattern.
#[tokio::test]
async fn the_region_a_record_is_filed_under_is_one_its_own_arn_can_name() {
    let router = new_router().0;
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.name;
        let expected_region = if row.filed_under_itself {
            row.region.clone()
        } else {
            DEFAULT_REGION.to_string()
        };

        let created = create_connector(
            &router,
            &row.region,
            // One private CA per row, so no two rows name the same one.
            &ca_arn(&format!("11111111-2222-3333-4444-5555555555{index:02}")),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected 202 from CreateConnector signed for {:?}, got {} / {:?}",
            row.region, created.status, created.body
        );
        let arn = created.text("ConnectorArn");
        assert_eq!(
            arn.split(':').nth(3),
            Some(expected_region.as_str()),
            "case {case:?}: a record signed for {:?} must be filed under {expected_region:?}, \
             got {arn:?}",
            row.region
        );

        // The other side of the same rule: the tagging operations parse that whole ARN against
        // the pattern the Region expression is embedded in, so they must resolve it.
        let tagged = tag_resource(&router, &row.region, &arn, json!({ "filed": "here" })).await;
        assert_eq!(
            (tagged.status, tagged.error_type.as_str()),
            (204, ""),
            "case {case:?}: TagResource must accept the ARN the create answered with ({arn:?}), \
             got {} / {:?}",
            tagged.status,
            tagged.body
        );
        let listed = list_tags(&router, &row.region, &arn).await;
        assert_eq!(
            listed.body["Tags"],
            json!({ "filed": "here" }),
            "case {case:?}: ListTagsForResource must read back the tag written through {arn:?}, \
             got {} / {:?}",
            listed.status,
            listed.body
        );
    }
}

/// An ARN whose Region segment breaks the rule is refused by the operations that parse a
/// `ResourceArn`, with the clause that quotes the published pattern; one whose Region segment
/// keeps it is well formed and simply names nothing, so it is the tagging layer's own
/// not-found refusal.
#[tokio::test]
async fn an_arn_is_accepted_exactly_where_its_region_segment_satisfies_the_rule() {
    let router = new_router().0;
    for row in rows() {
        let case = row.name;
        let arn = connector_arn(&row.region, CONNECTOR_ID);
        for (operation, answer) in [
            (
                "ListTagsForResource",
                list_tags(&router, DEFAULT_REGION, &arn).await,
            ),
            (
                "TagResource",
                tag_resource(&router, DEFAULT_REGION, &arn, json!({ "owner": "platform" })).await,
            ),
        ] {
            if row.arn_form_accepted {
                harness::assert_tagging_not_found(&answer, &format!("{case} on {operation}"));
            } else {
                assert_eq!(
                    (answer.status, answer.error_type.as_str()),
                    (400, "ValidationException"),
                    "case {case:?} on {operation}: expected 400 ValidationException for \
                     {arn:?}, got {} / {:?}",
                    answer.status,
                    answer.body
                );
                assert_eq!(
                    answer.message(),
                    expected_pattern_refusal(&arn),
                    "case {case:?} on {operation}: expected the validation envelope quoting \
                     the published pattern, got {:?}",
                    answer.message()
                );
            }
        }
    }
}
