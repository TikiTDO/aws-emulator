// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's access profiles.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, so a create is observable by the reader that runs next.
//! Failures are the operations' own modelled errors, so each assertion checks the wire
//! facts that are requirements: the HTTP status and the error code.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * `CreateAccessProfile` mints a profile in an EXISTING space, persists it and
//!   returns the persisted record, and `GetAccessProfile` answers from that same store;
//! * every derived member (`profileId`, `arn`, `createdAt`/`updatedAt`, `profileType`,
//!   `assumeStatus`) is computed from the stored resource rather than echoed;
//! * a profile is addressed by its space AND its id, so neither an unknown id nor a
//!   known id under another space resolves;
//! * a name already identifying a profile of that space cannot identify a second one,
//!   while the same name in another space is a different profile;
//! * `@idempotencyToken` replays return the original profile.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row
//! of the table that broke.

use std::collections::HashMap;
use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_access_profile::{CreateAccessProfile, CreateAccessProfileError};
use omni::ops::create_domain::CreateDomain;
use omni::ops::create_space::CreateSpace;
use omni::ops::get_access_profile::{GetAccessProfile, GetAccessProfileError};
use omni::ops::types::{
    AccessProfile, CreateAccessProfileInput, CreateAccessProfileOutput, CreateDomainInput,
    CreateSpaceInput, GetAccessProfileInput, GetAccessProfileOutput,
    CREATE_ACCESS_PROFILE_INPUT_SCHEMA, GET_ACCESS_PROFILE_INPUT_SCHEMA,
};
use omni::state::{
    ACCESS_PROFILE_QUOTA, ACCESS_PROFILE_TYPE_CUSTOMER_MANAGED, ASSUME_STATUS_DENIED,
};

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN below is expected to carry it.
const REGION: &str = "us-west-2";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// `ProfileId`'s modelled `@pattern`, with its `@length(1..64)`.
const PROFILE_ID_PATTERN: &str = r"^[a-zA-Z0-9_-]+$ (length 1..64)";

/// A well-formed `SpaceId` (its modelled pattern) that names nothing.
const MISSING_SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A well-formed `ProfileId` that names nothing.
const MISSING_PROFILE_ID: &str = "no-such-profile";

/// Whether `value` satisfies [`PROFILE_ID_PATTERN`].
///
/// Checked by hand rather than through a regex engine, so this crate declares no
/// dependency beyond the ones it already has. The predicate is exercised on values that
/// must FAIL it as well as pass ([`the_profile_id_pattern_is_checked_as_written`]).
fn is_profile_id(value: &str) -> bool {
    (1..=64).contains(&value.chars().count())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Whether `value` satisfies `Arn`'s modelled `^arn:.+$`.
fn is_arn(value: &str) -> bool {
    value
        .strip_prefix("arn:")
        .is_some_and(|rest| !rest.is_empty())
}

/// A timestamp at millisecond resolution — the resolution rpcv2Cbor encodes a
/// `Timestamp` at, so a value compared across a wire round trip is compared there.
fn millis(at: aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos() / 1_000_000)
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key) and the region the handlers record.
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/cloudwatch/aws4_request, \
                 SignedHeaders=host, Signature=emulator"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below all hold the SAME
        // engine, so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// A space to create profiles in, through the handlers that own those resources.
    async fn space(&self, case: &str, name: &str) -> String {
        let domain = CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: format!("omni-{name}"),
                    identity_providers: vec!["IAM".to_string()],
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain;
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id: domain.domain_id,
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space
            .space_id
    }

    async fn create_access_profile(
        &self,
        input: &CreateAccessProfileInput,
    ) -> Result<CreateAccessProfileOutput, CreateAccessProfileError> {
        CreateAccessProfile::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_access_profile(
        &self,
        space_id: &str,
        profile_id: &str,
    ) -> Result<GetAccessProfileOutput, GetAccessProfileError> {
        GetAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                GetAccessProfileInput {
                    space_id: space_id.to_string(),
                    profile_id: profile_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and body.
    async fn over_the_wire<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor");
        for (name, value) in &self.parts.headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// Require a modelled failure with the status and error code the model declares for it.
fn expect_modelled_error<O: std::fmt::Debug, E: OperationError + std::fmt::Debug>(
    case: &str,
    result: Result<O, E>,
    expected_status: u16,
    expected_code: &str,
) {
    match result {
        Ok(output) => panic!(
            "case {case:?}: expected {expected_code} ({expected_status}), got Ok({output:?})"
        ),
        Err(error) => {
            assert_eq!(
                error.shape_name(),
                expected_code,
                "case {case:?}: expected error code {expected_code}, got {error:?}"
            );
            assert_eq!(
                error.http_status(),
                expected_status,
                "case {case:?}: expected HTTP {expected_status} for {expected_code}, got {}",
                error.http_status()
            );
        }
    }
}

fn create_input(space_id: &str, name: &str) -> CreateAccessProfileInput {
    CreateAccessProfileInput {
        space_id: space_id.to_string(),
        name: name.to_string(),
        ..Default::default()
    }
}

/// One row of the `CreateAccessProfile` table: the request members that vary.
struct ProfileCase {
    case: &'static str,
    name: String,
    description: Option<String>,
    tags: Option<HashMap<String, String>>,
}

/// The rows span what the rule generalises over: `AccessProfileName`'s whole
/// `@length(1..256)` and its `^[a-zA-Z0-9 _.:/=+@-]+$` character class, a `description`
/// that is absent, empty and at `@length(max: 1024)`, and `tags` absent, one and many.
fn profile_cases() -> Vec<ProfileCase> {
    vec![
        ProfileCase {
            case: "the shortest name the model admits, nothing else",
            name: "a".to_string(),
            description: None,
            tags: None,
        },
        ProfileCase {
            case: "spaces and the whole punctuation class",
            name: "Read Only _.:/=+@- profile".to_string(),
            description: Some("an optional description".to_string()),
            tags: Some(HashMap::from([("team".to_string(), "sre".to_string())])),
        },
        ProfileCase {
            case: "the longest name the model admits",
            name: "n".repeat(256),
            description: Some(String::new()),
            tags: Some(HashMap::new()),
        },
        ProfileCase {
            case: "the longest description the model admits, many tags",
            name: "many-tags".to_string(),
            description: Some("d".repeat(1024)),
            tags: Some(
                (0..50)
                    .map(|index| (format!("key{index}"), format!("value{index}")))
                    .collect(),
            ),
        },
    ]
}

/// Every member of a profile, as a comparable tuple.
type ProfileFields = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Every member of a profile but its name and timestamps, as a comparable tuple.
fn fields(profile: &AccessProfile) -> ProfileFields {
    (
        profile.profile_id.clone(),
        profile.space_id.clone(),
        profile.arn.clone(),
        profile.description.clone(),
        profile.assume_status.clone(),
        profile.profile_type.clone(),
    )
}

#[tokio::test]
async fn the_profile_id_pattern_is_checked_as_written() {
    // Each row: the value, and whether the modelled pattern and length admit it.
    for (value, expected) in [
        ("abc123", true),
        ("a", true),
        ("A-b_c", true),
        ("8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6d", true),
        (&"p".repeat(64), true),
        (&"p".repeat(65), false),
        ("", false),
        ("has space", false),
        ("dots.are.out", false),
    ] {
        assert_eq!(
            is_profile_id(value),
            expected,
            "case {value:?}: {PROFILE_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [
        ("arn:aws:cloudwatch:us-west-2:000000000000:access-profile/p", true),
        ("arn:x", true),
        ("arn:", false),
        ("aws:arn:thing", false),
    ] {
        assert_eq!(
            is_arn(value),
            expected,
            "case {value:?}: ^arn:.+$ should {} it",
            if expected { "admit" } else { "reject" }
        );
    }
}

#[tokio::test]
async fn create_access_profile_persists_a_record_get_access_profile_returns_it() {
    let omni = Omni::new();
    let space_id = omni.space("table", "profile-table").await;
    let mut minted: Vec<String> = Vec::new();

    for row in profile_cases() {
        let case = row.case;
        let created = omni
            .create_access_profile(&CreateAccessProfileInput {
                space_id: space_id.clone(),
                name: row.name.clone(),
                description: row.description.clone(),
                tags: row.tags.clone(),
                client_token: None,
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessProfile failed: {error:?}"))
            .access_profile;

        // The id is minted by the service, inside what `ProfileId` models.
        assert!(
            is_profile_id(&created.profile_id),
            "case {case:?}: profileId {:?} does not satisfy {PROFILE_ID_PATTERN}",
            created.profile_id
        );
        assert!(
            !minted.contains(&created.profile_id),
            "case {case:?}: profileId {:?} repeats an earlier profile's id",
            created.profile_id
        );
        minted.push(created.profile_id.clone());

        // The ARN is derived from the parent space and this profile, in the signed
        // region, and satisfies `Arn`'s pattern.
        assert!(
            is_arn(&created.arn),
            "case {case:?}: arn {:?} does not satisfy ^arn:.+$",
            created.arn
        );
        assert!(
            created
                .arn
                .ends_with(&format!(":access-profile/{}", created.profile_id)),
            "case {case:?}: arn {:?} does not name this profile alone",
            created.arn
        );
        assert!(
            created.arn.contains(&format!(":{REGION}:")),
            "case {case:?}: arn {:?} does not carry the signed region {REGION}",
            created.arn
        );

        // The request's own members come back as sent, and the derived ones are the
        // documented values for a freshly created, customer-created profile.
        assert_eq!(
            (
                created.space_id.as_str(),
                created.name.as_str(),
                created.description.as_deref(),
                created.profile_type.as_deref(),
                created.assume_status.as_deref(),
            ),
            (
                space_id.as_str(),
                row.name.as_str(),
                row.description.as_deref(),
                Some(ACCESS_PROFILE_TYPE_CUSTOMER_MANAGED),
                Some(ASSUME_STATUS_DENIED),
            ),
            "case {case:?}: the created profile is {created:?}"
        );
        assert_eq!(
            millis(created.created_at),
            millis(created.updated_at),
            "case {case:?}: a profile nothing has updated reports createdAt {:?} and updatedAt {:?}",
            created.created_at,
            created.updated_at
        );

        // The reader answers from the store, verbatim and stably.
        for read in ["first read", "second read"] {
            let fetched = omni
                .get_access_profile(&space_id, &created.profile_id)
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?} ({read}): GetAccessProfile failed: {error:?}")
                })
                .access_profile;
            assert_eq!(
                fields(&fetched),
                fields(&created),
                "case {case:?} ({read}): the stored profile is {fetched:?}, the created one {created:?}"
            );
            assert_eq!(
                (millis(fetched.created_at), millis(fetched.updated_at)),
                (millis(created.created_at), millis(created.updated_at)),
                "case {case:?} ({read}): the stored timestamps differ from the created ones"
            );
            assert_eq!(
                fetched.name, created.name,
                "case {case:?} ({read}): expected name {:?}, got {:?}",
                created.name, fetched.name
            );
        }
    }
}

#[tokio::test]
async fn a_profile_is_addressed_by_its_space_and_its_id() {
    let omni = Omni::new();
    let home = omni.space("home", "profile-home").await;
    let other = omni.space("other", "profile-other").await;
    let created = omni
        .create_access_profile(&create_input(&home, "addressed"))
        .await
        .expect("CreateAccessProfile in an existing space")
        .access_profile;

    // Rows: the id under its own space resolves; everything else is the modelled 404.
    omni.get_access_profile(&home, &created.profile_id)
        .await
        .expect("case \"its own space\": the profile resolves");
    for (case, space_id, profile_id) in [
        (
            "a known id under another space",
            other.as_str(),
            created.profile_id.as_str(),
        ),
        ("an unknown id", home.as_str(), MISSING_PROFILE_ID),
        (
            "a space the store does not hold",
            MISSING_SPACE_ID,
            created.profile_id.as_str(),
        ),
    ] {
        expect_modelled_error(
            case,
            omni.get_access_profile(space_id, profile_id).await,
            404,
            "ResourceNotFoundException",
        );
    }

    // And a create names its parent: a space the store does not hold is the same 404.
    expect_modelled_error(
        "a create in a space the store does not hold",
        omni.create_access_profile(&create_input(MISSING_SPACE_ID, "orphan"))
            .await,
        404,
        "ResourceNotFoundException",
    );
}

#[tokio::test]
async fn a_name_identifies_one_profile_per_space() {
    let omni = Omni::new();
    let first = omni.space("first", "profile-names-a").await;
    let second = omni.space("second", "profile-names-b").await;

    // Rows span the name class again: each is created once, refused the second time in
    // the same space, and accepted in another space.
    for name in ["a", "Read Only _.:/=+@- profile", &"n".repeat(256)] {
        let created = omni
            .create_access_profile(&create_input(&first, name))
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: the first create failed: {error:?}"))
            .access_profile;
        expect_modelled_error(
            name,
            omni.create_access_profile(&create_input(&first, name)).await,
            409,
            "ConflictException",
        );
        let elsewhere = omni
            .create_access_profile(&create_input(&second, name))
            .await
            .unwrap_or_else(|error| {
                panic!("case {name:?}: the same name in another space failed: {error:?}")
            })
            .access_profile;
        assert_ne!(
            elsewhere.profile_id, created.profile_id,
            "case {name:?}: the same name in another space is a different profile, got {:?} twice",
            elsewhere.profile_id
        );
        assert_eq!(
            elsewhere.space_id, second,
            "case {name:?}: expected spaceId {second}, got {:?}",
            elsewhere.space_id
        );
    }
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_original_profile() {
    let omni = Omni::new();
    let space_id = omni.space("tokens", "profile-tokens").await;

    // Rows span `ClientToken`'s `@length(1..64)` and its `^[!-~]+$` character class.
    for (index, token) in ["!", "a-token", "~{}[]|\\^`", &"t".repeat(64)]
        .into_iter()
        .enumerate()
    {
        let input = CreateAccessProfileInput {
            client_token: Some(token.to_string()),
            ..create_input(&space_id, &format!("token-profile-{index}"))
        };
        let created = omni
            .create_access_profile(&input)
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: the first create failed: {error:?}"))
            .access_profile;
        let replayed = omni
            .create_access_profile(&input)
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: the replay failed: {error:?}"))
            .access_profile;
        assert_eq!(
            fields(&replayed),
            fields(&created),
            "case {token:?}: the replay returned {replayed:?} rather than the original {created:?}"
        );

        // A DIFFERENT token for the same request is a different profile, so the replay
        // is the token's doing rather than the name's.
        let fresh = omni
            .create_access_profile(&CreateAccessProfileInput {
                client_token: Some(format!("{token}x")),
                name: format!("token-profile-{index}-fresh"),
                ..input.clone()
            })
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: a fresh token failed: {error:?}"))
            .access_profile;
        assert_ne!(
            fresh.profile_id, created.profile_id,
            "case {token:?}: a fresh token must mint a new profile, got {:?} twice",
            fresh.profile_id
        );
    }
}

#[tokio::test]
async fn the_access_profile_store_is_bounded_and_refuses_past_its_quota() {
    // The store cannot grow without end: `CreateAccessProfile` refuses with the
    // modelled `ServiceQuotaExceededException` once `ACCESS_PROFILE_QUOTA` records are
    // held. Each row carries its own name, so the refusal is the bound rather than the
    // duplicate-name conflict.
    let omni = Omni::new();
    let space_id = omni.space("quota", "profile-quota").await;
    for index in 0..ACCESS_PROFILE_QUOTA {
        omni.create_access_profile(&create_input(&space_id, &format!("quota-{index}")))
            .await
            .unwrap_or_else(|error| panic!("profile {index} within the quota failed: {error:?}"));
    }
    expect_modelled_error(
        "one profile past ACCESS_PROFILE_QUOTA",
        omni.create_access_profile(&create_input(&space_id, "quota-overflow"))
            .await,
        402,
        "ServiceQuotaExceededException",
    );
}

#[tokio::test]
async fn the_access_profile_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    // The parent space is created through the handlers holding the same engine the
    // router's service holds, which is what makes the wire calls below see it.
    let space_id = omni.space("wire", "profile-wire").await;

    let codec = CborCodec::default();
    let (status, body) = omni
        .over_the_wire(
            "CreateAccessProfile",
            CREATE_ACCESS_PROFILE_INPUT_SCHEMA,
            &create_input(&space_id, "wire profile"),
        )
        .await;
    assert_eq!(
        status,
        200,
        "CreateAccessProfile over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let created = {
        let mut deserializer = codec.create_deserializer(&body);
        CreateAccessProfileOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .access_profile
    };
    assert!(
        is_profile_id(&created.profile_id),
        "profileId {:?} does not satisfy {PROFILE_ID_PATTERN}",
        created.profile_id
    );

    // The profile the wire call created is readable over the wire too, as persisted.
    let (status, body) = omni
        .over_the_wire(
            "GetAccessProfile",
            GET_ACCESS_PROFILE_INPUT_SCHEMA,
            &GetAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: created.profile_id.clone(),
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "GetAccessProfile over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let fetched = {
        let mut deserializer = codec.create_deserializer(&body);
        GetAccessProfileOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .access_profile
    };
    assert_eq!(
        fields(&fetched),
        fields(&created),
        "the wire read answered {fetched:?} for the created {created:?}"
    );
    assert_eq!(
        (millis(fetched.created_at), millis(fetched.updated_at)),
        (millis(created.created_at), millis(created.updated_at)),
        "the wire read and the create disagree on the timestamps"
    );

    // A miss over the wire is the modelled 404 with its modelled error code.
    let (status, body) = omni
        .over_the_wire(
            "GetAccessProfile",
            GET_ACCESS_PROFILE_INPUT_SCHEMA,
            &GetAccessProfileInput {
                space_id: space_id.clone(),
                profile_id: MISSING_PROFILE_ID.to_string(),
            },
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 404,
        "GetAccessProfile over the wire answered {status} for a missing profile: {rendered:?}"
    );
    assert!(
        rendered.contains("ResourceNotFoundException"),
        "the wire error body does not name ResourceNotFoundException: {rendered:?}"
    );

    // A modelled constraint is refused before the handler runs, as a 400.
    let (status, body) = omni
        .over_the_wire(
            "CreateAccessProfile",
            CREATE_ACCESS_PROFILE_INPUT_SCHEMA,
            &create_input(&space_id, &"n".repeat(257)),
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 400,
        "a name over @length(max: 256) answered {status}: {rendered:?}"
    );
    assert!(
        rendered.contains("ValidationException"),
        "the wire refusal does not name ValidationException: {rendered:?}"
    );
}
