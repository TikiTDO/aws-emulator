// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal this service answers a parameter it cannot take with.
//!
//! Two rules, each over a SET of requests rather than one recorded one:
//!
//! 1. ABSENT — a request that omits a parameter the model requires is refused HTTP 400
//!    `InvalidParameterValue`, worded `The parameter <Name> must be provided.` over the
//!    public name the request would have carried the parameter under. The rows span the
//!    operations and the kinds of member a request may leave out (a string, a list, the
//!    identity of a resource, the member that describes it) and, for one operation, each
//!    required member in turn.
//!
//! 2. UNSERVEABLE — a `MaxRecords` outside what its own member documents, and a `Marker`
//!    this service did not mint, are values a paginated describe cannot serve a page for,
//!    and every describe of this service refuses one with the same
//!    `InvalidParameterValue`. The rows span the describes whose modelled error list omits
//!    that fault, each published bound of the window and one past it, and a marker no
//!    `Marker` this service issues could be.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts; only the status, the `awsQuery` error code and the message are read.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator};

/// The awsQuery code ElastiCache names a parameter it cannot take with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// One absent-parameter row: the case, the operation, the form pairs that carry every
/// required member BUT one, and the public name of the member left out.
struct AbsentCase {
    case: &'static str,
    action: &'static str,
    pairs: &'static [(&'static str, &'static str)],
    member: &'static str,
}

#[tokio::test]
async fn an_absent_required_parameter_is_refused_as_a_bad_parameter_value() {
    let emulator = Emulator::new();

    // The axis: the operation, and which required member the request leaves out. The three
    // `CreateCacheSubnetGroup` rows leave out each of its required members in turn, so the
    // sentence is shown to name the member rather than the operation.
    let rows: [AbsentCase; 7] = [
        AbsentCase {
            case: "a create without its subnet list",
            action: "CreateCacheSubnetGroup",
            pairs: &[
                ("CacheSubnetGroupName", "no-subnets"),
                ("CacheSubnetGroupDescription", "none"),
            ],
            member: "SubnetIds",
        },
        AbsentCase {
            case: "a create without its name",
            action: "CreateCacheSubnetGroup",
            pairs: &[
                ("CacheSubnetGroupDescription", "none"),
                ("SubnetIds.SubnetIdentifier.1", "subnet-00000000deadbeef"),
            ],
            member: "CacheSubnetGroupName",
        },
        AbsentCase {
            case: "a create without its description",
            action: "CreateCacheSubnetGroup",
            pairs: &[
                ("CacheSubnetGroupName", "no-description"),
                ("SubnetIds.SubnetIdentifier.1", "subnet-00000000deadbeef"),
            ],
            member: "CacheSubnetGroupDescription",
        },
        AbsentCase {
            case: "a delete without the group it would remove",
            action: "DeleteCacheSubnetGroup",
            pairs: &[],
            member: "CacheSubnetGroupName",
        },
        AbsentCase {
            case: "a modify without the group it would change",
            action: "ModifyCacheSubnetGroup",
            pairs: &[("CacheSubnetGroupDescription", "nothing to change")],
            member: "CacheSubnetGroupName",
        },
        AbsentCase {
            case: "a reboot without the nodes it would reboot",
            action: "RebootCacheCluster",
            pairs: &[("CacheClusterId", "never-created")],
            member: "CacheNodeIdsToReboot",
        },
        AbsentCase {
            case: "an engine-defaults read without the family it would report",
            action: "DescribeEngineDefaultParameters",
            pairs: &[],
            member: "CacheParameterGroupFamily",
        },
    ];

    for AbsentCase {
        case,
        action,
        pairs,
        member,
    } in rows
    {
        let refused = emulator.call(action, &params(pairs)).await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            INVALID_PARAMETER_VALUE,
            "case {case}: expected {INVALID_PARAMETER_VALUE}, got {}",
            refused.body
        );
        let expected = format!("The parameter {member} must be provided.");
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }
}

#[tokio::test]
async fn a_page_a_describe_cannot_serve_is_refused_as_a_bad_parameter_value() {
    let emulator = Emulator::new();

    // The describes whose modelled error list omits the bad-parameter-value fault, which is
    // where this rule had to be widened in; each is also one a client paginates.
    let describes = [
        "DescribeCacheSubnetGroups",
        "DescribeUsers",
        "DescribeUserGroups",
    ];
    // The axis: the page size, on each published bound of the window `MaxRecords` documents
    // ("minimum 20; maximum 100" — `state::MAX_RECORDS_MIN` / `MAX_RECORDS_MAX`, where this
    // crate states that window once), one past each bound, no page at all and a negative
    // count. `DescribeUsers` documents no window, so only the values that are no page size
    // at all are refused there.
    let windowed: [(&str, i32, bool); 6] = [
        ("one below the minimum", state::MAX_RECORDS_MIN - 1, true),
        ("at the minimum", state::MAX_RECORDS_MIN, false),
        ("at the maximum", state::MAX_RECORDS_MAX, false),
        ("one above the maximum", state::MAX_RECORDS_MAX + 1, true),
        ("no records at all", 0, true),
        ("a negative count", -5, true),
    ];
    for action in describes {
        let unbounded = action == "DescribeUsers";
        for (case, max_records, refused_in_window) in windowed {
            let case = format!("{action} / MaxRecords {max_records} ({case})");
            // A window the member does not document cannot refuse a value for being
            // outside it: only a value that is no page size at all is refused.
            let must_refuse = if unbounded {
                max_records <= 0
            } else {
                refused_in_window
            };
            let reply = emulator
                .call(action, &params(&[("MaxRecords", &max_records.to_string())]))
                .await;
            if must_refuse {
                assert_eq!(
                    reply.status, 400,
                    "case {case}: expected HTTP 400, got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.error_code(),
                    INVALID_PARAMETER_VALUE,
                    "case {case}: expected {INVALID_PARAMETER_VALUE}, got {}",
                    reply.body
                );
            } else {
                assert_eq!(
                    reply.status, 200,
                    "case {case}: a page size the member publishes must be served, got {} {}",
                    reply.status, reply.body
                );
            }
        }

        // The axis: a `Marker` no token this service mints could be — not hex, an odd
        // number of hex digits, and hex that decodes to nothing this service issued.
        for marker in ["not-a-marker", "abc", "zzzz"] {
            let case = format!("{action} / Marker {marker:?}");
            let refused = emulator
                .call(action, &params(&[("Marker", marker)]))
                .await;
            assert_eq!(
                refused.status, 400,
                "case {case}: expected HTTP 400, got {} {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_code(),
                INVALID_PARAMETER_VALUE,
                "case {case}: expected {INVALID_PARAMETER_VALUE}, got {}",
                refused.body
            );
            let expected = format!("Invalid value {marker} for Marker.");
            assert_eq!(
                refused.error_message(),
                expected,
                "case {case}: expected {expected:?}, got {:?}",
                refused.error_message()
            );
        }
    }
}
