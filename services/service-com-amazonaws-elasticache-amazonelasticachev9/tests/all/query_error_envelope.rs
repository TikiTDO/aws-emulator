// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error envelope, over the wire: every failed call this service answers
//! carries a `<RequestId>` inside its `<ErrorResponse>`, beside `<Error>`.
//!
//! The rule under test is the PROTOCOL's and is not per operation: an awsQuery error
//! response is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>`,
//! so whichever layer refused the call — a handler's modelled error, a handler refusal,
//! the scaffold's 501, the boundary validator, the routing table — the answer carries a
//! request id, which is what an SDK's error object exposes and what correlates a failed
//! call against server-side records. The table below is therefore keyed on the CLASS of
//! failure rather than on an operation, spans the parameter-group, snapshot and
//! service-update surfaces, and names the status and wire code each row expects as well,
//! so a row cannot pass by failing for a different reason.
//!
//! Every assertion goes over the wire through the same router the production emulator
//! mounts, reusing the harness in [`crate::cache_cluster_lifecycle`]; only the HTTP
//! status and the XML body are inspected.

use crate::harness::{params, Emulator, Reply};

/// One row of the error-envelope table: a request that must fail, and the status and
/// wire code it must fail with.
struct ErrorCase {
    /// What class of failure this row drives.
    case: &'static str,
    action: &'static str,
    params: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
}

/// The classes of failure this service can answer, one row each.
///
/// The codes are the ones each layer names: a modelled error renders its `awsQueryError`
/// code (`smithy_query(query="operation", operation="…")` — `CacheParameterGroupNotFound`,
/// `InvalidParameterValue`, `InvalidCacheParameterGroupState`, `CacheClusterNotFound`,
/// `SnapshotNotFoundFault`, `ServiceUpdateNotFoundFault`), a scaffolded operation
/// `NotImplemented`, a body the codec could not read `MalformedInput`, an absent required
/// parameter the `InvalidParameterValue` this service states one with, and an `Action`
/// naming no modelled operation `InvalidAction`.
const ERROR_CASES: &[ErrorCase] = &[
    ErrorCase {
        case: "a modelled 404: a describe naming no stored parameter group",
        action: "DescribeCacheParameterGroups",
        params: &[("CacheParameterGroupName", "never-created")],
        status: 404,
        code: "CacheParameterGroupNotFound",
    },
    ErrorCase {
        case: "a modelled 404: a delete naming no stored parameter group",
        action: "DeleteCacheParameterGroup",
        params: &[("CacheParameterGroupName", "never-created")],
        status: 404,
        code: "CacheParameterGroupNotFound",
    },
    ErrorCase {
        case: "a modelled 400: a default parameter group cannot be deleted",
        action: "DeleteCacheParameterGroup",
        params: &[("CacheParameterGroupName", "default.redis7")],
        status: 400,
        code: "InvalidParameterValue",
    },
    ErrorCase {
        case: "a modelled 400: an engine-defaults read naming no published family",
        action: "DescribeEngineDefaultParameters",
        params: &[("CacheParameterGroupFamily", "not-a-real-family")],
        status: 400,
        code: "InvalidParameterValue",
    },
    ErrorCase {
        case: "a handler refusal: a page size outside the documented window",
        action: "DescribeCacheParameterGroups",
        params: &[("MaxRecords", "5")],
        status: 400,
        code: "InvalidParameterValue",
    },
    ErrorCase {
        case: "a handler refusal: a Marker this service never issued",
        action: "DescribeCacheParameterGroups",
        params: &[("Marker", "not-a-marker-this-service-issued")],
        status: 400,
        code: "InvalidParameterValue",
    },
    ErrorCase {
        case: "a modelled 404: a snapshot of a cluster the account does not hold",
        action: "CreateSnapshot",
        params: &[
            ("SnapshotName", "snap-of-nothing"),
            ("CacheClusterId", "never-created"),
        ],
        status: 404,
        code: "CacheClusterNotFound",
    },
    ErrorCase {
        case: "a modelled 404: a copy of a snapshot the account does not hold",
        action: "CopySnapshot",
        params: &[
            ("SourceSnapshotName", "never-created"),
            ("TargetSnapshotName", "copy-of-nothing"),
        ],
        status: 404,
        code: "SnapshotNotFoundFault",
    },
    ErrorCase {
        case: "a modelled 404: a service-update read naming no published update",
        action: "DescribeServiceUpdates",
        params: &[("ServiceUpdateName", "never-released")],
        status: 404,
        code: "ServiceUpdateNotFoundFault",
    },
    ErrorCase {
        case: "a modelled 404: a batch apply naming no published update",
        action: "BatchApplyUpdateAction",
        params: &[
            ("ServiceUpdateName", "never-released"),
            ("CacheClusterIds.member.1", "never-created"),
        ],
        status: 404,
        code: "ServiceUpdateNotFoundFault",
    },
    ErrorCase {
        case: "a modelled 404: a batch stop naming no published update",
        action: "BatchStopUpdateAction",
        params: &[
            ("ServiceUpdateName", "never-released"),
            ("CacheClusterIds.member.1", "never-created"),
        ],
        status: 404,
        code: "ServiceUpdateNotFoundFault",
    },
    ErrorCase {
        case: "a modelled 400: a batch apply naming no resource at all",
        action: "BatchApplyUpdateAction",
        params: &[("ServiceUpdateName", "never-released")],
        status: 400,
        code: "InvalidParameterValue",
    },
    ErrorCase {
        case: "a scaffolded operation, which answers the 501 channel",
        action: "StopMigration",
        params: &[("ServerlessCacheName", "no-such-serverless-cache")],
        status: 501,
        code: "NotImplemented",
    },
    ErrorCase {
        case: "an absent required member, which the codec refuses",
        action: "DescribeEngineDefaultParameters",
        params: &[],
        status: 400,
        code: "InvalidParameterValue",
    },
    ErrorCase {
        case: "an Action naming no modelled operation",
        action: "NotAnActionAtAll",
        params: &[],
        status: 400,
        code: "InvalidAction",
    },
];

/// The request id of an error response, checked to be where AWS puts it: exactly one
/// `<RequestId>`, a sibling AFTER `<Error>` and inside `<ErrorResponse>`.
fn error_envelope_request_id(case: &str, reply: &Reply) -> String {
    let body = &reply.body;
    assert!(
        body.contains("<ErrorResponse"),
        "case {case:?}: expected an <ErrorResponse> envelope, got {body:?}"
    );
    let ids = reply.all("RequestId");
    assert_eq!(
        ids.len(),
        1,
        "case {case:?}: expected exactly one <RequestId> in the envelope, got {ids:?} from \
         {body:?}"
    );
    let error_close = body
        .rfind("</Error>")
        .unwrap_or_else(|| panic!("case {case:?}: expected an <Error> element, got {body:?}"));
    let request_id_open = body
        .find("<RequestId>")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <RequestId> element, got {body:?}"));
    let envelope_close = body.rfind("</ErrorResponse>").unwrap_or_else(|| {
        panic!("case {case:?}: expected a closed <ErrorResponse>, got {body:?}")
    });
    assert!(
        error_close < request_id_open && request_id_open < envelope_close,
        "case {case:?}: expected <RequestId> between </Error> and </ErrorResponse>, got {body:?}"
    );
    let request_id = ids[0].to_string();
    assert!(
        !request_id.is_empty(),
        "case {case:?}: expected a non-empty request id, got {body:?}"
    );
    request_id
}

/// One row of a table that drives one action: the case, the action, and the form pairs
/// the request carries.
type ActionCase = (&'static str, &'static str, Vec<(String, String)>);

/// One row of a table whose action must FAIL: [`ActionCase`] plus the status and the wire
/// code it must fail with.
type RefusedCase = (&'static str, &'static str, Vec<(String, String)>, u16, &'static str);

/// A request id has the 8-4-4-4-12 lowercase-hex form AWS answers with, whether it came
/// from an error envelope or from a successful response's `ResponseMetadata`.
fn assert_request_id_shape(case: &str, request_id: &str) {
    let groups: Vec<&str> = request_id.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    assert_eq!(
        groups.len(),
        widths.len(),
        "case {case:?}: expected five hyphen-separated groups, got {request_id:?}"
    );
    for (group, width) in groups.iter().zip(widths) {
        assert!(
            group.len() == width
                && group
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "case {case:?}: expected a {width}-character lowercase-hex group, got {group:?} in \
             {request_id:?}"
        );
    }
}

/// Every class of failure answers an envelope carrying a request id — and still answers
/// the status, the `<Type>` and the wire code it did before, since this rule owns one
/// element of the envelope and nothing else.
#[tokio::test]
async fn every_error_response_carries_a_request_id() {
    let emulator = Emulator::new();

    for case in ERROR_CASES {
        let reply = emulator.call(case.action, &params(case.params)).await;
        assert_eq!(
            reply.status, case.status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            case.case, case.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            case.code,
            "case {:?}: expected <Code>{}</Code>, got {:?}",
            case.case,
            case.code,
            reply.body
        );
        let expected_type = if case.status >= 500 {
            "Receiver"
        } else {
            "Sender"
        };
        assert_eq!(
            reply.text("Error/Type"),
            Some(expected_type),
            "case {:?}: expected <Type>{expected_type}</Type>, got {:?}",
            case.case,
            reply.body
        );
        let request_id = error_envelope_request_id(case.case, &reply);
        assert_request_id_shape(case.case, &request_id);
    }
}

/// A request id names ONE request: two failures, of the same class or of different ones,
/// never report the same id.
#[tokio::test]
async fn each_error_response_carries_its_own_request_id() {
    let emulator = Emulator::new();

    let mut seen: Vec<String> = Vec::new();
    for case in ERROR_CASES.iter().chain(ERROR_CASES.iter()) {
        let reply = emulator.call(case.action, &params(case.params)).await;
        let request_id = error_envelope_request_id(case.case, &reply);
        assert!(
            !seen.contains(&request_id),
            "case {:?}: expected a fresh request id, got the already-reported {request_id:?}",
            case.case
        );
        seen.push(request_id);
    }
}

/// A refusal that depends on STORED state answers the same envelope: a duplicate create,
/// and a delete of the group a cluster is associated with.
#[tokio::test]
async fn a_state_dependent_refusal_carries_a_request_id() {
    let emulator = Emulator::new();

    let created = emulator
        .call(
            "CreateCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", "envelope-group"),
                ("CacheParameterGroupFamily", "redis7"),
                ("Description", "the group the refusals below are about"),
            ]),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the first create must succeed, got {} {:?}",
        created.status, created.body
    );
    let cluster = emulator
        .call(
            "CreateCacheCluster",
            &params(&[
                ("CacheClusterId", "envelope-cluster"),
                ("Engine", "redis"),
                ("CacheNodeType", "cache.t3.micro"),
                ("NumCacheNodes", "1"),
                ("CacheParameterGroupName", "envelope-group"),
            ]),
        )
        .await;
    assert_eq!(
        cluster.status, 200,
        "the cluster must be created, got {} {:?}",
        cluster.status, cluster.body
    );

    // (case, action, params, status, code)
    let rows: [RefusedCase; 2] = [
        (
            "a duplicate create",
            "CreateCacheParameterGroup",
            params(&[
                ("CacheParameterGroupName", "envelope-group"),
                ("CacheParameterGroupFamily", "redis7"),
                ("Description", "a second group of the same name"),
            ]),
            400,
            "CacheParameterGroupAlreadyExists",
        ),
        (
            "a delete of the group a cluster is associated with",
            "DeleteCacheParameterGroup",
            params(&[("CacheParameterGroupName", "envelope-group")]),
            400,
            "InvalidCacheParameterGroupState",
        ),
    ];

    for (case, action, pairs, status, code) in rows {
        let reply = emulator.call(action, &pairs).await;
        assert_eq!(
            reply.status, status,
            "case {case:?}: expected HTTP {status}, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            code,
            "case {case:?}: expected <Code>{code}</Code>, got {:?}",
            reply.body
        );
        let request_id = error_envelope_request_id(case, &reply);
        assert_request_id_shape(case, &request_id);
    }
}

/// The success path is untouched: a successful response keeps its one
/// `ResponseMetadata/RequestId` and grows no error envelope — including the delete, whose
/// output shape has no members at all.
#[tokio::test]
async fn a_successful_response_keeps_its_response_metadata_request_id() {
    let emulator = Emulator::new();

    // (case, action, params)
    let rows: [ActionCase; 5] = [
        (
            "a describe of every group",
            "DescribeCacheParameterGroups",
            Vec::new(),
        ),
        (
            "a create",
            "CreateCacheParameterGroup",
            params(&[
                ("CacheParameterGroupName", "success-group"),
                ("CacheParameterGroupFamily", "valkey8"),
                ("Description", "a group read back below"),
            ]),
        ),
        (
            "a describe that reads it back",
            "DescribeCacheParameterGroups",
            params(&[("CacheParameterGroupName", "success-group")]),
        ),
        (
            "a delete, whose output shape has no members",
            "DeleteCacheParameterGroup",
            params(&[("CacheParameterGroupName", "success-group")]),
        ),
        (
            "a read of the published service updates",
            "DescribeServiceUpdates",
            Vec::new(),
        ),
    ];

    for (case, action, pairs) in rows {
        let reply = emulator.call(action, &pairs).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            reply.status, reply.body
        );
        assert!(
            !reply.body.contains("<ErrorResponse"),
            "case {case:?}: a success must carry no error envelope, got {:?}",
            reply.body
        );
        let ids = reply.all("RequestId");
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: expected one ResponseMetadata/RequestId, got {ids:?} from {:?}",
            reply.body
        );
        assert_request_id_shape(case, ids[0]);
    }
}

/// The escaping invariant the splice rests on, exercised with REQUEST input rather than a
/// pre-escaped literal: a name carrying XML metacharacters is quoted back in the message
/// as entities, so it cannot close `<Message>`, `<Error>` or `<ErrorResponse>` early, and
/// the envelope still carries exactly one `<RequestId>` in the sibling position. The
/// delete half of each row answers the identifier refusal instead — such a name is not
/// one of this service's identifiers — which splices nothing and must keep the same
/// envelope shape.
#[tokio::test]
async fn a_name_carrying_xml_metacharacters_cannot_reshape_the_envelope() {
    let emulator = Emulator::new();

    // (case, the CacheParameterGroupName the request carries)
    let names: [(&str, &str); 6] = [
        ("a name closing the error element", "x</Error></ErrorResponse>"),
        ("a name closing the message element", "x</Message>"),
        ("a name of bare metacharacters", "a&b<c>d"),
        ("a name quoting attribute delimiters", "a\"b'c"),
        ("a name that is an entity already", "&amp;lt;"),
        ("a name opening a tag", "<RequestId>spoofed</RequestId>"),
    ];

    for (case, name) in names {
        // Only the READER resolves such a name against the store and quotes it back: a
        // DELETE holds the name against the service's identifier grammar first, and none
        // of these names is a legal identifier, so its refusal quotes no request text at
        // all (asserted below).
        let action = "DescribeCacheParameterGroups";
        {
            let reply = emulator
                .call(action, &params(&[("CacheParameterGroupName", name)]))
                .await;
            assert_eq!(
                reply.status, 404,
                "case {case:?} on {action}: expected HTTP 404, got {} with {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code(),
                "CacheParameterGroupNotFound",
                "case {case:?} on {action}: expected the modelled not-found code, got {:?}",
                reply.body
            );
            // The reader unescapes text nodes, so the message carries the submitted name
            // itself — which is only possible if the wire form was escaped. A handler may
            // quote the name as submitted or as stored (lowercased); either names it.
            let message = reply.text("Error/Message").unwrap_or_default();
            assert!(
                message.contains(name) || message.contains(&name.to_ascii_lowercase()),
                "case {case:?} on {action}: expected the message to name the submitted group, \
                 got {message:?} from {:?}",
                reply.body
            );
            // One `<RequestId>`, still the envelope's last child: a metacharacter in the
            // message did not move it or let the request forge a second one.
            let request_id = error_envelope_request_id(case, &reply);
            assert_request_id_shape(case, &request_id);
            assert!(
                reply.body.trim_end().ends_with(
                    &format!("<RequestId>{request_id}</RequestId></ErrorResponse>")
                ),
                "case {case:?} on {action}: expected the element as the envelope's last child, \
                 got {:?}",
                reply.body
            );
        }

        // The delete: a name carrying a metacharacter is not a legal identifier, so it is
        // the modelled parameter-value refusal — and the envelope is still the one shape,
        // with its single `<RequestId>` as the last child.
        let deleted = emulator
            .call(
                "DeleteCacheParameterGroup",
                &params(&[("CacheParameterGroupName", name)]),
            )
            .await;
        assert_eq!(
            (deleted.status, deleted.error_code()),
            (400, "InvalidParameterValue"),
            "case {case:?} on DeleteCacheParameterGroup: expected the identifier refusal, got \
             {} with {:?}",
            deleted.status,
            deleted.body
        );
        let request_id = error_envelope_request_id(case, &deleted);
        assert_request_id_shape(case, &request_id);
        assert!(
            deleted
                .body
                .trim_end()
                .ends_with(&format!("<RequestId>{request_id}</RequestId></ErrorResponse>")),
            "case {case:?} on DeleteCacheParameterGroup: expected the element as the envelope's \
             last child, got {:?}",
            deleted.body
        );
    }
}

/// The error envelope is rooted at the model's `<ErrorResponse xmlns="...">`, and the
/// added `<RequestId>` sits inside that root rather than after it.
#[tokio::test]
async fn the_error_envelope_carries_the_service_namespace() {
    let emulator = Emulator::new();
    let namespace = modelled_xml_namespace();

    let reply = emulator
        .call(
            "DescribeCacheParameterGroups",
            &params(&[("CacheParameterGroupName", "never-created")]),
        )
        .await;
    assert_eq!(
        reply.status, 404,
        "the read must be refused, got {} {:?}",
        reply.status, reply.body
    );
    assert!(
        reply
            .body
            .contains(&format!("<ErrorResponse xmlns=\"{namespace}\">")),
        "case \"a modelled error\": expected the model's namespace on the envelope root, got {:?}",
        reply.body
    );
    let request_id = error_envelope_request_id("a modelled error", &reply);
    assert_request_id_shape("a modelled error", &request_id);
    assert!(
        reply.body.trim_end().ends_with("</ErrorResponse>"),
        "the envelope must be the whole document, got {:?}",
        reply.body
    );
}

/// The service's `@xmlNamespace` URI, read out of the shipped model rather than restated
/// here (`smithy_query(query="service_info")` -> `xmlNamespace.uri`).
fn modelled_xml_namespace() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json"
    );
    let model: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("the shipped model"))
            .expect("the model parses as JSON");
    model["shapes"]["com.amazonaws.elasticache#AmazonElastiCacheV9"]["traits"]
        ["smithy.api#xmlNamespace"]["uri"]
        .as_str()
        .expect("the service shape declares an @xmlNamespace uri")
        .to_string()
}
