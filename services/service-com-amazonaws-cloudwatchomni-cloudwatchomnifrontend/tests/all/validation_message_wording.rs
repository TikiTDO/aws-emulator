// SPDX-License-Identifier: CC0-1.0

//! How CloudWatchOmni words a request refused by the modelled input constraints.
//!
//! Every modelled operation of this service declares `ValidationException` (HTTP 400), and
//! a request that violates a declared constraint is refused with it BEFORE any handler
//! runs. The message is a requirement of that refusal, so this suite drives real requests
//! over the rpcv2Cbor wire and reads the message out of the error envelope.
//!
//! The rules asserted here, over a table spanning every constraint class the model declares
//! rather than one recorded request:
//!
//! * a clause names the offending MEMBER PATH and the REQUIREMENT it violated, and NOTHING
//!   of the value the caller sent — the one exception being a member absent from the
//!   request, stated as the `null` it was;
//! * an enumerated type's value SET is listed in the order the front door walks its own
//!   table of the admissible values (`service_common::validation_message::hashed_enum_order`)
//!   rather than in the order the model declares them. Every expectation below is DERIVED
//!   through that shared order rather than transcribed, so a second copy of the order that
//!   drifted from it fails here; [`the_permission_value_set_is_listed_as_the_recording_lists_it`]
//!   anchors the derivation on the one listing a recorded refusal of this service shows;
//! * the clauses sit in the envelope the service counts them with: `1 validation error
//!   detected: ` for one, `N validation errors detected: ` joined with `; ` for more.
//!
//! The rows differ along the constraint class (`@length` min and max, `@pattern`, `@range`
//! min and max, `@uniqueItems`, an enumerated value set, an absent required member), along
//! the member's position (a top-level member, a member of a nested structure, an element of
//! a list) and along the operation, so none of them is the recorded case restated.

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::CborCodec;
use service_common::validation_message::hashed_enum_order;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::types::{
    AccessGrantPrincipal, CreateAccessGrantInput, CreateDomainInput, CreateSpaceInput,
    EncryptionConfiguration, ListSpacesInput, CREATE_ACCESS_GRANT_INPUT_SCHEMA,
    CREATE_DOMAIN_INPUT_SCHEMA, CREATE_SPACE_INPUT_SCHEMA, LIST_SPACES_INPUT_SCHEMA,
};

/// The error code every constraint refusal of this service names, as the model declares it
/// with `@httpError 400`.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The status that error carries.
const VALIDATION_STATUS: u16 = 400;

/// A well-formed `DomainId` (its modelled `^d-[0-9a-z]{1,25}$`): the refusals below are
/// decided at the boundary, so nothing has to name an existing resource.
const DOMAIN_ID: &str = "d-0000000000";

/// A well-formed `SpaceId` (its modelled UUID pattern).
const SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A role ARN satisfying `Arn`'s modelled `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// The enumerated types the refused requests below carry, as the values they declare, in
/// DECLARATION order.
///
/// Source: `smithy_query(query="shape", shape="AccessGrantPermission,AccessGrantPrincipalType,
/// IdentityProvider,EncryptionStrategy")`. A row names the enum; the values are written once,
/// here, and every expectation reads the listing out of the shared order.
const MODELLED_ENUMS: &[(&str, &[&str])] = &[
    (
        "AccessGrantPermission",
        &["SPACE_ADMIN", "READ", "READ_WRITE_DELETE", "CUSTOM"],
    ),
    (
        "AccessGrantPrincipalType",
        &[
            "IDC_USER",
            "IDC_GROUP",
            "IAM_USER",
            "IAM_ROLE",
            "IAM_ROOT",
            "ACCESS_PROFILE",
            "ALERT",
            "AGENT",
        ],
    ),
    ("IdentityProvider", &["IAM", "IDC"]),
    ("EncryptionStrategy", &["AWS_OWNED", "CUSTOMER_MANAGED"]),
];

/// The values `name` declares, read out of [`MODELLED_ENUMS`].
fn declared(name: &str) -> &'static [&'static str] {
    MODELLED_ENUMS
        .iter()
        .find(|(enumerated, _)| *enumerated == name)
        .map(|(_, values)| *values)
        .unwrap_or_else(|| panic!("case {name:?}: this model declares no such enum"))
}

/// The whole message a refusal of a value outside `enumerated` must carry at `member`: the
/// counted envelope over one clause naming the member and the value set, with the set put
/// through the SHARED order rather than transcribed.
fn enum_message(member: &str, enumerated: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must satisfy enum value set: [{}]",
        hashed_enum_order(declared(enumerated)).join(", ")
    )
}

/// One emulator: the service mounted on a router, which is what applies the modelled
/// constraints to a request before any handler sees it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(omni::EmulatedService::new(engine));
    router
}

/// `input` as the rpcv2Cbor body the wire carries it in.
fn body<I: SerializableStruct>(input_schema: &Schema, input: &I) -> bytes::Bytes {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(input_schema, input)
        .expect("the input shape serializes");
    bytes::Bytes::from(serializer.finish())
}

/// Dispatch one rpcv2Cbor request and return its status and body.
async fn dispatch(operation: &str, request_body: bytes::Bytes) -> (u16, bytes::Bytes) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "/service/CloudWatchOmniFrontend/operation/{operation}"
        ))
        .header("content-type", "application/cbor")
        .header("smithy-protocol", "rpc-v2-cbor")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/us-west-2/cloudwatch/aws4_request, \
             SignedHeaders=host, Signature=emulator",
        )
        .body(request_body)
        .expect("a well-formed rpcv2Cbor request");
    let response = router().dispatch(request).await;
    (response.status().as_u16(), response.body().clone())
}

/// The `__type` and `message` of an rpcv2Cbor error envelope, decoded as CBOR rather than
/// scanned as text, so the message asserted on is the WHOLE message.
fn refusal(case: &str, body: &[u8]) -> (String, String) {
    let mut decoder = aws_smithy_cbor::Decoder::new(body);
    let members = decoder
        .map()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the error envelope is not a CBOR map: {error}")
        })
        .unwrap_or_else(|| panic!("case {case:?}: the error envelope is an indefinite-length map"));
    let mut code = None;
    let mut message = None;
    for _ in 0..members {
        let key = decoder
            .str()
            .unwrap_or_else(|error| panic!("case {case:?}: an envelope key is not text: {error}"))
            .into_owned();
        let value = decoder
            .str()
            .unwrap_or_else(|error| panic!("case {case:?}: an envelope value is not text: {error}"))
            .into_owned();
        match key.as_str() {
            "__type" => code = Some(value),
            "message" => message = Some(value),
            _ => {}
        }
    }
    (
        code.unwrap_or_else(|| panic!("case {case:?}: the envelope names no error type")),
        message.unwrap_or_else(|| panic!("case {case:?}: the envelope carries no message")),
    )
}

/// Require that `operation` refuses `request_body` with the modelled error, and return the
/// message it stated.
async fn refused(case: &str, operation: &str, request_body: bytes::Bytes) -> String {
    let (status, body) = dispatch(operation, request_body).await;
    let (code, message) = refusal(case, &body);
    assert_eq!(
        (status, code.as_str()),
        (VALIDATION_STATUS, VALIDATION_EXCEPTION),
        "case {case:?}: expected {VALIDATION_STATUS} {VALIDATION_EXCEPTION}, got {status} \
         {code:?}: {message:?}"
    );
    message
}

/// A `CreateDomainInput` with everything but `name` and `identityProviders` left out — both
/// are `@required`, so a row sets exactly what it means to refuse.
fn create_domain(name: &str, identity_providers: &[&str]) -> CreateDomainInput {
    CreateDomainInput {
        name: name.to_string(),
        identity_providers: identity_providers
            .iter()
            .map(|provider| provider.to_string())
            .collect(),
        ..Default::default()
    }
}

/// A `CreateAccessGrantInput` every member of which satisfies its constraints except the
/// `permission` and `principalType` the caller passes.
fn create_access_grant(permission: &str, principal_type: &str) -> CreateAccessGrantInput {
    CreateAccessGrantInput {
        domain_id: DOMAIN_ID.to_string(),
        space_id: SPACE_ID.to_string(),
        name: "grant-one".to_string(),
        principal: AccessGrantPrincipal {
            principal_type: principal_type.to_string(),
            principal_id: Some("role/OmniReader".to_string()),
            principal_attributes: None,
        },
        permission: permission.to_string(),
        ..Default::default()
    }
}

/// A `CreateSpaceInput` whose only questionable member is the encryption strategy, which
/// sits one level down in a nested structure.
fn create_space(encryption_strategy: &str) -> CreateSpaceInput {
    CreateSpaceInput {
        name: "wire-space".to_string(),
        domain_id: DOMAIN_ID.to_string(),
        data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
        encryption_configuration: Some(EncryptionConfiguration {
            encryption_strategy: encryption_strategy.to_string(),
            kms_key_arn: None,
        }),
        ..Default::default()
    }
}

/// A `ListSpacesInput` naming only a page size.
fn list_spaces(max_results: i32) -> ListSpacesInput {
    ListSpacesInput {
        max_results: Some(max_results),
        ..Default::default()
    }
}

/// One row of the table: which request, and the whole message the refusal must state.
struct Case {
    /// What the row is about, named in every assertion it makes.
    label: &'static str,
    operation: &'static str,
    request: bytes::Bytes,
    /// The value the request carried at the offending member, which the message must not
    /// render — empty for a row whose offending member carries no value.
    sent: String,
    message: String,
}

#[tokio::test]
async fn a_constraint_refusal_names_the_member_and_the_requirement_only() {
    // The `@length`/`@pattern`/`@range`/`@uniqueItems`/enum facts each row relies on are the
    // model's own, read with
    // `smithy_query(query="operation", operation="CreateDomain,CreateAccessGrant,CreateSpace,ListSpaces")`
    // and the shapes listed on `MODELLED_ENUMS`.
    let cases = vec![
        Case {
            label: "name shorter than @length min",
            operation: "CreateDomain",
            request: body(CREATE_DOMAIN_INPUT_SCHEMA, &create_domain("ab", &["IAM"])),
            sent: "ab".to_string(),
            message: "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                      Member must have length greater than or equal to 3"
                .to_string(),
        },
        Case {
            label: "name longer than @length max",
            operation: "CreateDomain",
            request: body(
                CREATE_DOMAIN_INPUT_SCHEMA,
                &create_domain(&"a".repeat(64), &["IAM"]),
            ),
            sent: "a".repeat(64),
            message: "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                      Member must have length less than or equal to 63"
                .to_string(),
        },
        Case {
            // The expression is quoted without the whole-value anchors the model wraps it in
            // (`service_common::validation::unanchored`), which is the rendering the shared
            // wording states.
            label: "name outside @pattern",
            operation: "CreateDomain",
            request: body(
                CREATE_DOMAIN_INPUT_SCHEMA,
                &create_domain("Not-A-Domain", &["IAM"]),
            ),
            sent: "Not-A-Domain".to_string(),
            message: "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                      Member must satisfy regular expression pattern: [a-z0-9]+(-[a-z0-9]+)*"
                .to_string(),
        },
        Case {
            label: "a list repeating an element under @uniqueItems",
            operation: "CreateDomain",
            request: body(
                CREATE_DOMAIN_INPUT_SCHEMA,
                &create_domain("omni-domain", &["IAM", "IAM"]),
            ),
            sent: String::new(),
            message: "1 validation error detected: Value at 'identityProviders' failed to \
                      satisfy constraint: Member must have unique values"
                .to_string(),
        },
        Case {
            label: "a list element outside its enum value set",
            operation: "CreateDomain",
            request: body(
                CREATE_DOMAIN_INPUT_SCHEMA,
                &create_domain("omni-domain", &["SAML"]),
            ),
            sent: "SAML".to_string(),
            message: enum_message("identityProviders.1.member", "IdentityProvider"),
        },
        Case {
            label: "an enumerated member outside its value set",
            operation: "CreateAccessGrant",
            request: body(
                CREATE_ACCESS_GRANT_INPUT_SCHEMA,
                &create_access_grant("BOGUS_PERM", "IAM_ROLE"),
            ),
            sent: "BOGUS_PERM".to_string(),
            message: enum_message("permission", "AccessGrantPermission"),
        },
        Case {
            // A longer set, in a nested structure, listed by the same order.
            label: "an enumerated member of a nested structure",
            operation: "CreateAccessGrant",
            request: body(
                CREATE_ACCESS_GRANT_INPUT_SCHEMA,
                &create_access_grant("READ", "NOBODY"),
            ),
            sent: "NOBODY".to_string(),
            message: enum_message("principal.principalType", "AccessGrantPrincipalType"),
        },
        Case {
            label: "an enumerated member of another operation's nested structure",
            operation: "CreateSpace",
            request: body(CREATE_SPACE_INPUT_SCHEMA, &create_space("PLAINTEXT")),
            sent: "PLAINTEXT".to_string(),
            message: enum_message(
                "encryptionConfiguration.encryptionStrategy",
                "EncryptionStrategy",
            ),
        },
        Case {
            label: "a page size below @range min",
            operation: "ListSpaces",
            request: body(LIST_SPACES_INPUT_SCHEMA, &list_spaces(0)),
            sent: String::new(),
            message: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                      constraint: Member must have value greater than or equal to 1"
                .to_string(),
        },
        Case {
            label: "a page size above @range max",
            operation: "ListSpaces",
            request: body(LIST_SPACES_INPUT_SCHEMA, &list_spaces(101)),
            sent: String::new(),
            message: "1 validation error detected: Value at 'maxResults' failed to satisfy \
                      constraint: Member must have value less than or equal to 100"
                .to_string(),
        },
        Case {
            label: "two members violating a constraint each",
            operation: "CreateDomain",
            request: body(
                CREATE_DOMAIN_INPUT_SCHEMA,
                &create_domain("ab", &["IAM", "IAM"]),
            ),
            sent: "ab".to_string(),
            message: "2 validation errors detected: Value at 'name' failed to satisfy \
                      constraint: Member must have length greater than or equal to 3; Value at \
                      'identityProviders' failed to satisfy constraint: Member must have unique \
                      values"
                .to_string(),
        },
        Case {
            // An empty CBOR map is a request that names no member at all, so the first
            // `@required` member of the input is absent rather than ill-valued.
            label: "a required member absent from the request",
            operation: "CreateDomain",
            request: bytes::Bytes::from_static(&[0xa0]),
            sent: String::new(),
            message: "1 validation error detected: Value null at 'name' failed to satisfy \
                      constraint: Member must not be null"
                .to_string(),
        },
    ];

    for case in cases {
        let actual = refused(case.label, case.operation, case.request.clone()).await;
        assert_eq!(
            actual, case.message,
            "case {:?}: expected {:?}, got {:?}",
            case.label, case.message, actual
        );
        if !case.sent.is_empty() {
            assert!(
                !actual.contains(&case.sent),
                "case {:?}: the message renders the value the caller sent ({:?}): {actual:?}",
                case.label,
                case.sent
            );
        }
    }
}

/// ANCHOR for the derived expectations above: the one enumerated value set a recorded
/// refusal of this service shows is listed `[CUSTOM, READ_WRITE_DELETE, READ, SPACE_ADMIN]`,
/// while the model declares it `[SPACE_ADMIN, READ, READ_WRITE_DELETE, CUSTOM]` — so the
/// listing is the table's order and NOT the declaration.
#[test]
fn the_permission_value_set_is_listed_as_the_recording_lists_it() {
    const RECORDED: &[&str] = &["CUSTOM", "READ_WRITE_DELETE", "READ", "SPACE_ADMIN"];

    let model = declared("AccessGrantPermission");
    let listed = hashed_enum_order(model);
    assert_eq!(
        listed, RECORDED,
        "the listing must be the recorded one, got {listed:?}"
    );
    assert_ne!(
        listed,
        model.to_vec(),
        "the listing must not be the model's declaration, got {model:?}"
    );
}

/// GUARD on the derivation, over every enumerated type the rows above carry: the listing
/// holds exactly the values the type admits — none added, none dropped, none repeated.
#[test]
fn every_modelled_value_set_is_listed_whole() {
    for (name, values) in MODELLED_ENUMS {
        let mut listed = hashed_enum_order(values);
        listed.sort_unstable();
        let mut declared_values = values.to_vec();
        declared_values.sort_unstable();
        assert_eq!(
            listed, declared_values,
            "case {name:?}: the listing must hold exactly the declared values, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_page_size_is_refused_only_outside_the_published_range() {
    // `ListSpaces.maxResults` is modelled `@range(min: 1, max: 100)`: both published bounds
    // are accepted and one step past each is refused.
    for max_results in [1, 100] {
        let (status, body) = dispatch(
            "ListSpaces",
            body(LIST_SPACES_INPUT_SCHEMA, &list_spaces(max_results)),
        )
        .await;
        assert_eq!(
            status,
            200,
            "case {max_results}: a page size inside the modelled range must be served, got \
             {status}: {:?}",
            String::from_utf8_lossy(&body)
        );
    }

    for (max_results, requirement) in [
        (0, "Member must have value greater than or equal to 1"),
        (101, "Member must have value less than or equal to 100"),
    ] {
        let label = format!("maxResults {max_results}");
        let message = refused(
            &label,
            "ListSpaces",
            body(LIST_SPACES_INPUT_SCHEMA, &list_spaces(max_results)),
        )
        .await;
        let expected = format!(
            "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: \
             {requirement}"
        );
        assert_eq!(
            message, expected,
            "case {label:?}: expected {expected:?}, got {message:?}"
        );
    }
}
