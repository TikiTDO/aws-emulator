// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's domain -> space lifecycle.
//!
//! Every call goes through a real operation handler holding the SAME shared engine
//! the router hands every service, so a create is observable by the reader that runs
//! next — exactly the store contract `tests/all/state_roundtrip.rs` pins. Failures
//! are the operations' own modelled errors, so each assertion checks the wire facts
//! that are requirements: the HTTP status and the error code.
//!
//! [`the_space_lifecycle_over_the_rpcv2cbor_wire`] additionally drives the same store
//! over HTTP through the framework's [`MultiServiceRouter`], so routing,
//! deserialization, the handler and response serialization all run for the space
//! operations.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * a create persists, and the matching reader returns the persisted record
//!   verbatim and stably;
//! * every derived member (id shape, ARN, endpoint URL, region, owner account,
//!   status, encryption configuration) is computed from the stored resource;
//! * a reader answers `ResourceNotFoundException` for a resource the store does not
//!   hold, and a delete makes the reader answer that way;
//! * `@idempotencyToken` replays return the original resource;
//! * `ListSpaces` pages a stable creation order under every page bound, and its
//!   token resumes exactly where the previous page stopped;
//! * the modelled input constraints refuse what they declare.
//!
//! Each assertion names its case and the actual value, so a failure identifies the
//! row of the table that broke.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_domain::{CreateDomain, CreateDomainError};
use omni::ops::create_space::{CreateSpace, CreateSpaceError};
use omni::ops::delete_domain::{DeleteDomain, DeleteDomainError};
use omni::ops::delete_space::{DeleteSpace, DeleteSpaceError};
use omni::ops::get_domain::{GetDomain, GetDomainError};
use omni::ops::get_space::{GetSpace, GetSpaceError};
use omni::ops::list_spaces::{ListSpaces, ListSpacesError};
use omni::ops::types::{
    CreateDomainInput, CreateDomainOutput, CreateSpaceInput, CreateSpaceOutput, DeleteDomainInput,
    DeleteDomainOutput, DeleteSpaceInput, DeleteSpaceOutput, Domain, EncryptionConfiguration,
    GetDomainInput, GetDomainOutput, GetSpaceInput, GetSpaceOutput, IdentityCenterConfiguration,
    IdentityProviderConfiguration, ListSpacesInput, ListSpacesOutput, Space,
    CREATE_DOMAIN_INPUT_SCHEMA, CREATE_SPACE_INPUT_SCHEMA, GET_SPACE_INPUT_SCHEMA,
    LIST_SPACES_INPUT_SCHEMA,
};
use omni::state::{DOMAIN_QUOTA, SPACE_QUOTA};

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN and `region` member below is expected to carry it.
const REGION: &str = "us-west-2";

/// `DomainId`'s modelled `@pattern`.
const DOMAIN_ID_PATTERN: &str = r"^d-[0-9a-z]{1,25}$";

/// `SpaceId`'s modelled `@pattern`.
const SPACE_ID_PATTERN: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

/// `AccountId`'s modelled `@pattern`.
const ACCOUNT_ID_PATTERN: &str = r"^[0-9]{12}$";

/// `Arn`'s modelled `@pattern`.
const ARN_PATTERN: &str = r"^arn:.+$";

/// A KMS key ARN satisfying `KmsKeyArn`'s modelled
/// `^arn:aws[a-zA-Z-]*:kms:[a-z0-9-]+:[0-9]{12}:key/(mrk-)?[a-f0-9-]+$`.
const CUSTOMER_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6d";

/// A role ARN satisfying `Arn`'s `^arn:.+$`.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// A well-formed `DomainId` that names nothing.
const MISSING_DOMAIN_ID: &str = "d-doesnotexist";

/// A well-formed `SpaceId` that names nothing.
const MISSING_SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// Whether `value` satisfies [`DOMAIN_ID_PATTERN`].
///
/// The modelled `@pattern`s below are checked by hand rather than through a regex
/// engine, so this crate declares no dependency beyond the ones it already has and the
/// fix carries no `Cargo.lock` delta. Each predicate states the pattern it implements
/// and is exercised on values that must FAIL it as well as pass
/// ([`the_model_patterns_are_checked_as_written`]).
fn is_domain_id(value: &str) -> bool {
    // ^d-[0-9a-z]{1,25}$
    value.strip_prefix("d-").is_some_and(|body| {
        (1..=25).contains(&body.len())
            && body
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase())
    })
}

/// Whether `value` satisfies [`SPACE_ID_PATTERN`].
fn is_space_id(value: &str) -> bool {
    // ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8_usize, 4, 4, 4, 12])
            .all(|(group, width)| {
                group.len() == width
                    && group
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
}

/// Whether `value` satisfies [`ACCOUNT_ID_PATTERN`].
fn is_account_id(value: &str) -> bool {
    // ^[0-9]{12}$
    value.len() == 12 && value.chars().all(|c| c.is_ascii_digit())
}

/// Whether `value` satisfies [`ARN_PATTERN`].
fn is_arn(value: &str) -> bool {
    // ^arn:.+$
    value
        .strip_prefix("arn:")
        .is_some_and(|rest| !rest.is_empty() && !rest.contains('\n'))
}

#[tokio::test]
async fn the_model_patterns_are_checked_as_written() {
    // Each row: the value, and whether the modelled pattern admits it. The predicates
    // above are what every other assertion in this suite relies on, so they are pinned
    // against values on both sides of each pattern.
    let domain_ids = [
        ("d-abc123", true),
        ("d-a", true),
        (&format!("d-{}", "a".repeat(25)), true),
        (&format!("d-{}", "a".repeat(26)), false),
        ("d-", false),
        ("d-ABC", false),
        ("d-ab_c", false),
        ("abc123", false),
    ];
    for (value, expected) in domain_ids {
        assert_eq!(
            is_domain_id(value),
            expected,
            "case {value:?}: {DOMAIN_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    let space_ids = [
        ("00000000-0000-4000-8000-000000000000", true),
        ("8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6d", true),
        ("8A4F1B3C-0E2D-4A5B-9C7D-1F2E3A4B5C6D", false),
        ("8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6", false),
        ("8a4f1b3c0e2d4a5b9c7d1f2e3a4b5c6d", false),
        ("8a4f1b3g-0e2d-4a5b-9c7d-1f2e3a4b5c6d", false),
    ];
    for (value, expected) in space_ids {
        assert_eq!(
            is_space_id(value),
            expected,
            "case {value:?}: {SPACE_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [
        ("000000000000", true),
        ("123456789012", true),
        ("12345678901", false),
        ("1234567890123", false),
        ("12345678901a", false),
    ] {
        assert_eq!(
            is_account_id(value),
            expected,
            "case {value:?}: {ACCOUNT_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [
        ("arn:aws:iam::000000000000:role/OmniData", true),
        ("arn:x", true),
        ("arn:", false),
        ("aws:arn:thing", false),
    ] {
        assert_eq!(
            is_arn(value),
            expected,
            "case {value:?}: {ARN_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    // And the ids the service mints satisfy the patterns it models them against.
    let omni = Omni::new();
    let parent = domain(&omni, "minted", "omni-minted-ids", &["IAM"]).await;
    assert!(
        is_domain_id(&parent.domain_id),
        "case \"minted\": domainId {:?} does not satisfy {DOMAIN_ID_PATTERN}",
        parent.domain_id
    );
    let created = space(&omni, "minted", "minted-space", &parent.domain_id).await;
    assert!(
        is_space_id(&created.space_id),
        "case \"minted\": spaceId {:?} does not satisfy {SPACE_ID_PATTERN}",
        created.space_id
    );
}

/// A timestamp at millisecond resolution.
///
/// rpcv2Cbor encodes a `Timestamp` with millisecond resolution, so a value compared
/// across a wire round trip is compared at that resolution rather than at the
/// microsecond resolution the clock reports.
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

    async fn create_domain(
        &self,
        input: &CreateDomainInput,
    ) -> Result<CreateDomainOutput, CreateDomainError> {
        CreateDomain::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_domain(&self, domain_id: &str) -> Result<GetDomainOutput, GetDomainError> {
        GetDomain::new(Arc::clone(&self.engine))
            .handle(
                GetDomainInput {
                    domain_id: domain_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn delete_domain(&self, domain_id: &str) -> Result<DeleteDomainOutput, DeleteDomainError> {
        DeleteDomain::new(Arc::clone(&self.engine))
            .handle(
                DeleteDomainInput {
                    domain_id: domain_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn create_space(
        &self,
        input: &CreateSpaceInput,
    ) -> Result<CreateSpaceOutput, CreateSpaceError> {
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_space(&self, space_id: &str) -> Result<GetSpaceOutput, GetSpaceError> {
        GetSpace::new(Arc::clone(&self.engine))
            .handle(
                GetSpaceInput {
                    space_id: space_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn delete_space(&self, space_id: &str) -> Result<DeleteSpaceOutput, DeleteSpaceError> {
        DeleteSpace::new(Arc::clone(&self.engine))
            .handle(
                DeleteSpaceInput {
                    space_id: space_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn list_spaces(
        &self,
        domain_id: Option<&str>,
        max_results: Option<i32>,
        next_token: Option<String>,
    ) -> Result<ListSpacesOutput, ListSpacesError> {
        ListSpaces::new(Arc::clone(&self.engine))
            .handle(
                ListSpacesInput {
                    domain_id: domain_id.map(str::to_string),
                    next_token,
                    max_results,
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

/// One row of the `CreateSpace` table: the request members that vary, and the
/// `encryptionConfiguration` the created space must report back.
struct SpaceCase {
    name: String,
    requested_encryption: Option<EncryptionConfiguration>,
    agent_role: Option<&'static str>,
    expected_strategy: &'static str,
    expected_key: Option<&'static str>,
}

fn create_domain_input(name: &str, providers: &[&str]) -> CreateDomainInput {
    CreateDomainInput {
        name: name.to_string(),
        identity_providers: providers.iter().map(|p| (*p).to_string()).collect(),
        ..Default::default()
    }
}

fn create_space_input(name: &str, domain_id: &str) -> CreateSpaceInput {
    CreateSpaceInput {
        name: name.to_string(),
        domain_id: domain_id.to_string(),
        data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
        ..Default::default()
    }
}

async fn domain(omni: &Omni, case: &str, name: &str, providers: &[&str]) -> Domain {
    omni.create_domain(&create_domain_input(name, providers))
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
        .domain
}

async fn space(omni: &Omni, case: &str, name: &str, domain_id: &str) -> Space {
    omni.create_space(&create_space_input(name, domain_id))
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
        .space
}

/// Walk every page of `ListSpaces` and return the space ids in the order they came
/// back, asserting each page respects the requested bound.
async fn paged_space_ids(
    omni: &Omni,
    case: &str,
    domain_id: Option<&str>,
    max_results: Option<i32>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let page = omni
            .list_spaces(domain_id, max_results, token)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListSpaces failed: {error:?}"));
        if let Some(bound) = max_results {
            assert!(
                page.items.len() <= bound as usize,
                "case {case:?}: a page of {} spaces exceeds maxResults={bound}",
                page.items.len()
            );
        }
        ids.extend(page.items.iter().map(|item| item.space_id.clone()));
        match page.next_token {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            ids.len() < 500,
            "case {case:?}: pagination did not terminate after {} ids",
            ids.len()
        );
    }
    ids
}

#[tokio::test]
async fn create_domain_persists_a_record_get_domain_returns_it_verbatim() {
    let omni = Omni::new();
    // Rows differ along the identity-provider set (the model's `IdentityProvider`
    // value set is exactly IAM | IDC, `@length(1..2)` `@uniqueItems`) and along the
    // name's size, including both published bounds of `name`'s `@length(3..63)`.
    let longest_name = "a".repeat(63);
    let cases: Vec<(String, Vec<&str>, bool)> = vec![
        ("abc".to_string(), vec!["IAM"], false),
        ("omni-one".to_string(), vec!["IDC"], false),
        (
            "omni-two-with-several-hyphens".to_string(),
            vec!["IAM", "IDC"],
            true,
        ),
        (longest_name, vec!["IDC", "IAM"], false),
    ];

    for (name, providers, with_idc_instance) in cases {
        let case = format!("name={name} providers={providers:?}");
        let mut input = create_domain_input(&name, &providers);
        if with_idc_instance {
            input.identity_provider_configuration = Some(IdentityProviderConfiguration {
                identity_center_configuration: Some(IdentityCenterConfiguration {
                    identity_center_instance_arn: Some(
                        "arn:aws:sso:::instance/ssoins-1234567890abcdef".to_string(),
                    ),
                }),
            });
        }
        let created = omni
            .create_domain(&input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"));
        let domain = created.domain;

        assert!(
            is_domain_id(&domain.domain_id),
            "case {case:?}: domainId {:?} does not satisfy {DOMAIN_ID_PATTERN}",
            domain.domain_id
        );
        assert!(
            is_arn(&domain.domain_arn),
            "case {case:?}: domainArn {:?} does not satisfy {ARN_PATTERN}",
            domain.domain_arn
        );
        assert_eq!(
            domain.domain_arn,
            format!(
                "arn:aws:cloudwatch:{REGION}:{}:domain/{}",
                engine::EMULATOR_ACCOUNT_ID, domain.domain_id
            ),
            "case {case:?}: domainArn is not built in the service's ARN namespace"
        );
        assert_eq!(
            domain.name.as_deref(),
            Some(name.as_str()),
            "case {case:?}: name was not persisted"
        );
        assert_eq!(
            domain.identity_providers,
            providers
                .iter()
                .map(|p| (*p).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: identityProviders were not persisted"
        );
        assert_eq!(
            domain.region, REGION,
            "case {case:?}: region is not the signed region"
        );
        // `DomainStatus` declares exactly one value.
        assert_eq!(
            domain.status, "ACTIVE",
            "case {case:?}: unexpected status {:?}",
            domain.status
        );
        assert_eq!(
            domain.domain_endpoint_url,
            format!("https://{}.cloudwatch-omni.global.app.aws", domain.domain_id),
            "case {case:?}: domainEndpointUrl {:?} is not the domain's global endpoint",
            domain.domain_endpoint_url
        );
        assert!(
            !domain.domain_endpoint_url.contains(REGION),
            "case {case:?}: domainEndpointUrl {:?} is global and must carry no region",
            domain.domain_endpoint_url
        );
        let custom = domain.custom_endpoint_urls.clone().unwrap_or_default();
        assert_eq!(
            custom,
            vec![format!("https://{name}.cloudwatch-omni.global.app.aws")],
            "case {case:?}: customEndpointUrls {custom:?} are not the name's global endpoint"
        );
        // `identityCenterApplicationArn` is "Absent for IAM-only domains".
        assert_eq!(
            domain.identity_center_application_arn.is_some(),
            providers.contains(&"IDC"),
            "case {case:?}: identityCenterApplicationArn presence {:?} does not follow the IDC provider",
            domain.identity_center_application_arn
        );

        let fetched = omni
            .get_domain(&domain.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{fetched:?}"),
            format!("{domain:?}"),
            "case {case:?}: GetDomain did not return the record CreateDomain persisted"
        );
        let refetched = omni
            .get_domain(&domain.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{:?}", refetched.created_at),
            format!("{:?}", domain.created_at),
            "case {case:?}: createdAt moved between reads"
        );
    }
}

#[tokio::test]
async fn create_space_derives_every_member_from_its_parent_and_round_trips() {
    let omni = Omni::new();
    let parent = domain(&omni, "parent", "omni-parent", &["IAM"]).await;

    // Rows differ along the encryption configuration (absent, explicit AWS_OWNED,
    // CUSTOMER_MANAGED with a key — the `EncryptionStrategy` value set in full) and
    // along whether the optional AgentCore role is supplied; `name` spans both
    // published bounds of its `@length(3..64)`.
    let longest_name = "b".repeat(64);
    let cases: Vec<SpaceCase> = vec![
        SpaceCase {
            name: "spc".to_string(),
            requested_encryption: None,
            agent_role: None,
            expected_strategy: "AWS_OWNED",
            expected_key: None,
        },
        SpaceCase {
            name: "space-aws-owned".to_string(),
            requested_encryption: Some(EncryptionConfiguration {
                encryption_strategy: "AWS_OWNED".to_string(),
                kms_key_arn: None,
            }),
            agent_role: None,
            expected_strategy: "AWS_OWNED",
            expected_key: None,
        },
        SpaceCase {
            name: "space-customer-managed".to_string(),
            requested_encryption: Some(EncryptionConfiguration {
                encryption_strategy: "CUSTOMER_MANAGED".to_string(),
                kms_key_arn: Some(CUSTOMER_KEY_ARN.to_string()),
            }),
            agent_role: Some("arn:aws:iam::000000000000:role/AgentCoreEval"),
            expected_strategy: "CUSTOMER_MANAGED",
            expected_key: Some(CUSTOMER_KEY_ARN),
        },
        SpaceCase {
            name: longest_name,
            requested_encryption: None,
            agent_role: None,
            expected_strategy: "AWS_OWNED",
            expected_key: None,
        },
    ];

    for SpaceCase {
        name,
        requested_encryption,
        agent_role,
        expected_strategy,
        expected_key,
    } in cases
    {
        let case = format!("name={name} strategy={expected_strategy}");
        let mut input = create_space_input(&name, &parent.domain_id);
        input.encryption_configuration = requested_encryption;
        input.agent_core_evaluation_role_arn = agent_role.map(str::to_string);
        let space = omni
            .create_space(&input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space;

        assert!(
            is_space_id(&space.space_id),
            "case {case:?}: spaceId {:?} does not satisfy {SPACE_ID_PATTERN}",
            space.space_id
        );
        assert_eq!(
            space.space_arn,
            format!(
                "arn:aws:cloudwatch:{REGION}:{}:space/{}",
                engine::EMULATOR_ACCOUNT_ID, space.space_id
            ),
            "case {case:?}: spaceArn is not built in the service's ARN namespace"
        );
        assert_eq!(
            space.domain_arn.as_deref(),
            Some(parent.domain_arn.as_str()),
            "case {case:?}: domainArn was not copied from the parent domain"
        );
        assert_eq!(
            space.region, REGION,
            "case {case:?}: region is not the signed region"
        );
        assert!(
            is_account_id(&space.owner_account_id),
            "case {case:?}: ownerAccountId {:?} does not satisfy {ACCOUNT_ID_PATTERN}",
            space.owner_account_id
        );
        assert_eq!(
            space.data_access_role_arn, DATA_ACCESS_ROLE_ARN,
            "case {case:?}: dataAccessRoleArn was not persisted"
        );
        assert_eq!(
            space.agent_core_evaluation_role_arn.as_deref(),
            agent_role,
            "case {case:?}: agentCoreEvaluationRoleArn was not persisted"
        );
        assert_eq!(
            space.status, "ACTIVE",
            "case {case:?}: unexpected status {:?}",
            space.status
        );
        // `Space.encryptionConfiguration` is "Always populated".
        let reported = space
            .encryption_configuration
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: encryptionConfiguration is absent"));
        assert_eq!(
            reported.encryption_strategy, expected_strategy,
            "case {case:?}: unexpected encryptionStrategy {:?}",
            reported.encryption_strategy
        );
        assert_eq!(
            reported.kms_key_arn.as_deref(),
            expected_key,
            "case {case:?}: unexpected kmsKeyArn {:?}",
            reported.kms_key_arn
        );

        let fetched = omni
            .get_space(&space.space_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetSpace failed: {error:?}"))
            .space;
        assert_eq!(
            format!("{fetched:?}"),
            format!("{space:?}"),
            "case {case:?}: GetSpace did not return the record CreateSpace persisted"
        );
    }
}

#[tokio::test]
async fn an_inconsistent_encryption_configuration_is_refused() {
    let omni = Omni::new();
    let parent = domain(&omni, "parent", "omni-encryption", &["IAM"]).await;

    // `EncryptionConfiguration`'s model doc: `kmsKeyArn` is required for
    // CUSTOMER_MANAGED and must be absent for AWS_OWNED; "a mismatch is a
    // ValidationException rather than a silently ignored field".
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("customer-managed-without-key", "CUSTOMER_MANAGED", None),
        ("aws-owned-with-key", "AWS_OWNED", Some(CUSTOMER_KEY_ARN)),
        ("unmodelled-strategy", "SELF_MANAGED", None),
    ];

    for (name, strategy, key) in cases {
        let case = format!("{name}: strategy={strategy} key={key:?}");
        let mut input = create_space_input(name, &parent.domain_id);
        input.encryption_configuration = Some(EncryptionConfiguration {
            encryption_strategy: strategy.to_string(),
            kms_key_arn: key.map(str::to_string),
        });
        expect_modelled_error(
            &case,
            omni.create_space(&input).await,
            400,
            "ValidationException",
        );
    }

    // The refusals persisted nothing.
    let spaces = paged_space_ids(&omni, "after refusals", Some(&parent.domain_id), None).await;
    assert!(
        spaces.is_empty(),
        "a refused CreateSpace must store nothing, found {spaces:?}"
    );
}

#[tokio::test]
async fn a_resource_the_store_does_not_hold_is_not_found() {
    let omni = Omni::new();

    expect_modelled_error(
        "unknown domain",
        omni.get_domain(MISSING_DOMAIN_ID).await,
        404,
        "ResourceNotFoundException",
    );
    expect_modelled_error(
        "unknown space",
        omni.get_space(MISSING_SPACE_ID).await,
        404,
        "ResourceNotFoundException",
    );
    expect_modelled_error(
        "space under an unknown domain",
        omni.create_space(&create_space_input("orphan-space", MISSING_DOMAIN_ID))
            .await,
        404,
        "ResourceNotFoundException",
    );
    expect_modelled_error(
        "spaces filtered by an unknown domain",
        omni.list_spaces(Some(MISSING_DOMAIN_ID), None, None).await,
        404,
        "ResourceNotFoundException",
    );
    // The unfiltered listing names no resource, so it is an empty page rather than a miss.
    let unfiltered = omni
        .list_spaces(None, None, None)
        .await
        .expect("an unfiltered listing of an empty store succeeds");
    assert!(
        unfiltered.items.is_empty(),
        "case \"unfiltered empty store\": expected no spaces, got {:?}",
        unfiltered.items
    );

    // A domain created after the failed lookups is still reachable: a miss is not a
    // write.
    let created = domain(&omni, "after misses", "omni-after-misses", &["IAM"]).await;
    omni.get_domain(&created.domain_id)
        .await
        .expect("the domain created after the misses is readable");
}

#[tokio::test]
async fn a_duplicate_domain_name_conflicts() {
    let omni = Omni::new();
    for name in ["abc", "omni-duplicate-name"] {
        let case = format!("name={name}");
        let first = domain(&omni, &case, name, &["IAM"]).await;
        expect_modelled_error(
            &case,
            omni.create_domain(&create_domain_input(name, &["IDC"])).await,
            409,
            "ConflictException",
        );
        // The conflict left the original untouched.
        let still_there = omni
            .get_domain(&first.domain_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetDomain failed: {error:?}"))
            .domain;
        assert_eq!(
            still_there.identity_providers,
            vec!["IAM".to_string()],
            "case {case:?}: the conflicting create overwrote the original domain"
        );
    }
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_original_resource() {
    let omni = Omni::new();
    // Rows span `ClientToken`'s `@length(1..64)` and its `^[!-~]+$` character class.
    let tokens: Vec<String> = vec![
        "t".to_string(),
        "token-one".to_string(),
        "~!#$%&*+".to_string(),
        "z".repeat(64),
    ];

    let parent = domain(&omni, "parent", "omni-idempotent", &["IAM"]).await;

    for (index, token) in tokens.iter().enumerate() {
        let case = format!("token={token}");

        let mut domain_input = create_domain_input(&format!("idem-domain-{index}"), &["IAM"]);
        domain_input.client_token = Some(token.clone());
        let first = omni
            .create_domain(&domain_input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain;
        let replay = omni
            .create_domain(&domain_input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the replay failed: {error:?}"))
            .domain;
        assert_eq!(
            format!("{replay:?}"),
            format!("{first:?}"),
            "case {case:?}: a CreateDomain replay did not return the original domain"
        );

        let mut space_input = create_space_input(&format!("idem-space-{index}"), &parent.domain_id);
        space_input.client_token = Some(token.clone());
        let first = omni
            .create_space(&space_input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space;
        let replay = omni
            .create_space(&space_input)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the replay failed: {error:?}"))
            .space;
        assert_eq!(
            format!("{replay:?}"),
            format!("{first:?}"),
            "case {case:?}: a CreateSpace replay did not return the original space"
        );
    }

    // One space per token, not two: the replays minted nothing.
    let ids = paged_space_ids(&omni, "after replays", Some(&parent.domain_id), None).await;
    assert_eq!(
        ids.len(),
        tokens.len(),
        "replays minted extra spaces: {ids:?}"
    );
}

#[tokio::test]
async fn list_spaces_pages_a_stable_order_under_every_bound() {
    let omni = Omni::new();

    let empty = omni
        .list_spaces(None, None, None)
        .await
        .expect("an empty store lists successfully");
    assert!(
        empty.items.is_empty() && empty.next_token.is_none(),
        "an empty store must list no spaces and hand back no token, got {} items / {:?}",
        empty.items.len(),
        empty.next_token
    );

    let first = domain(&omni, "first", "omni-first", &["IAM"]).await;
    let second = domain(&omni, "second", "omni-second", &["IDC"]).await;

    let mut in_first = Vec::new();
    for index in 0..5 {
        let created = space(
            &omni,
            "seed first",
            &format!("first-space-{index}"),
            &first.domain_id,
        )
        .await;
        in_first.push(created.space_id);
    }
    let mut in_second = Vec::new();
    for index in 0..2 {
        let created = space(
            &omni,
            "seed second",
            &format!("second-space-{index}"),
            &second.domain_id,
        )
        .await;
        in_second.push(created.space_id);
    }
    let everything: Vec<String> = in_first.iter().chain(in_second.iter()).cloned().collect();

    // Rows differ along the page bound — both published bounds of `maxResults`'s
    // `@range(1..100)`, sizes either side of the collection, and the documented
    // default (absent) — and along the `domainId` filter.
    let bounds: Vec<Option<i32>> = vec![None, Some(1), Some(2), Some(3), Some(7), Some(100)];
    let filters: Vec<(Option<&str>, &Vec<String>)> = vec![
        (None, &everything),
        (Some(first.domain_id.as_str()), &in_first),
        (Some(second.domain_id.as_str()), &in_second),
    ];

    for bound in &bounds {
        for (filter, expected) in &filters {
            let case = format!("maxResults={bound:?} domainId={filter:?}");
            let seen = paged_space_ids(&omni, &case, *filter, *bound).await;
            assert_eq!(
                &seen, *expected,
                "case {case:?}: paging did not traverse the collection in creation order, got {seen:?}"
            );
        }
    }

    // A bounded first page hands back a token; the token resumes right after it.
    let page = omni
        .list_spaces(Some(&first.domain_id), Some(2), None)
        .await
        .expect("the first page lists");
    assert_eq!(
        page.items.len(),
        2,
        "a bound of 2 over 5 spaces must fill the page, got {}",
        page.items.len()
    );
    let token = page
        .next_token
        .clone()
        .expect("spaces remain, so a token must be returned");
    let next = omni
        .list_spaces(Some(&first.domain_id), Some(2), Some(token))
        .await
        .expect("the second page lists");
    assert_eq!(
        next.items
            .iter()
            .map(|item| item.space_id.clone())
            .collect::<Vec<String>>(),
        in_first[2..4].to_vec(),
        "the token did not resume exactly where the previous page stopped"
    );

    // The summary a list returns reports the same values as the full space.
    let summary = &page.items[0];
    let full = omni
        .get_space(&summary.space_id)
        .await
        .expect("the listed space is readable")
        .space;
    assert_eq!(
        (
            summary.name.as_str(),
            summary.space_arn.as_str(),
            summary.domain_arn.as_deref(),
            summary.region.as_str(),
            summary.owner_account_id.as_str(),
            summary.status.as_str(),
            format!("{:?}", summary.created_at),
            format!("{:?}", summary.updated_at),
        ),
        (
            full.name.as_str(),
            full.space_arn.as_str(),
            full.domain_arn.as_deref(),
            full.region.as_str(),
            full.owner_account_id.as_str(),
            full.status.as_str(),
            format!("{:?}", full.created_at),
            format!("{:?}", full.updated_at),
        ),
        "a SpaceSummary must project the stored space, not a second copy of it"
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused() {
    let omni = Omni::new();
    // Rows: not hex at all, valid hex that decodes to another collection's cursor,
    // and hex that decodes to this collection's prefix with an unparseable cursor.
    for token in ["not-a-token", "646f6d61696e733a31", "7370616365733a78"] {
        expect_modelled_error(
            token,
            omni.list_spaces(None, None, Some(token.to_string())).await,
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn deleting_a_space_removes_it_and_a_domain_refuses_while_it_holds_spaces() {
    let omni = Omni::new();
    let parent = domain(&omni, "lifecycle", "omni-lifecycle", &["IAM"]).await;
    let created = space(&omni, "lifecycle", "lifecycle-space", &parent.domain_id).await;

    // "A domain cannot be deleted while it contains spaces."
    expect_modelled_error(
        "domain still holding a space",
        omni.delete_domain(&parent.domain_id).await,
        409,
        "ConflictException",
    );
    omni.get_domain(&parent.domain_id)
        .await
        .expect("the refused delete left the domain in place");

    // DeleteSpace is @idempotent and declares no ResourceNotFoundException, so the
    // second call succeeds exactly as the first.
    for attempt in 0..2 {
        let case = format!("DeleteSpace attempt {attempt}");
        omni.delete_space(&created.space_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: DeleteSpace failed: {error:?}"));
        expect_modelled_error(
            &case,
            omni.get_space(&created.space_id).await,
            404,
            "ResourceNotFoundException",
        );
        let remaining = paged_space_ids(&omni, &case, Some(&parent.domain_id), None).await;
        assert!(
            remaining.is_empty(),
            "case {case:?}: the deleted space is still listed: {remaining:?}"
        );
    }

    // With the domain empty the delete goes through, and the name is free again.
    omni.delete_domain(&parent.domain_id)
        .await
        .expect("an empty domain deletes");
    expect_modelled_error(
        "deleted domain",
        omni.get_domain(&parent.domain_id).await,
        404,
        "ResourceNotFoundException",
    );
    let reused = domain(&omni, "name reuse", "omni-lifecycle", &["IAM"]).await;
    assert_ne!(
        reused.domain_id, parent.domain_id,
        "reusing a freed name must mint a new domain, not resurrect the old one"
    );

    // DeleteDomain is @idempotent too.
    omni.delete_domain(MISSING_DOMAIN_ID)
        .await
        .expect("deleting a domain that is already gone succeeds");
}

#[tokio::test]
async fn the_modelled_input_constraints_refuse_what_they_declare() {
    // `aws_smithy_schema_server::constraints::validate` is the exact check the
    // framework runs on a deserialized input before any handler sees it (see
    // `run_operation_http`), and a non-empty result becomes a 400
    // `ValidationException`. Each row below is refused there, so the handler never
    // has to re-check a modelled constraint.
    let too_long_space_name = "c".repeat(65);
    let too_long_domain_name = "a".repeat(64);

    let refused_space_names: Vec<(&str, String)> = vec![
        ("shorter than @length(min: 3)", "ab".to_string()),
        ("longer than @length(max: 64)", too_long_space_name),
        ("outside the name pattern (upper case)", "Upper".to_string()),
        (
            "outside the name pattern (consecutive hyphens)",
            "double--hyphen".to_string(),
        ),
        (
            "outside the name pattern (leading hyphen)",
            "-leading".to_string(),
        ),
    ];
    for (why, name) in refused_space_names {
        let input = create_space_input(&name, "d-parent");
        assert!(
            aws_smithy_schema_server::validate_to_message(&input, CREATE_SPACE_INPUT_SCHEMA)
                .is_some(),
            "case {why:?}: CreateSpace name {name:?} was accepted"
        );
    }
    // Both published bounds of `name`'s @length(3..64) are accepted.
    for name in ["abc".to_string(), "b".repeat(64)] {
        let input = create_space_input(&name, "d-parent");
        assert_eq!(
            aws_smithy_schema_server::validate_to_message(&input, CREATE_SPACE_INPUT_SCHEMA),
            None,
            "case {name:?}: a name on the published bound was refused"
        );
    }

    let refused_provider_sets: Vec<(&str, Vec<&str>)> = vec![
        ("empty, under @length(min: 1)", vec![]),
        ("over @length(max: 2)", vec!["IAM", "IDC", "IAM"]),
        ("repeated, against @uniqueItems", vec!["IAM", "IAM"]),
        ("outside the IdentityProvider value set", vec!["SAML"]),
    ];
    for (why, providers) in refused_provider_sets {
        let input = create_domain_input("provider-case", &providers);
        assert!(
            aws_smithy_schema_server::validate_to_message(&input, CREATE_DOMAIN_INPUT_SCHEMA)
                .is_some(),
            "case {why:?}: identityProviders {providers:?} was accepted"
        );
    }
    for providers in [vec!["IAM"], vec!["IDC"], vec!["IAM", "IDC"]] {
        let input = create_domain_input("provider-case", &providers);
        assert_eq!(
            aws_smithy_schema_server::validate_to_message(&input, CREATE_DOMAIN_INPUT_SCHEMA),
            None,
            "case {providers:?}: a modelled provider set was refused"
        );
    }
    // `name` on CreateDomain carries @length(3..63), one character tighter than
    // CreateSpace's.
    let input = create_domain_input(&too_long_domain_name, &["IAM"]);
    assert!(
        aws_smithy_schema_server::validate_to_message(&input, CREATE_DOMAIN_INPUT_SCHEMA).is_some(),
        "a 64-character domain name was accepted past @length(max: 63)"
    );

    // `maxResults` carries @range(min: 1, max: 100): both bounds pass, one step past
    // either is refused.
    for bound in [1, 100] {
        let input = ListSpacesInput {
            domain_id: None,
            next_token: None,
            max_results: Some(bound),
        };
        assert_eq!(
            aws_smithy_schema_server::validate_to_message(&input, LIST_SPACES_INPUT_SCHEMA),
            None,
            "case maxResults={bound}: a published bound was refused"
        );
    }
    for bound in [0, 101] {
        let input = ListSpacesInput {
            domain_id: None,
            next_token: None,
            max_results: Some(bound),
        };
        assert!(
            aws_smithy_schema_server::validate_to_message(&input, LIST_SPACES_INPUT_SCHEMA)
                .is_some(),
            "case maxResults={bound}: a value outside @range(1..100) was accepted"
        );
    }
}

#[tokio::test]
async fn the_domain_store_is_bounded_and_refuses_past_its_quota() {
    // The store cannot grow without end: `CreateDomain` refuses with the modelled
    // `ServiceQuotaExceededException` once `DOMAIN_QUOTA` records are held, and
    // `DeleteDomain` frees a slot again.
    let omni = Omni::new();
    let mut last = String::new();
    for index in 0..DOMAIN_QUOTA {
        last = omni
            .create_domain(&create_domain_input(
                &format!("quota-domain-{index}"),
                &["IAM"],
            ))
            .await
            .unwrap_or_else(|error| panic!("domain {index} within the quota failed: {error:?}"))
            .domain
            .domain_id;
    }
    expect_modelled_error(
        "one domain past DOMAIN_QUOTA",
        omni.create_domain(&create_domain_input("quota-domain-overflow", &["IAM"]))
            .await,
        402,
        "ServiceQuotaExceededException",
    );

    // Releasing a record releases its slot, so the bound is a bound and not a
    // one-way ratchet.
    omni.delete_domain(&last)
        .await
        .expect("deleting a domain releases its slot");
    omni.create_domain(&create_domain_input("quota-domain-overflow", &["IAM"]))
        .await
        .expect("a freed slot accepts another domain");
    expect_modelled_error(
        "full again",
        omni.create_domain(&create_domain_input("quota-domain-second-overflow", &["IAM"]))
            .await,
        402,
        "ServiceQuotaExceededException",
    );
}

#[tokio::test]
async fn the_space_store_is_bounded_and_refuses_past_its_quota() {
    // Same bound for spaces, and a delete frees a slot again.
    let omni = Omni::new();
    let parent = domain(&omni, "quota", "omni-space-quota", &["IAM"]).await;
    let input = create_space_input("quota-space", &parent.domain_id);
    let mut last = String::new();
    for index in 0..SPACE_QUOTA {
        last = omni
            .create_space(&input)
            .await
            .unwrap_or_else(|error| panic!("space {index} within the quota failed: {error:?}"))
            .space
            .space_id;
    }
    expect_modelled_error(
        "one space past SPACE_QUOTA",
        omni.create_space(&input).await,
        402,
        "ServiceQuotaExceededException",
    );

    omni.delete_space(&last)
        .await
        .expect("deleting a space releases its slot");
    omni.create_space(&input)
        .await
        .expect("a freed slot accepts another space");
    expect_modelled_error(
        "full again",
        omni.create_space(&input).await,
        402,
        "ServiceQuotaExceededException",
    );
}

#[tokio::test]
async fn the_space_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    // The parent domain is created through the handler holding the same engine the
    // router's service holds, which is what makes the wire calls below see it.
    let parent = domain(&omni, "wire", "omni-wire", &["IAM"]).await;

    let codec = CborCodec::default();
    let decode = |body: &bytes::Bytes| -> CreateSpaceOutput {
        let mut deserializer = codec.create_deserializer(body);
        CreateSpaceOutput::deserialize(&mut deserializer).expect("the output deserializes")
    };

    let (status, body) = omni
        .over_the_wire(
            "CreateSpace",
            CREATE_SPACE_INPUT_SCHEMA,
            &create_space_input("wire-space", &parent.domain_id),
        )
        .await;
    assert_eq!(
        status,
        200,
        "CreateSpace over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let created = decode(&body).space;
    assert!(
        is_space_id(&created.space_id),
        "spaceId {:?} does not satisfy {SPACE_ID_PATTERN}",
        created.space_id
    );

    // The space the wire call created is readable through the handler's store.
    let fetched = omni
        .get_space(&created.space_id)
        .await
        .expect("the space created over the wire is in the store")
        .space;
    assert_eq!(
        (
            fetched.space_id.as_str(),
            fetched.name.as_str(),
            fetched.space_arn.as_str(),
            fetched.domain_arn.as_deref(),
            fetched.region.as_str(),
            fetched.owner_account_id.as_str(),
            fetched.data_access_role_arn.as_str(),
            fetched.status.as_str(),
            format!("{:?}", fetched.encryption_configuration),
        ),
        (
            created.space_id.as_str(),
            created.name.as_str(),
            created.space_arn.as_str(),
            created.domain_arn.as_deref(),
            created.region.as_str(),
            created.owner_account_id.as_str(),
            created.data_access_role_arn.as_str(),
            created.status.as_str(),
            format!("{:?}", created.encryption_configuration),
        ),
        "the wire response and the stored record disagree"
    );
    assert_eq!(
        (millis(fetched.created_at), millis(fetched.updated_at)),
        (millis(created.created_at), millis(created.updated_at)),
        "the wire response and the stored record disagree on the timestamps"
    );

    // And a miss over the wire is the modelled 404 with its modelled error code.
    let (status, body) = omni
        .over_the_wire(
            "GetSpace",
            GET_SPACE_INPUT_SCHEMA,
            &GetSpaceInput {
                space_id: MISSING_SPACE_ID.to_string(),
            },
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 404,
        "GetSpace over the wire answered {status} for a missing space: {rendered:?}"
    );
    assert!(
        rendered.contains("ResourceNotFoundException"),
        "the wire error body does not name ResourceNotFoundException: {rendered:?}"
    );

    // A modelled constraint is refused before the handler runs, as a 400.
    let (status, body) = omni
        .over_the_wire(
            "CreateSpace",
            CREATE_SPACE_INPUT_SCHEMA,
            &create_space_input("ab", &parent.domain_id),
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 400,
        "a name under @length(min: 3) answered {status}: {rendered:?}"
    );
    assert!(
        rendered.contains("ValidationException"),
        "the wire refusal does not name ValidationException: {rendered:?}"
    );

    // ListSpaces over the wire reports the one space the store holds.
    let (status, body) = omni
        .over_the_wire(
            "ListSpaces",
            LIST_SPACES_INPUT_SCHEMA,
            &ListSpacesInput {
                domain_id: Some(parent.domain_id.clone()),
                next_token: None,
                max_results: Some(10),
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "ListSpaces over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let mut deserializer = codec.create_deserializer(&body);
    let listed = ListSpacesOutput::deserialize(&mut deserializer).expect("the output deserializes");
    assert_eq!(
        listed
            .items
            .iter()
            .map(|item| item.space_id.clone())
            .collect::<Vec<String>>(),
        vec![created.space_id.clone()],
        "ListSpaces over the wire did not report the stored space"
    );
    assert!(
        listed.next_token.is_none(),
        "one space under a bound of 10 needs no token, got {:?}",
        listed.next_token
    );
}
