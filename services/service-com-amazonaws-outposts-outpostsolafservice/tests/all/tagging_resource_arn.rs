// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the ONE reading of `ResourceArn` the tagging API has,
//! driven over the wire through `MultiServiceRouter::dispatch` exactly as a signed
//! `aws outposts` call arrives (reusing the harness of
//! [`crate::site_and_outpost_lifecycle`]).
//!
//! The rule under test, stated once and then exercised over a SET of inputs:
//! every operation that binds `ResourceArn` reads that member by the same rule and
//! words its refusal from the same report, so ONE ARN value is classified the same
//! way — the same HTTP status, the same wire error code and the same message —
//! whichever of them was called, and a refusal of the ARN's syntax renders none of
//! the value it refused. A value the operations SERVE is answered by each with its
//! own success status: the read's 200 carries the tag map, and the mutation's
//! no-content status carries nothing.
//!
//! The rows span the classes a `ResourceArn` falls in: one naming a stored resource
//! of each taggable kind, one that is readable but names nothing (including a
//! resource kind this service does not hold, and an ARN in another partition,
//! region and account), and one that is not an Outposts resource ARN at all — the
//! last in each of the ways the modelled `Arn`
//! `^(arn:aws([a-z-]+)?:outposts:[a-z\d-]+:\d{12}:([a-z\d-]+)/)[a-z]{2,8}-[a-f0-9]{17}$`
//! can be failed in.

use serde_json::{json, Value};

use crate::error_classification::reported_tags;
use crate::site_and_outpost_lifecycle::{segment, text, Outposts, TAG_MUTATION_SUCCESS_STATUS};

/// The account id every generated emulator presents, which its ARNs carry.
const ACCOUNT_ID: &str = "000000000000";

/// The modelled maximum length of `ResourceArn` (`Arn`, `@length(max: 1011)`), from
/// `smithy_query(query="operation",operation="ListTagsForResource")`: a value past it
/// is one more way to fail to be an Outposts resource ARN.
const RESOURCE_ARN_MAX: usize = 1011;

/// One `ResourceArn` the whole tagging API is called with, and what every operation
/// that binds that member must answer for it.
struct Arn {
    case: String,
    arn: String,
    status: u16,
    /// The status the tag MUTATION answers for the same value. A refusal is ONE
    /// status for every operation that binds the member; a served tag reports
    /// nothing back about the resource, so it is answered with the no-content
    /// status instead of the read's 200.
    mutation_status: u16,
    /// The wire error code, empty for a row the operations serve.
    code: &'static str,
}

/// The error message a reply carries, under the key its envelope spells it with
/// (`Message` for a modelled error shape, `message` for the framework's synthetic
/// one), read as the empty string when it carries none.
fn message(body: &Value) -> String {
    ["Message", "message"]
        .iter()
        .find_map(|key| body.get(key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// Every class of `ResourceArn`, with a stored site and a stored Outpost created for
/// the rows that name one.
async fn resource_arns(outposts: &Outposts) -> Vec<Arn> {
    let site = outposts
        .create_site(&json!({ "Name": "resource arn site" }))
        .await;
    let outpost = outposts
        .create_outpost(&json!({
            "Name": "resource arn outpost",
            "SiteId": text(&site, "SiteId"),
        }))
        .await;

    let readable: Vec<(&str, String, u16, &'static str)> = vec![
        ("a stored site", text(&site, "SiteArn"), 200, ""),
        ("a stored Outpost", text(&outpost, "OutpostArn"), 200, ""),
        (
            "an absent site",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:site/os-00000000000000000"),
            404,
            "NotFoundException",
        ),
        (
            "an absent Outpost",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/op-00000000000000000"),
            404,
            "NotFoundException",
        ),
        (
            "a resource kind this service does not hold",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:order/oo-00000000000000000"),
            404,
            "NotFoundException",
        ),
        (
            "another partition, region and account",
            "arn:aws-us-gov:outposts:us-gov-west-1:123456789012:site/os-11111111111111111"
                .to_string(),
            404,
            "NotFoundException",
        ),
        ("not an ARN at all", "not-a-valid-arn".to_string(), 400, "BadRequestException"),
        (
            "a bare resource id",
            "op-0123456789abcdef0".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "another service's ARN",
            format!("arn:aws:s3:us-east-1:{ACCOUNT_ID}:bucket/op-0123456789abcdef0"),
            400,
            "BadRequestException",
        ),
        (
            "no resource type",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:op-0123456789abcdef0"),
            400,
            "BadRequestException",
        ),
        (
            "a resource id that is not 17 hex characters",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/op-nothexadecimal0"),
            400,
            "BadRequestException",
        ),
        (
            "past the member's modelled maximum length",
            format!(
                "arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/op-{}",
                "a".repeat(RESOURCE_ARN_MAX)
            ),
            400,
            "BadRequestException",
        ),
    ];

    readable
        .into_iter()
        .map(|(case, arn, status, code)| Arn {
            case: case.to_string(),
            arn,
            status,
            mutation_status: if code.is_empty() {
                TAG_MUTATION_SUCCESS_STATUS
            } else {
                status
            },
            code,
        })
        .collect()
}

/// The rule, over every class of `ResourceArn` and both operations that bind it.
///
/// `ListTagsForResource` (GET) and `TagResource` (POST) are called with the SAME
/// value, and have to agree on how it is classified and worded; a row the two serve
/// is then round-tripped — the tag the POST wrote is what the GET answers — which is
/// what shows both resolved the same stored record out of the one ARN.
#[tokio::test]
async fn every_tagging_operation_reads_one_resource_arn_the_same_way() {
    let outposts = Outposts::new();

    for row in resource_arns(&outposts).await {
        let case = &row.case;
        let path = format!("/tags/{}", segment(&row.arn));
        let request = json!({ "Tags": { "shared": "path" } });

        let listed = outposts.call("GET", &path, None).await;
        let tagged = outposts.call("POST", &path, Some(&request)).await;

        assert_eq!(
            (
                listed.status,
                listed.error_code.as_str(),
                listed.errortype_header.as_str()
            ),
            (row.status, row.code, row.code),
            "case {case:?}: ListTagsForResource must answer {} {:?} for this ResourceArn, \
             got {} {:?}",
            row.status,
            row.code,
            listed.status,
            listed.body
        );
        assert_eq!(
            (
                tagged.status,
                tagged.error_code.as_str(),
                tagged.errortype_header.as_str()
            ),
            (row.mutation_status, row.code, row.code),
            "case {case:?}: TagResource must answer {} {:?} for the same ResourceArn, \
             got {} {:?}",
            row.mutation_status,
            row.code,
            tagged.status,
            tagged.body
        );

        if row.status == 200 {
            // Both operations resolved the same stored record from the one ARN: the
            // tags the POST wrote are the tags the GET answers.
            let read_back = outposts.ok("GET", &path, None).await;
            let reported = reported_tags(&read_back);
            assert_eq!(
                reported.get("shared").map(String::as_str),
                Some("path"),
                "case {case:?}: the tag TagResource wrote through this ARN must be what \
                 ListTagsForResource answers through it, got {reported:?}"
            );
            continue;
        }

        // A refusal is worded once, so the two operations state it identically.
        let (list_message, tag_message) = (message(&listed.body), message(&tagged.body));
        assert_eq!(
            list_message, tag_message,
            "case {case:?}: both operations must word this refusal the same way, got \
             {list_message:?} and {tag_message:?}"
        );
        assert!(
            !list_message.is_empty(),
            "case {case:?}: the refusal must carry a message, got {:?}",
            listed.body
        );

        // A refusal of the ARN's SYNTAX names the member and the requirement and
        // renders none of the value it refused. (A 404 is the other rule: the tagging
        // API's fixed not-found sentence, which renders none of the value either and
        // is asserted in `crate::tagging_not_found_wording`.)
        if row.status == 400 {
            assert!(
                !list_message.contains(&row.arn),
                "case {case:?}: a refusal of the ARN's syntax must render none of the \
                 value, got {list_message:?}"
            );
        }
    }
}
