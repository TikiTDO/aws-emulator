// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the b2bi transformer slice: `CreateTransformer` and
//! `GetTransformer`, driven over the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`)
//! through the same router the emulator boots, so every assertion is about what a real
//! SDK client observes.
//!
//! Each test states a rule and asserts it over a TABLE of inputs rather than one recorded
//! request: the rows differ along whatever the rule generalises over (configuration style,
//! which optional members are present, enum value, signing region, client token).
//!
//! The wire harness is reused from the `onboarding_loop` suite rather than copied.

use std::collections::HashSet;

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;

use crate::onboarding_loop::{
    b2bi_router, call, call_signed_for, error_code, given_transformer, is_generated_id, text,
    ACCOUNT, SIGNING_REGION,
};
// The shape id this service's wire discriminates a `Tag` object by, stated once for the
// whole test target in the suite that owns that rule.
use crate::tag_type_discriminator::TAG_WIRE_SHAPE_ID;
use crate::wire_timestamps_and_discriminators::as_wire_reports;

/// Every configuration member `CreateTransformerResponse` can echo, so a row can assert
/// that what it did NOT send comes back absent.
///
/// Source: the input/output members of `com.amazonaws.b2bi#CreateTransformer`
/// (`smithy_query(query="operation",operation="CreateTransformer")`) minus the
/// system-assigned ones (`transformerId`, `transformerArn`, `status`, `createdAt`) and
/// `name`.
pub(crate) const CONFIGURATION_MEMBERS: &[&str] = &[
    "ediType",
    "fileFormat",
    "mappingTemplate",
    "sampleDocument",
    "inputConversion",
    "mapping",
    "outputConversion",
    "sampleDocuments",
];

/// The four members the model marks `@deprecated` on `CreateTransformerRequest`, each
/// with a value its own shape accepts.
///
/// Source: the `deprecated` trait ("This is a legacy trait. Please use input-conversion or
/// output-conversion.") on those members; the values come from `X12TransactionSet` /
/// `X12Version` / `FileFormat` as `smithy_query` lists them.
pub(crate) fn legacy_members() -> Vec<(&'static str, Value)> {
    vec![
        (
            "ediType",
            json!({ "x12Details": { "transactionSet": "X12_850", "version": "VERSION_4010" } }),
        ),
        ("fileFormat", json!("JSON")),
        ("mappingTemplate", json!("{\"id\": payload.id}")),
        ("sampleDocument", json!("s3://b2bi-samples/850.edi")),
    ]
}

/// The members that replace [`legacy_members`], each with a value its own shape accepts.
///
/// Source: `InputConversion$fromFormat` / `OutputConversion$toFormat` (`FromFormat` and
/// `ToFormat` both accept only `X12`), `Mapping$templateLanguage` (`JSONATA` | `XSLT`) and
/// `SampleDocuments` (`bucketName` + `keys`), as `smithy_query` reports them.
pub(crate) fn current_members() -> Vec<(&'static str, Value)> {
    vec![
        (
            "inputConversion",
            json!({
                "fromFormat": "X12",
                "formatOptions": {
                    "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                }
            }),
        ),
        (
            "mapping",
            json!({ "templateLanguage": "JSONATA", "template": "{\"id\": id}" }),
        ),
        (
            "outputConversion",
            json!({
                "toFormat": "X12",
                "formatOptions": {
                    "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                }
            }),
        ),
        (
            "sampleDocuments",
            json!({
                "bucketName": "b2bi-samples",
                "keys": [{ "input": "in/850.edi", "output": "out/850.json" }]
            }),
        ),
    ]
}

/// RULE: `CreateTransformer` mints a transformer, persists every member the request
/// supplied, and answers with that stored record — a system-assigned `transformerId`, an
/// ARN naming the request's signing region and the canonical account, a creation
/// timestamp, and the `inactive` status a transformer rests in until a caller activates
/// it; `GetTransformer` then reports the same record, member for member, and a member the
/// request omitted is absent from both answers.
///
/// The rows span both configuration styles (the current `inputConversion` / `mapping` /
/// `outputConversion` / `sampleDocuments` members and the deprecated `ediType` /
/// `fileFormat` / `mappingTemplate` / `sampleDocument` ones), several subsets of each,
/// every value of the `FileFormat` and `MappingTemplateLanguage` enums, a request with no
/// configuration member at all, tags present and absent, and two signing regions.
#[tokio::test]
async fn a_created_transformer_round_trips_every_persisted_member() {
    let router = b2bi_router();

    // (case, signing region, request body)
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "current style, inbound X12 with a JSONATA mapping and sample documents",
            SIGNING_REGION,
            json!({
                "name": "inbound-850",
                "inputConversion": current_members()[0].1,
                "mapping": current_members()[1].1,
                "sampleDocuments": current_members()[3].1,
            }),
        ),
        (
            "current style, outbound X12 with an XSLT mapping",
            SIGNING_REGION,
            json!({
                "name": "outbound-850",
                "outputConversion": current_members()[2].1,
                "mapping": {
                    "templateLanguage": "XSLT",
                    "template": "<xsl:stylesheet version=\"1.0\"/>"
                },
            }),
        ),
        (
            "current style, inputConversion only",
            "eu-central-1",
            json!({
                "name": "inbound_minimal",
                "inputConversion": current_members()[0].1,
            }),
        ),
        (
            "legacy style, every deprecated member, fileFormat JSON",
            SIGNING_REGION,
            json!({
                "name": "legacy-json",
                "ediType": legacy_members()[0].1,
                "fileFormat": "JSON",
                "mappingTemplate": legacy_members()[2].1,
                "sampleDocument": legacy_members()[3].1,
                "tags": [{ "Key": "team", "Value": "edi" }],
            }),
        ),
        (
            "legacy style, fileFormat XML only",
            "ap-southeast-2",
            json!({ "name": "legacy-xml", "fileFormat": "XML" }),
        ),
        (
            "legacy style, fileFormat NOT_USED with an ediType",
            SIGNING_REGION,
            json!({
                "name": "legacy-not-used",
                "fileFormat": "NOT_USED",
                "ediType": {
                    "x12Details": { "transactionSet": "X12_999", "version": "VERSION_5010" }
                },
            }),
        ),
        (
            // No recording or model constraint says the real service refuses an
            // unconfigured transformer, so this row must be STORED, not refused.
            "no configuration member at all",
            SIGNING_REGION,
            json!({ "name": "bare" }),
        ),
    ];

    let mut minted: HashSet<String> = HashSet::new();

    for (case, region, request) in &cases {
        let (status, created) =
            call_signed_for(&router, region, "CreateTransformer", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateTransformer must succeed, got {status} {created}"
        );

        let transformer_id = text(&created, "transformerId");
        assert!(
            is_generated_id(&transformer_id, "t"),
            "case {case:?}: expected a system-assigned t-<17 hex> transformerId, got {transformer_id:?}"
        );
        assert!(
            minted.insert(transformer_id.clone()),
            "case {case:?}: transformerId {transformer_id:?} was minted twice"
        );
        assert_eq!(
            text(&created, "transformerArn"),
            format!("arn:aws:b2bi:{region}:{ACCOUNT}:transformer/{transformer_id}"),
            "case {case:?}: the ARN must name the request's region and the emulator account"
        );
        assert_eq!(
            text(&created, "name"),
            text(request, "name"),
            "case {case:?}: the name must be echoed from the stored record"
        );
        assert_eq!(
            text(&created, "status"),
            "inactive",
            "case {case:?}: a newly created transformer rests inactive, got {:?}",
            created["status"]
        );
        assert!(
            !created["createdAt"].is_null(),
            "case {case:?}: createdAt must be present, got {:?}",
            created["createdAt"]
        );
        assert!(
            created["tags"].is_null(),
            "case {case:?}: CreateTransformerResponse models no tags member, got {:?}",
            created["tags"]
        );

        // A nested structure is reported with the `__type` this service's wire puts on its
        // shape, which `as_wire_reports` adds to the value the request carried.
        for member in CONFIGURATION_MEMBERS {
            assert_eq!(
                created[*member],
                as_wire_reports(member, &request[*member]),
                "case {case:?}: {member} must come back exactly as supplied (absent when \
                 omitted): got {:?}, sent {:?}",
                created[*member], request[*member]
            );
        }

        // Round trip: the reader reports the stored record, member for member.
        let (status, fetched) = call_signed_for(
            &router,
            region,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetTransformer must find the transformer just created, got \
             {status} {fetched}"
        );
        for member in ["transformerId", "transformerArn", "name", "status", "createdAt"]
            .iter()
            .chain(CONFIGURATION_MEMBERS.iter())
        {
            assert_eq!(
                fetched[*member], created[*member],
                "case {case:?}: GetTransformer disagrees with CreateTransformer on {member}: \
                 {:?} vs {:?}",
                fetched[*member], created[*member]
            );
        }
        assert_eq!(
            fetched["modifiedAt"], fetched["createdAt"],
            "case {case:?}: a transformer nobody has updated reports its creation instant as \
             modifiedAt, got {:?} against a createdAt of {:?}",
            fetched["modifiedAt"], fetched["createdAt"]
        );

        // Tags live on the stored record, so this service's tag reader reports exactly
        // the list the create supplied for the transformer's own ARN — an empty list when
        // it supplied none. Member names are `ResourceARN` / `Tags` per
        // `smithy_query(query="operation",operation="ListTagsForResource")`, and each
        // reported entry carries the `__type` discriminator this service's wire puts on a
        // `Tag` object beside its modelled members (see `tag_type_discriminator.rs`).
        let (status, listed) = call_signed_for(
            &router,
            region,
            "ListTagsForResource",
            json!({ "ResourceARN": text(&created, "transformerArn") }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a transformer's ARN must resolve for the tag reader, got \
             {status} {listed}"
        );
        let expected_tags: Value = Value::Array(
            request
                .get("tags")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|tag| {
                    json!({
                        "__type": TAG_WIRE_SHAPE_ID,
                        "Key": tag["Key"],
                        "Value": tag["Value"],
                    })
                })
                .collect(),
        );
        assert_eq!(
            listed["Tags"], expected_tags,
            "case {case:?}: expected the tags the create supplied, discriminated: \
             {expected_tags}, got {:?}",
            listed["Tags"]
        );
    }
}

/// RULE: a transformer is configured in exactly ONE of the two styles the model offers —
/// the members it marks `@deprecated` or the members that replace them — so a request
/// naming at least one of each is refused with the modelled `ValidationException`
/// (HTTP 400), naming the members it sent in each style, and stores nothing.
///
/// Source: the recorded AWS behaviour for
/// `b2bi.create-transformer-mixed-legacy-and-new-fields-error` (a mixed request is refused
/// with `ValidationException` and creates nothing), whose message is one FIXED sentence
/// stating the two sets of fields and that they must not be mixed — it reports none of the
/// members the request supplied.
///
/// The rows are every (deprecated member, replacement member) pair: 4 x 4 = 16, so the
/// rule is asserted over the whole cross product rather than the one pair the corpus
/// recorded.
#[tokio::test]
async fn mixing_the_deprecated_and_current_configuration_members_is_refused() {
    use service_com_amazonaws_b2bi_b2bi::ops::create_transformer::MIXED_CONFIGURATION_STYLES;

    let router = b2bi_router();
    let shared_token = "mixed-style-token";

    for (legacy_member, legacy_value) in legacy_members() {
        for (current_member, current_value) in current_members() {
            let case = format!("{legacy_member} + {current_member}");
            let mut request = json!({
                "name": "mixed-style",
                "clientToken": shared_token,
            });
            request[legacy_member] = legacy_value.clone();
            request[current_member] = current_value.clone();

            let (status, body) = call(&router, "CreateTransformer", request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a mixed-style request must be refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            // `ValidationException`'s member is `Message` (capital M) in this service's
            // model, unlike `ResourceNotFoundException`'s `message` —
            // `smithy_query(query="shape",shape="ValidationException")`.
            let message = text(&body, "Message");
            assert_eq!(
                message, MIXED_CONFIGURATION_STYLES,
                "case {case:?}: the refusal states the rule in fixed text, got {message:?} \
                 in {body}"
            );
        }
    }

    // Nothing was stored, and nothing was remembered for the shared clientToken: a
    // well-formed create reusing that token still mints a transformer of its own.
    let (status, created) = call(
        &router,
        "CreateTransformer",
        json!({
            "name": "single-style",
            "clientToken": shared_token,
            "inputConversion": current_members()[0].1,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "a single-style create reusing the refused token must succeed, got {status} {created}"
    );
    assert!(
        is_generated_id(&text(&created, "transformerId"), "t"),
        "the refused requests must not have left a transformer behind for that token, got {created}"
    );
}

/// RULE: `clientToken` is this operation's `@idempotencyToken`, so repeating a create
/// with the same token answers with the transformer the first call created — same id,
/// same ARN, same timestamp — once that create has settled, while a duplicate sent WHILE
/// it is in flight is refused with `ConflictException` (409); a different token mints a
/// different transformer.
///
/// The rows span both configuration styles and a token reused twice as well as once.
#[tokio::test]
async fn repeating_a_create_with_the_same_client_token_returns_the_same_transformer() {
    use service_com_amazonaws_b2bi_b2bi::state::IDEMPOTENT_CREATE_IN_FLIGHT;

    let router = b2bi_router();

    // (case, the configuration this row creates, how many replays it sends)
    let cases: Vec<(&str, Value, usize)> = vec![
        (
            "current style",
            json!({ "inputConversion": current_members()[0].1 }),
            1,
        ),
        (
            "legacy style",
            json!({ "fileFormat": "XML", "mappingTemplate": "{}" }),
            3,
        ),
    ];

    let mut minted: HashSet<String> = HashSet::new();

    for (index, (case, configuration, replays)) in cases.iter().enumerate() {
        let request = |token: &str| {
            let mut body = json!({
                "name": format!("idempotent-{index}"),
                "clientToken": format!("{token}-{index}"),
            });
            for (member, value) in configuration.as_object().expect("an object of members") {
                body[member] = value.clone();
            }
            body
        };

        let (status, first) = call(&router, "CreateTransformer", request("token-alpha")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first create must succeed, got {status} {first}"
        );
        let first_id = text(&first, "transformerId");
        assert!(
            minted.insert(first_id.clone()),
            "case {case:?}: transformerId {first_id:?} was minted twice"
        );

        // While the create is still in flight, a duplicate is the racer AWS refuses with
        // ConflictException; once it has settled, the same token answers with the
        // transformer it minted, however often it is repeated.
        let (status, raced) = call(&router, "CreateTransformer", request("token-alpha")).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a duplicate sent while the create is in flight must be \
             refused with ConflictException, got {status} {raced}"
        );
        tokio::time::sleep(IDEMPOTENT_CREATE_IN_FLIGHT).await;

        for replay_index in 0..*replays {
            let (status, replay) = call(&router, "CreateTransformer", request("token-alpha")).await;
            assert_eq!(
                status, 200,
                "case {case:?}: replay #{replay_index} must succeed, got {status} {replay}"
            );
            for member in ["transformerId", "transformerArn", "createdAt", "status"] {
                assert_eq!(
                    replay[member], first[member],
                    "case {case:?}: replay #{replay_index} must answer with the first \
                     transformer's {member}: {:?} vs {:?}",
                    replay[member], first[member]
                );
            }
        }

        let (status, other) = call(&router, "CreateTransformer", request("token-beta")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a different token must mint a new transformer, got {status} {other}"
        );
        let other_id = text(&other, "transformerId");
        assert_ne!(
            other_id, first_id,
            "case {case:?}: a different clientToken must mint a different transformer"
        );
        assert!(
            minted.insert(other_id.clone()),
            "case {case:?}: transformerId {other_id:?} was minted twice"
        );
    }
}

/// RULE: `GetTransformer` for an id this account's store does not hold is the modelled
/// `ResourceNotFoundException` at this service's not-found status — never an empty
/// success — whatever the id looks like, including an id another b2bi resource kind really
/// does hold.
#[tokio::test]
async fn getting_an_unknown_transformer_is_refused_as_not_found() {
    let router = b2bi_router();

    // The capability this test borrows an id from needs a transformer to name, so one is
    // created first; its own id is NOT among the rows below.
    let host_transformer = given_transformer(&router, "host-transformer").await;
    crate::onboarding_loop::given_capability_buckets(&router).await;
    let (status, created) = call(
        &router,
        "CreateCapability",
        json!({
            "name": "edi-850",
            "type": "edi",
            "configuration": {
                "edi": {
                    "type": {
                        "x12Details": { "transactionSet": "X12_850", "version": "VERSION_4010" }
                    },
                    "inputLocation": { "bucketName": "inbound-bucket", "key": "in/" },
                    "outputLocation": { "bucketName": "outbound-bucket", "key": "out/" },
                    "transformerId": host_transformer
                }
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the capability this test borrows an id from must be created, got {status} {created}"
    );
    let capability_id = text(&created, "capabilityId");

    // (case, a transformerId satisfying the modelled len 1..64 / ^[a-zA-Z0-9_-]+$)
    let cases: Vec<(&str, String)> = vec![
        ("a never-minted transformer id", "t-0123456789abcdef0".to_string()),
        ("a one-character id", "t".to_string()),
        ("an id of the maximum modelled length", "t".repeat(64)),
        ("an id with every permitted character class", "T-9_z".to_string()),
        ("a capability id that really exists", capability_id),
    ];

    for (case, transformer_id) in &cases {
        let (status, body) = call(
            &router,
            "GetTransformer",
            json!({ "transformerId": transformer_id }),
        )
        .await;
        assert_eq!(
            status,
            NOT_FOUND_STATUS,
            "case {case:?}: expected {NOT_FOUND_STATUS} for {transformer_id:?}, got {status} \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        assert!(
            text(&body, "message").contains(transformer_id),
            "case {case:?}: the refusal must name the id that was not found, got {:?}",
            text(&body, "message")
        );
    }
}

/// RULE: `CreateTransformer` refuses once the transformer store is at the emulator's
/// residency bound (`MAX_RECORDS_PER_STORE`), with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) stating AWS's quota sentence for
/// transformers and the account — the same ceiling the service's other creates hold, so
/// repeated creates cannot grow the process's memory without limit.
#[tokio::test]
async fn creating_a_transformer_is_refused_once_the_store_is_at_the_residency_bound() {
    let router = b2bi_router();
    let bound = service_com_amazonaws_b2bi_b2bi::state::MAX_RECORDS_PER_STORE;

    let request = |index: usize| {
        json!({
            "name": format!("transformer-{index}"),
            "inputConversion": current_members()[0].1,
        })
    };

    for index in 0..bound {
        let (status, response) = call(&router, "CreateTransformer", request(index)).await;
        assert_eq!(
            status, 200,
            "create #{index} below the {bound}-record bound must succeed, got {status} {response}"
        );
    }

    let (status, response) = call(&router, "CreateTransformer", request(bound)).await;
    assert_eq!(
        status, 402,
        "a create past the {bound}-record bound must be refused, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {response}"
    );
    let expected_message = format!(
        "Transformer limit reached for AWS account {}",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert_eq!(
        text(&response, "message"),
        expected_message,
        "the refusal must state the quota sentence {expected_message:?}, got {:?}",
        text(&response, "message")
    );
}
