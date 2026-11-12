// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's access grants.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, so a create is observable by the reader that runs next.
//! Failures are the operations' own modelled errors, so each assertion checks the wire
//! facts that are requirements: the HTTP status and the error code.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * `CreateAccessGrant` mints a grant in an EXISTING space of an EXISTING domain,
//!   persists it and returns the persisted record, and `GetAccessGrant` answers from
//!   that same store;
//! * every derived member (`grantId`, `grantArn`, `accountId`, `grantType`, `createdBy`,
//!   `createdAt`/`updatedAt`) is computed from the store and the request rather than
//!   echoed from the body;
//! * a grant belongs to one space of one domain, so neither an unknown domain nor a
//!   space of another domain resolves;
//! * a name already identifying a grant of that space cannot identify a second one;
//! * `@idempotencyToken` replays return the original grant;
//! * the member combinations the model documents as invalid are refused with
//!   `ValidationException`;
//! * a grant whose scoped actions apply to an access profile is recorded against that
//!   profile, which is what `AccessProfile.assumeStatus` reports.
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

use omni::ops::create_access_grant::{CreateAccessGrant, CreateAccessGrantError};
use omni::ops::create_access_profile::CreateAccessProfile;
use omni::ops::create_domain::CreateDomain;
use omni::ops::create_space::CreateSpace;
use omni::ops::get_access_grant::{GetAccessGrant, GetAccessGrantError};
use omni::ops::get_access_profile::GetAccessProfile;
use omni::ops::types::{
    AccessGrant, AccessGrantPrincipal, AccessGrantPrincipalAttribute, CreateAccessGrantInput,
    CreateAccessGrantOutput, CreateAccessProfileInput, CreateDomainInput, CreateSpaceInput,
    GetAccessGrantInput, GetAccessGrantOutput, GetAccessProfileInput, ResourceScope, ScopedActions,
    CREATE_ACCESS_GRANT_INPUT_SCHEMA, GET_ACCESS_GRANT_INPUT_SCHEMA,
};
use omni::state::{
    ACCESS_GRANT_TYPE_CUSTOMER_MANAGED, ACCESS_PROFILE_RESOURCE_TYPE, ASSUME_STATUS_ALLOWED,
    ASSUME_STATUS_DENIED,
};

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN below is expected to carry it.
const REGION: &str = "us-west-2";

/// The access key id the suite's `Authorization` header carries. `createdBy` is derived
/// from it, so a grant reports who called rather than a fabricated identity.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// `GrantId`'s modelled `@pattern` — the model doc reads "a v4 UUID assigned when the
/// grant is created".
const GRANT_ID_PATTERN: &str =
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

/// A well-formed `DomainId` (its modelled `^d-[0-9a-z]{1,25}$`) that names nothing.
const MISSING_DOMAIN_ID: &str = "d-0000000000";

/// A well-formed `SpaceId` (its modelled pattern) that names nothing.
const MISSING_SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A well-formed `GrantId` that names nothing.
const MISSING_GRANT_ID: &str = "11111111-2222-4333-8444-555555555555";

/// Whether `value` satisfies [`GRANT_ID_PATTERN`].
///
/// Checked by hand rather than through a regex engine, so this crate declares no
/// dependency beyond the ones it already has. The predicate is exercised on values that
/// must FAIL it as well as pass ([`the_grant_id_pattern_is_checked_as_written`]).
fn is_grant_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
}

/// Whether `value` satisfies `Arn`'s modelled `^arn:.+$`.
fn is_arn(value: &str) -> bool {
    value
        .strip_prefix("arn:")
        .is_some_and(|rest| !rest.is_empty())
}

/// Whether `value` satisfies `AccountId`'s modelled `^[0-9]{12}$`.
fn is_account_id(value: &str) -> bool {
    value.len() == 12 && value.chars().all(|c| c.is_ascii_digit())
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
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/cloudwatch/aws4_request, \
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

    /// A domain, through the handler that owns it.
    async fn domain(&self, case: &str, name: &str) -> String {
        CreateDomain::new(Arc::clone(&self.engine))
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
            .domain
            .domain_id
    }

    /// A space of `domain_id`, through the handler that owns it.
    async fn space(&self, case: &str, domain_id: &str, name: &str) -> String {
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id: domain_id.to_string(),
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

    /// A domain with one space in it, which is what a grant needs.
    async fn domain_and_space(&self, case: &str, name: &str) -> (String, String) {
        let domain_id = self.domain(case, name).await;
        let space_id = self.space(case, &domain_id, name).await;
        (domain_id, space_id)
    }

    /// An access profile of `space_id`, and its ARN.
    async fn access_profile(&self, case: &str, space_id: &str, name: &str) -> (String, String) {
        let profile = CreateAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessProfileInput {
                    space_id: space_id.to_string(),
                    name: name.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessProfile failed: {error:?}"))
            .access_profile;
        (profile.profile_id, profile.arn)
    }

    /// The `assumeStatus` the reader reports for a profile.
    async fn assume_status(&self, case: &str, space_id: &str, profile_id: &str) -> String {
        GetAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                GetAccessProfileInput {
                    space_id: space_id.to_string(),
                    profile_id: profile_id.to_string(),
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetAccessProfile failed: {error:?}"))
            .access_profile
            .assume_status
            .unwrap_or_else(|| panic!("case {case:?}: the profile reported no assumeStatus"))
    }

    async fn create_access_grant(
        &self,
        input: &CreateAccessGrantInput,
    ) -> Result<CreateAccessGrantOutput, CreateAccessGrantError> {
        CreateAccessGrant::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_access_grant(
        &self,
        grant_id: &str,
    ) -> Result<GetAccessGrantOutput, GetAccessGrantError> {
        GetAccessGrant::new(Arc::clone(&self.engine))
            .handle(
                GetAccessGrantInput {
                    grant_id: grant_id.to_string(),
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

/// A principal naming an id, which is what every principal type accepts.
fn principal(principal_type: &str, principal_id: &str) -> AccessGrantPrincipal {
    AccessGrantPrincipal {
        principal_type: principal_type.to_string(),
        principal_id: Some(principal_id.to_string()),
        principal_attributes: None,
    }
}

/// One group of scoped actions, limited to the resources `resources` name.
fn scoped_actions(actions: &[&str], resources: Option<Vec<ResourceScope>>) -> ScopedActions {
    ScopedActions {
        actions: actions.iter().map(|action| action.to_string()).collect(),
        resources,
        context_conditions: None,
    }
}

/// A resource scope naming specific ARNs of one resource type.
fn resource_scope(resource_type: &str, resource_arns: Option<Vec<String>>) -> ResourceScope {
    ResourceScope {
        resource_type: resource_type.to_string(),
        resource_arns,
        tags: None,
        signal_types: None,
        row_scope_groups: None,
    }
}

/// The smallest grant request the model admits, for the parents `domain_id`/`space_id`.
fn create_input(domain_id: &str, space_id: &str, name: &str) -> CreateAccessGrantInput {
    CreateAccessGrantInput {
        domain_id: domain_id.to_string(),
        space_id: space_id.to_string(),
        name: name.to_string(),
        principal: principal("IAM_ROLE", "arn:aws:iam::000000000000:role/Reader"),
        permission: "READ".to_string(),
        scoped_actions: None,
        tags: None,
        client_token: None,
    }
}

/// Every member of a grant but its timestamps, as a comparable tuple. The structures
/// (`principal`, `scopedActions`) are compared through their rendering, which is what
/// keeps them comparable without the model's shapes deriving equality.
type GrantFields = (
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

fn fields(grant: &AccessGrant) -> GrantFields {
    (
        grant.grant_id.clone(),
        grant.grant_arn.clone(),
        grant.name.clone(),
        grant.account_id.clone(),
        grant.domain_id.clone(),
        grant.space_id.clone(),
        grant.permission.clone(),
        grant.grant_type.clone(),
        grant.created_by.clone(),
        format!("{:?}", grant.principal),
        format!("{:?}", grant.scoped_actions),
    )
}

/// One row of the `CreateAccessGrant` table: the request members that vary.
struct GrantCase {
    case: &'static str,
    name: String,
    permission: &'static str,
    principal: AccessGrantPrincipal,
    scoped_actions: Option<Vec<ScopedActions>>,
    tags: Option<HashMap<String, String>>,
}

/// The rows span what the rule generalises over: every `AccessGrantPermission` the model
/// declares (`smithy_query(query="shape",shape="AccessGrantPermission")`: SPACE_ADMIN,
/// READ, READ_WRITE_DELETE, CUSTOM), principals named by id and by attributes across
/// several `AccessGrantPrincipalType` values, `name` over its whole `@length(1..64)` and
/// `^[a-zA-Z0-9_-]+$` class, `scopedActions` absent, one group and the model's
/// `@length(max: 20)`, and `tags` absent, one and many.
fn grant_cases() -> Vec<GrantCase> {
    vec![
        GrantCase {
            case: "the shortest name the model admits, READ for an IAM role",
            name: "a".to_string(),
            permission: "READ",
            principal: principal("IAM_ROLE", "arn:aws:iam::000000000000:role/Reader"),
            scoped_actions: None,
            tags: None,
        },
        GrantCase {
            case: "SPACE_ADMIN for an Identity Center user named by attributes",
            name: "space-admins".to_string(),
            permission: "SPACE_ADMIN",
            principal: AccessGrantPrincipal {
                principal_type: "IDC_USER".to_string(),
                principal_id: None,
                principal_attributes: Some(vec![AccessGrantPrincipalAttribute {
                    key: "department".to_string(),
                    value: "observability".to_string(),
                }]),
            },
            scoped_actions: None,
            tags: Some(HashMap::from([("team".to_string(), "sre".to_string())])),
        },
        GrantCase {
            case: "READ_WRITE_DELETE for an alert, narrowed to one resource type",
            name: "alert_writer".to_string(),
            permission: "READ_WRITE_DELETE",
            principal: principal("ALERT", "alert-high-latency"),
            scoped_actions: Some(vec![scoped_actions(
                &["cloudwatch:GetTelemetry"],
                Some(vec![resource_scope("DataSet", None)]),
            )]),
            tags: Some(HashMap::new()),
        },
        GrantCase {
            case: "the longest name the model admits, CUSTOM with an explicit action list",
            name: "n".repeat(64),
            permission: "CUSTOM",
            principal: principal("AGENT", "agent/telemetry-reader"),
            scoped_actions: Some(vec![scoped_actions(
                &["cloudwatch:GetTelemetry", "logs:StartQuery"],
                Some(vec![resource_scope(
                    "OmniDashboard",
                    Some(vec![
                        "arn:aws:cloudwatch:us-west-2:000000000000:omni-dashboard/one".to_string(),
                    ]),
                )]),
            )]),
            tags: None,
        },
        GrantCase {
            case: "the whole name character class, many tags, scopedActions at its bound",
            name: "Grant_9-NAME".to_string(),
            permission: "CUSTOM",
            principal: principal("IAM_USER", "arn:aws:iam::000000000000:user/analyst"),
            scoped_actions: Some(
                (0..20)
                    .map(|index| {
                        scoped_actions(
                            &["cloudwatch:GetTelemetry"],
                            Some(vec![resource_scope(&format!("DataSet{index}"), None)]),
                        )
                    })
                    .collect(),
            ),
            tags: Some(
                (0..50)
                    .map(|index| (format!("key{index}"), format!("value{index}")))
                    .collect(),
            ),
        },
    ]
}

#[tokio::test]
async fn the_grant_id_pattern_is_checked_as_written() {
    // Each row: the value, and whether the modelled pattern admits it.
    for (value, expected) in [
        ("11111111-2222-4333-8444-555555555555", true),
        ("00000000-0000-0000-0000-000000000000", true),
        ("abcdefab-cdef-abcd-efab-cdefabcdefab", true),
        ("ABCDEFAB-CDEF-ABCD-EFAB-CDEFABCDEFAB", false),
        ("11111111-2222-4333-8444-55555555555", false),
        ("11111111222243338444555555555555", false),
        ("gggggggg-2222-4333-8444-555555555555", false),
        ("", false),
    ] {
        assert_eq!(
            is_grant_id(value),
            expected,
            "case {value:?}: {GRANT_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [("000000000000", true), ("00000000000", false), ("0000000000a", false)] {
        assert_eq!(
            is_account_id(value),
            expected,
            "case {value:?}: ^[0-9]{{12}}$ should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [("arn:aws:cloudwatch:us-west-2:000000000000:access-grant/g", true), ("arn:", false)] {
        assert_eq!(
            is_arn(value),
            expected,
            "case {value:?}: ^arn:.+$ should {} it",
            if expected { "admit" } else { "reject" }
        );
    }
}

#[tokio::test]
async fn create_access_grant_persists_a_record_get_access_grant_returns_it() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("table", "grant-table").await;
    let mut minted: Vec<String> = Vec::new();

    for row in grant_cases() {
        let case = row.case;
        let created = omni
            .create_access_grant(&CreateAccessGrantInput {
                domain_id: domain_id.clone(),
                space_id: space_id.clone(),
                name: row.name.clone(),
                principal: row.principal.clone(),
                permission: row.permission.to_string(),
                scoped_actions: row.scoped_actions.clone(),
                tags: row.tags.clone(),
                client_token: None,
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessGrant failed: {error:?}"))
            .access_grant;

        // The id is minted by the service, inside what `GrantId` models.
        assert!(
            is_grant_id(&created.grant_id),
            "case {case:?}: grantId {:?} does not satisfy {GRANT_ID_PATTERN}",
            created.grant_id
        );
        assert!(
            !minted.contains(&created.grant_id),
            "case {case:?}: grantId {:?} repeats an earlier grant's id",
            created.grant_id
        );
        minted.push(created.grant_id.clone());

        // The ARN is derived from this grant alone, in the signed region.
        assert!(
            is_arn(&created.grant_arn),
            "case {case:?}: grantArn {:?} does not satisfy ^arn:.+$",
            created.grant_arn
        );
        assert!(
            created
                .grant_arn
                .ends_with(&format!(":access-grant/{}", created.grant_id)),
            "case {case:?}: grantArn {:?} does not name this grant alone",
            created.grant_arn
        );
        assert!(
            created.grant_arn.contains(&format!(":{REGION}:")),
            "case {case:?}: grantArn {:?} does not carry the signed region {REGION}",
            created.grant_arn
        );

        // The request's own members come back as sent, and the derived ones are the
        // documented values for a freshly created, customer-created grant.
        assert_eq!(
            (
                created.domain_id.as_str(),
                created.space_id.as_str(),
                created.name.as_deref(),
                created.permission.as_str(),
                created.grant_type.as_str(),
            ),
            (
                domain_id.as_str(),
                space_id.as_str(),
                Some(row.name.as_str()),
                row.permission,
                ACCESS_GRANT_TYPE_CUSTOMER_MANAGED,
            ),
            "case {case:?}: the created grant is {created:?}"
        );
        // The principal comes back as sent, with its attribute conditions reported as
        // an empty list when the request named none: `principalAttributes` is a member
        // of every principal, so it is reported empty rather than dropped.
        assert_eq!(
            format!("{:?}", created.principal),
            format!(
                "{:?}",
                AccessGrantPrincipal {
                    principal_attributes: Some(
                        row.principal.principal_attributes.clone().unwrap_or_default()
                    ),
                    ..row.principal.clone()
                }
            ),
            "case {case:?}: expected the principal as sent, got {:?}",
            created.principal
        );
        assert_eq!(
            format!("{:?}", created.scoped_actions),
            format!("{:?}", row.scoped_actions),
            "case {case:?}: expected the scoped actions as sent, got {:?}",
            created.scoped_actions
        );
        assert!(
            is_account_id(&created.account_id),
            "case {case:?}: accountId {:?} does not satisfy ^[0-9]{{12}}$",
            created.account_id
        );
        // `createdBy` reports the principal that called, which is the request's own
        // access key id rather than a constant.
        assert!(
            is_arn(&created.created_by) && created.created_by.ends_with(ACCESS_KEY_ID),
            "case {case:?}: createdBy {:?} does not name the calling principal {ACCESS_KEY_ID}",
            created.created_by
        );
        assert_eq!(
            millis(created.created_at),
            millis(created.updated_at),
            "case {case:?}: a grant nothing has updated reports createdAt {:?} and updatedAt {:?}",
            created.created_at,
            created.updated_at
        );

        // The reader answers from the store, verbatim and stably.
        for read in ["first read", "second read"] {
            let fetched = omni
                .get_access_grant(&created.grant_id)
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?} ({read}): GetAccessGrant failed: {error:?}")
                })
                .access_grant;
            assert_eq!(
                fields(&fetched),
                fields(&created),
                "case {case:?} ({read}): the stored grant is {fetched:?}, the created one {created:?}"
            );
            assert_eq!(
                (millis(fetched.created_at), millis(fetched.updated_at)),
                (millis(created.created_at), millis(created.updated_at)),
                "case {case:?} ({read}): the stored timestamps differ from the created ones"
            );
        }
    }
}

#[tokio::test]
async fn a_grant_is_created_in_a_space_of_a_domain_the_store_holds() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("home", "grant-home").await;
    let other_domain_id = omni.domain("other", "grant-other").await;
    let other_space_id = omni
        .space("other", &other_domain_id, "grant-other-space")
        .await;

    // The pair the store holds resolves.
    omni.create_access_grant(&create_input(&domain_id, &space_id, "resolves"))
        .await
        .expect("case \"its own domain and space\": the grant is created");

    // Rows: each is a parent the store does not hold under the pair the request names.
    for (case, domain, space) in [
        ("a domain the store does not hold", MISSING_DOMAIN_ID, space_id.as_str()),
        ("a space the store does not hold", domain_id.as_str(), MISSING_SPACE_ID),
        (
            "a known space under another domain",
            domain_id.as_str(),
            other_space_id.as_str(),
        ),
        (
            "a known domain with a space of another domain",
            other_domain_id.as_str(),
            space_id.as_str(),
        ),
    ] {
        expect_modelled_error(
            case,
            omni.create_access_grant(&create_input(domain, space, "orphan"))
                .await,
            404,
            "ResourceNotFoundException",
        );
    }

    // A read names the grant alone, so an id the store does not hold is the same 404.
    expect_modelled_error(
        "a read of a grant the store does not hold",
        omni.get_access_grant(MISSING_GRANT_ID).await,
        404,
        "ResourceNotFoundException",
    );
}

#[tokio::test]
async fn a_name_identifies_one_grant_per_space() {
    let omni = Omni::new();
    let (domain_id, first) = omni.domain_and_space("first", "grant-names-a").await;
    let second = omni.space("second", &domain_id, "grant-names-b").await;

    // Rows span the name class again: each is created once, refused the second time in
    // the same space, and accepted in another space.
    for name in ["a", "Grant_9-NAME", &"n".repeat(64)] {
        let created = omni
            .create_access_grant(&create_input(&domain_id, &first, name))
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: the first create failed: {error:?}"))
            .access_grant;
        expect_modelled_error(
            name,
            omni.create_access_grant(&create_input(&domain_id, &first, name))
                .await,
            409,
            "ConflictException",
        );
        let elsewhere = omni
            .create_access_grant(&create_input(&domain_id, &second, name))
            .await
            .unwrap_or_else(|error| {
                panic!("case {name:?}: the same name in another space failed: {error:?}")
            })
            .access_grant;
        assert_ne!(
            elsewhere.grant_id, created.grant_id,
            "case {name:?}: the same name in another space is a different grant, got {:?} twice",
            elsewhere.grant_id
        );
        assert_eq!(
            elsewhere.space_id, second,
            "case {name:?}: expected spaceId {second}, got {:?}",
            elsewhere.space_id
        );
    }
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_original_grant() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("tokens", "grant-tokens").await;

    // Rows span `ClientToken`'s `@length(1..64)` and its `^[!-~]+$` character class.
    for (index, token) in ["!", "a-token", "~{}[]|\\^`", &"t".repeat(64)]
        .into_iter()
        .enumerate()
    {
        let input = CreateAccessGrantInput {
            client_token: Some(token.to_string()),
            ..create_input(&domain_id, &space_id, &format!("token-grant-{index}"))
        };
        let created = omni
            .create_access_grant(&input)
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: the first create failed: {error:?}"))
            .access_grant;
        let replayed = omni
            .create_access_grant(&input)
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: the replay failed: {error:?}"))
            .access_grant;
        assert_eq!(
            fields(&replayed),
            fields(&created),
            "case {token:?}: the replay returned {replayed:?} rather than the original {created:?}"
        );

        // A DIFFERENT token for the same request is a different grant, so the replay is
        // the token's doing rather than the name's.
        let fresh = omni
            .create_access_grant(&CreateAccessGrantInput {
                client_token: Some(format!("{token}x")),
                name: format!("token-grant-{index}-fresh"),
                ..input.clone()
            })
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: a fresh token failed: {error:?}"))
            .access_grant;
        assert_ne!(
            fresh.grant_id, created.grant_id,
            "case {token:?}: a fresh token must mint a new grant, got {:?} twice",
            fresh.grant_id
        );
    }
}

#[tokio::test]
async fn the_member_combinations_the_model_forbids_are_refused() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("validation", "grant-validation").await;

    // Rows: each is a request the model's own documentation rules out —
    // a value outside an enum's declared set, a principal naming nobody
    // (`AccessGrantPrincipal`: "Specify principalId, principalAttributes, or both"),
    // attributes on a principal type that does not support them ("Supported only for
    // IDC_USER principals"), and CUSTOM without the action list it consists of
    // ("Access limited to an explicit list of actions, specified in scopedActions").
    let attributes = Some(vec![AccessGrantPrincipalAttribute {
        key: "department".to_string(),
        value: "observability".to_string(),
    }]);
    for (case, input) in [
        (
            "a permission the model does not declare",
            CreateAccessGrantInput {
                permission: "FULL_CONTROL".to_string(),
                ..create_input(&domain_id, &space_id, "bad-permission")
            },
        ),
        (
            "a principalType the model does not declare",
            CreateAccessGrantInput {
                principal: principal("IAM_GROUP", "analysts"),
                ..create_input(&domain_id, &space_id, "bad-principal-type")
            },
        ),
        (
            "a principal naming neither an id nor attributes",
            CreateAccessGrantInput {
                principal: AccessGrantPrincipal {
                    principal_type: "IDC_USER".to_string(),
                    principal_id: None,
                    principal_attributes: None,
                },
                ..create_input(&domain_id, &space_id, "no-principal")
            },
        ),
        (
            "a principal whose only attributes list is empty",
            CreateAccessGrantInput {
                principal: AccessGrantPrincipal {
                    principal_type: "IDC_USER".to_string(),
                    principal_id: None,
                    principal_attributes: Some(Vec::new()),
                },
                ..create_input(&domain_id, &space_id, "empty-attributes")
            },
        ),
        (
            "principalAttributes on a principal type that does not support them",
            CreateAccessGrantInput {
                principal: AccessGrantPrincipal {
                    principal_type: "IAM_USER".to_string(),
                    principal_id: Some("arn:aws:iam::000000000000:user/analyst".to_string()),
                    principal_attributes: attributes.clone(),
                },
                ..create_input(&domain_id, &space_id, "attributes-on-iam-user")
            },
        ),
        (
            "CUSTOM with no scopedActions",
            CreateAccessGrantInput {
                permission: "CUSTOM".to_string(),
                ..create_input(&domain_id, &space_id, "custom-without-actions")
            },
        ),
        (
            "CUSTOM with an empty scopedActions list",
            CreateAccessGrantInput {
                permission: "CUSTOM".to_string(),
                scoped_actions: Some(Vec::new()),
                ..create_input(&domain_id, &space_id, "custom-with-empty-actions")
            },
        ),
    ] {
        expect_modelled_error(
            case,
            omni.create_access_grant(&input).await,
            400,
            "ValidationException",
        );
    }

    // The same requests with the forbidden combination removed are accepted, so the
    // refusals above are the rule rather than a blanket no.
    omni.create_access_grant(&CreateAccessGrantInput {
        principal: AccessGrantPrincipal {
            principal_type: "IDC_USER".to_string(),
            principal_id: None,
            principal_attributes: attributes,
        },
        ..create_input(&domain_id, &space_id, "attributes-on-an-idc-user")
    })
    .await
    .expect("case \"attributes on an IDC_USER principal\": the grant is created");
    omni.create_access_grant(&CreateAccessGrantInput {
        permission: "CUSTOM".to_string(),
        scoped_actions: Some(vec![scoped_actions(&["cloudwatch:GetTelemetry"], None)]),
        ..create_input(&domain_id, &space_id, "custom-with-actions")
    })
    .await
    .expect("case \"CUSTOM with an action list\": the grant is created");
}

#[tokio::test]
async fn a_grant_scoped_to_an_access_profile_is_recorded_against_it() {
    let omni = Omni::new();
    let (domain_id, space_id) = omni.domain_and_space("trust", "grant-trust").await;
    let (first_profile, first_arn) = omni.access_profile("trust", &space_id, "first").await;
    let (second_profile, _) = omni.access_profile("trust", &space_id, "second").await;
    let elsewhere_space = omni.space("trust", &domain_id, "grant-trust-other").await;
    let (elsewhere_profile, _) = omni
        .access_profile("trust", &elsewhere_space, "elsewhere")
        .await;

    // Before any grant, no profile is assumable: nothing authorizes it.
    for (case, space, profile) in [
        ("the named profile", space_id.as_str(), first_profile.as_str()),
        ("another profile of the space", space_id.as_str(), second_profile.as_str()),
        (
            "a profile of another space",
            elsewhere_space.as_str(),
            elsewhere_profile.as_str(),
        ),
    ] {
        let status = omni.assume_status(case, space, profile).await;
        assert_eq!(
            status, ASSUME_STATUS_DENIED,
            "case {case:?}: with no grant recorded, expected {ASSUME_STATUS_DENIED}, got {status}"
        );
    }

    // A grant whose scoped actions are limited to one profile's ARN applies to that
    // profile and to no other.
    omni.create_access_grant(&CreateAccessGrantInput {
        permission: "CUSTOM".to_string(),
        principal: principal("ALERT", "alert-high-latency"),
        scoped_actions: Some(vec![scoped_actions(
            &["cloudwatch:GetTelemetry"],
            Some(vec![resource_scope(
                ACCESS_PROFILE_RESOURCE_TYPE,
                Some(vec![first_arn.clone()]),
            )]),
        )]),
        ..create_input(&domain_id, &space_id, "assume-the-first-profile")
    })
    .await
    .expect("the grant naming one profile's ARN is created");
    for (case, space, profile, expected) in [
        (
            "the profile the grant names",
            space_id.as_str(),
            first_profile.as_str(),
            ASSUME_STATUS_ALLOWED,
        ),
        (
            "a profile the grant does not name",
            space_id.as_str(),
            second_profile.as_str(),
            ASSUME_STATUS_DENIED,
        ),
        (
            "a profile of another space",
            elsewhere_space.as_str(),
            elsewhere_profile.as_str(),
            ASSUME_STATUS_DENIED,
        ),
    ] {
        let status = omni.assume_status(case, space, profile).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}"
        );
    }

    // A resource scope of the profile type that narrows to no ARNs defaults to "*"
    // (`ResourceScope.resourceArns`: "When absent, defaults to \"*\""), so it applies to
    // every profile of ITS space — and still to none of another space's.
    omni.create_access_grant(&CreateAccessGrantInput {
        permission: "CUSTOM".to_string(),
        principal: principal("ALERT", "alert-every-profile"),
        scoped_actions: Some(vec![scoped_actions(
            &["cloudwatch:GetTelemetry"],
            Some(vec![resource_scope(ACCESS_PROFILE_RESOURCE_TYPE, None)]),
        )]),
        ..create_input(&domain_id, &space_id, "assume-every-profile")
    })
    .await
    .expect("the grant scoping the whole profile type is created");
    for (case, space, profile, expected) in [
        (
            "every profile of the grant's space",
            space_id.as_str(),
            second_profile.as_str(),
            ASSUME_STATUS_ALLOWED,
        ),
        (
            "a profile of another space",
            elsewhere_space.as_str(),
            elsewhere_profile.as_str(),
            ASSUME_STATUS_DENIED,
        ),
    ] {
        let status = omni.assume_status(case, space, profile).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status}"
        );
    }

    // A grant with no scoped actions at all names no resource, so it records nothing
    // against the profiles of its own space.
    let (bare_space, bare_profile) = {
        let space = omni.space("bare", &domain_id, "grant-trust-bare").await;
        let (profile, _) = omni.access_profile("bare", &space, "bare").await;
        (space, profile)
    };
    omni.create_access_grant(&create_input(&domain_id, &bare_space, "no-scoped-actions"))
        .await
        .expect("the grant with no scoped actions is created");
    let status = omni
        .assume_status("a grant naming no resource", &bare_space, &bare_profile)
        .await;
    assert_eq!(
        status, ASSUME_STATUS_DENIED,
        "case \"a grant naming no resource\": expected {ASSUME_STATUS_DENIED}, got {status}"
    );
}

#[tokio::test]
async fn the_access_grant_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    // The parents are created through the handlers holding the same engine the router's
    // service holds, which is what makes the wire calls below see them.
    let (domain_id, space_id) = omni.domain_and_space("wire", "grant-wire").await;

    let codec = CborCodec::default();
    let (status, body) = omni
        .over_the_wire(
            "CreateAccessGrant",
            CREATE_ACCESS_GRANT_INPUT_SCHEMA,
            &create_input(&domain_id, &space_id, "wire-grant"),
        )
        .await;
    assert_eq!(
        status,
        200,
        "CreateAccessGrant over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let created = {
        let mut deserializer = codec.create_deserializer(&body);
        CreateAccessGrantOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .access_grant
    };
    assert!(
        is_grant_id(&created.grant_id),
        "grantId {:?} does not satisfy {GRANT_ID_PATTERN}",
        created.grant_id
    );

    // The grant the wire call created is readable over the wire too, as persisted.
    let (status, body) = omni
        .over_the_wire(
            "GetAccessGrant",
            GET_ACCESS_GRANT_INPUT_SCHEMA,
            &GetAccessGrantInput {
                grant_id: created.grant_id.clone(),
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "GetAccessGrant over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let fetched = {
        let mut deserializer = codec.create_deserializer(&body);
        GetAccessGrantOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .access_grant
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
            "GetAccessGrant",
            GET_ACCESS_GRANT_INPUT_SCHEMA,
            &GetAccessGrantInput {
                grant_id: MISSING_GRANT_ID.to_string(),
            },
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 404,
        "GetAccessGrant over the wire answered {status} for a missing grant: {rendered:?}"
    );
    assert!(
        rendered.contains("ResourceNotFoundException"),
        "the wire error body does not name ResourceNotFoundException: {rendered:?}"
    );

    // A modelled constraint is refused before the handler runs, as a 400.
    let (status, body) = omni
        .over_the_wire(
            "CreateAccessGrant",
            CREATE_ACCESS_GRANT_INPUT_SCHEMA,
            &create_input(&domain_id, &space_id, "a name with spaces"),
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 400,
        "a name outside ^[a-zA-Z0-9_-]+$ answered {status}: {rendered:?}"
    );
    assert!(
        rendered.contains("ValidationException"),
        "the wire refusal does not name ValidationException: {rendered:?}"
    );
}
