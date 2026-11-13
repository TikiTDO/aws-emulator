// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the identity-provider registration and product-subscription
//! loop, driven over the real wire path: every call goes through the router's
//! `dispatch`, so the request is deserialized, constraint-checked, handled and
//! serialized exactly as a client's would be.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! request:
//!
//! * every operation answers from the single shared store — what a register/subscribe
//!   persisted is what a later list/stop/deregister reports, and what a deregister/stop
//!   removed is gone;
//! * a resource gets a minted ARN of the modelled `Arn` shape, recorded in state;
//! * `MaxResults`/`NextToken` page a list exactly once over every element;
//! * `Filters` select on the attributes the operation's model documents, and refuse
//!   anything else;
//! * the registration status transition the clock owes is materialised on a later read.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::{
    ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState,
    EmulatedService,
};

/// This service's sigv4 signing name (`OPERATIONS.md`, "Wire protocol").
pub(crate) const SIGNING_NAME: &str = "license-manager-user-subscriptions";
/// The region the test requests are signed for; every minted ARN must carry it.
pub(crate) const REGION: &str = "us-west-2";
/// The canonical account id every emulated ARN carries.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The products `RegisterIdentityProvider`'s `Product` documentation lists:
/// VISUAL_STUDIO_ENTERPRISE | VISUAL_STUDIO_PROFESSIONAL | OFFICE_PROFESSIONAL_PLUS |
/// OFFICE_STANDARD | REMOTE_DESKTOP_SERVICES.
///
/// The published literals, spelled out here rather than read from the production set, so
/// a change to `state::PRODUCTS` fails a test instead of silently changing what this
/// suite accepts; `refusals::the_enforced_product_set_is_the_documented_one` is the
/// assertion that ties the two together.
pub(crate) const PRODUCTS: &[&str] = &[
    "VISUAL_STUDIO_ENTERPRISE",
    "VISUAL_STUDIO_PROFESSIONAL",
    "OFFICE_PROFESSIONAL_PLUS",
    "OFFICE_STANDARD",
    "REMOTE_DESKTOP_SERVICES",
];

/// Percent-encode ONE URI path segment, for the tagging routes that bind `ResourceArn`
/// to `@httpLabel` (`/tags/{ResourceArn}`): an ARN carries `:` and `/`, and a `/` left as
/// it is would split the label into another path segment.
///
/// The one encoder for this crate's suites, so two suites cannot disagree about what a
/// client sends. It encodes the three characters an ARN can carry that a path segment
/// cannot take literally (`%` first, so an encoding is not re-encoded), which is the
/// substitution the suite this helper was lifted out of already made.
pub(crate) fn encoded_label(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// One emulator: the router that dispatches, plus the shared engine its service was
/// built from (so a test can read the store the handlers wrote).
///
/// Shared with the other suites in this crate (see `tests/all.rs`) so there is ONE wire
/// harness for the service rather than a copy per suite.
pub(crate) struct Harness {
    pub(crate) router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

pub(crate) fn harness() -> Harness {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The tests sign nothing; authentication is the framework's concern, not this
    // service's.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    // Amazon EC2 beside this service on the SAME engine: a registration's `Settings` name
    // EC2's resources, and this service resolves them by asking EC2 over the in-process
    // callback, so the owner of subnets and security groups has to be mounted for that
    // question to have an answer — exactly as `instance_fleet_resolution` mounts it for the
    // instance a user association names.
    router.register_service(ec2::EmulatedService::new(Arc::clone(&engine)));
    // `RegisterIdentityProvider` resolves an AWS-managed `DirectoryId` against the
    // account's directories, so the directories these registrations name are registered
    // in that fleet first.
    create_directories(&engine, AWS_MANAGED_DIRECTORIES);
    // And it resolves a registration's `Settings` against what the account has in Amazon
    // EC2 — License Manager creates the activation-server VPC endpoints in those subnets
    // with that security group — so the network every suite's registrations name is created
    // there first, for the same reason the directories are.
    create_network(
        &engine,
        ACCOUNT_VPC,
        ACCOUNT_SUBNETS,
        ACCOUNT_SECURITY_GROUPS,
    );
    Harness { router, engine }
}

/// The AWS-managed directories this suite's registrations are for: every `d-` id
/// [`REGISTRATIONS`] names. A `sd-` id is a directory shared INTO this account, which
/// `ds:DescribeSharedDirectories` is what reports and which this service therefore does
/// not resolve, so none is created here.
const AWS_MANAGED_DIRECTORIES: &[&str] = &["d-0123456789", "d-00000000ff", "d-123456789a"];

/// Register `directory_ids` in the account's directory fleet, each one ACTIVE — the stage
/// an identity provider can be registered for.
///
/// Written through the state seam the account's own directory fleet is seeded by
/// (`register_account_directory`) rather than through a Directory Service API, for the
/// same reason the suite seeds instances that way: a create mints the id it chooses, and
/// these registrations are written against specific ones.
pub(crate) fn create_directories(engine: &Arc<engine::InMemoryEngine>, directory_ids: &[&str]) {
    let store = engine.state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
    let mut store = store.lock();
    for directory_id in directory_ids {
        store.register_account_directory(*directory_id, DIRECTORY_STAGE_ACTIVE);
    }
}

/// The stage a directory has to be in before an identity provider can be registered for
/// it, spelled out here rather than read from the production constant so a change to it
/// fails a test instead of silently changing what this suite accepts.
pub(crate) const DIRECTORY_STAGE_ACTIVE: &str = "Active";

/// The VPC of the account's own Amazon EC2 store that [`harness`] puts every subnet and
/// security group its suites' registrations name into.
///
/// One VPC, because one registration's subnets and security group are the resources of
/// the ONE VPC License Manager creates its activation-server VPC endpoints in.
pub(crate) const ACCOUNT_VPC: &str = "vpc-0123456789abcdef0";

/// The IPv4 block every VPC [`create_network`] writes carries, and the block every
/// subnet of it is carved from. Two VPCs of one account may carry the same block (nothing
/// routes between them here), so one constant serves every VPC the suites create.
const ACCOUNT_VPC_CIDR: &str = "10.0.0.0/8";

/// The Availability Zone the account's subnets are in — a zone of [`REGION`], which is
/// what EC2's own zone list reports for it.
const ACCOUNT_ZONE: &str = "us-west-2a";

/// Every subnet id the suites of this target register an identity provider with, or add
/// to one: `Subnets` names "the ID of one or more subnets in which License Manager will
/// create a VPC endpoint", so a registration is accepted only for subnets the account
/// really holds, exactly as it is accepted only for a directory the account really has.
///
/// Each matches the member's `^subnet-[a-z0-9]{8,17}` pattern. A suite that means to name
/// a subnet the account does NOT hold names one that is not here (see
/// `settings_network_resolution`).
pub(crate) const ACCOUNT_SUBNETS: &[&str] = &[
    "subnet-0123456789abcdef0",
    "subnet-1111111111111111a",
    "subnet-2222222222222222b",
    "subnet-3333333333333333c",
    "subnet-4444444444444444d",
    "subnet-5555555555555555e",
    "subnet-6666666666666666f",
    "subnet-7777777777777777a",
    "subnet-8888888888888888b",
    "subnet-9999999999999999a",
    "subnet-aaaaaaaaaaaaaaaab",
];

/// Every security group id those registrations name, each matching
/// `^sg-(([0-9a-z]{8})|([0-9a-z]{17}))$` — both published lengths appear.
pub(crate) const ACCOUNT_SECURITY_GROUPS: &[&str] = &[
    "sg-0123abcd",
    "sg-abcd0123",
    "sg-ffffffff",
    "sg-0123456789abcdef0",
    "sg-00000000000000001",
];

/// Write one VPC, `subnets` inside it and `security_groups` of it into the account's own
/// Amazon EC2 store, at the ids given.
///
/// `RegisterIdentityProvider` and `UpdateIdentityProviderSettings` check a registration's
/// `Settings` against THAT store (the VPC endpoints are created in the caller's existing
/// network), so every test that expects those calls to be accepted holds the network they
/// name here first. Written through EC2's own store at chosen ids, rather than through
/// EC2's API, for the same reason the directories are: a create mints the id it chooses,
/// and these registrations name specific ones.
pub(crate) fn create_network(
    engine: &Arc<engine::InMemoryEngine>,
    vpc_id: &str,
    subnets: &[&str],
    security_groups: &[&str],
) {
    let store = ec2::state::service_state(&**engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = store.lock();
    store.vpcs.insert(
        vpc_id.to_string(),
        ec2::ops::types::Vpc {
            vpc_id: Some(vpc_id.to_string()),
            state: Some(ec2::state::VPC_STATE_AVAILABLE.to_string()),
            cidr_block: Some(ACCOUNT_VPC_CIDR.to_string()),
            ..Default::default()
        },
    );
    // The Region the VPC belongs to: Amazon EC2 is a regional service, so a VPC written
    // without one is in no Region and its own describes report neither it nor the subnets
    // and groups of it. Written beside the record here because this seeds the store at
    // chosen ids rather than going through EC2's own create, which records it.
    store
        .vpc_regions
        .insert(vpc_id.to_string(), REGION.to_string());
    for (index, subnet_id) in subnets.iter().enumerate() {
        store.subnets.insert(
            (*subnet_id).to_string(),
            ec2::ops::types::Subnet {
                subnet_id: Some((*subnet_id).to_string()),
                vpc_id: Some(vpc_id.to_string()),
                state: Some(ec2::state::SUBNET_STATE_AVAILABLE.to_string()),
                // A distinct /24 of the VPC's block per subnet, because two subnets of one
                // VPC cannot overlap — distinct for every index a `&[&str]` can hold that
                // this store also can (65 536 of them, past EC2's own subnet ceiling).
                cidr_block: Some(format!("10.{}.{}.0/24", index / 256, index % 256)),
                availability_zone: Some(ACCOUNT_ZONE.to_string()),
                availability_zone_id: ec2::state::zone_id_for_zone_name(ACCOUNT_ZONE),
                ..Default::default()
            },
        );
    }
    for group_id in security_groups {
        store.security_groups.insert(
            (*group_id).to_string(),
            ec2::ops::types::SecurityGroup {
                group_id: Some((*group_id).to_string()),
                vpc_id: Some(vpc_id.to_string()),
                ..Default::default()
            },
        );
    }
}

/// Remove `subnets` and `security_groups` from the account's own Amazon EC2 store: the
/// counterpart of [`create_network`], written through the same store for the same reason —
/// a suite that deletes a resource a registration already resolved names that specific id.
pub(crate) fn delete_network(
    engine: &Arc<engine::InMemoryEngine>,
    subnets: &[&str],
    security_groups: &[&str],
) {
    let store = ec2::state::service_state(&**engine)
        .expect("the in-memory engine carries the account's EC2 store");
    let mut store = store.lock();
    for subnet_id in subnets {
        store.subnets.remove(*subnet_id);
    }
    for group_id in security_groups {
        store.security_groups.remove(*group_id);
    }
}

/// The `UpdateSettings` payload of an `UpdateIdentityProviderSettings` request, as the
/// suites that exercise that operation spell one.
///
/// One builder for the whole crate, so two suites cannot disagree about what a client
/// sends: leaving `security_group_id` out of the payload is what keeps the stored group.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct UpdateSettingsPayload {
    pub(crate) add_subnets: &'static [&'static str],
    pub(crate) remove_subnets: &'static [&'static str],
    pub(crate) security_group_id: Option<&'static str>,
}

impl UpdateSettingsPayload {
    /// The `UpdateSettings` member on its own.
    pub(crate) fn settings(self) -> Value {
        let mut settings = json!({
            "AddSubnets": self.add_subnets,
            "RemoveSubnets": self.remove_subnets,
        });
        if let Some(security_group_id) = self.security_group_id {
            settings["SecurityGroupId"] = json!(security_group_id);
        }
        settings
    }

    /// The whole request body, addressing the registration `provider_arn` names.
    pub(crate) fn request(self, provider_arn: &str) -> Value {
        json!({ "IdentityProviderArn": provider_arn, "UpdateSettings": self.settings() })
    }
}

/// What a call answered: the HTTP status, the wire error code (if any) and the body.
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

impl Reply {
    pub(crate) fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            self.status, self.body
        );
        &self.body
    }

    /// The `message` of a refusal, as a client reads it off the error body.
    pub(crate) fn message(&self) -> Option<&str> {
        self.body.get("message").and_then(Value::as_str)
    }
}

/// Dispatch one request through the router: the method, the path (with any query
/// string) and the JSON body, if the operation carries one.
///
/// Every call in this crate's suites goes through here, so a request is deserialized,
/// constraint-checked, handled and serialized exactly as a client's would be.
pub(crate) async fn send(
    h: &Harness,
    method: &str,
    path_and_query: &str,
    body: Option<&Value>,
) -> Reply {
    let response = dispatch(h, method, path_and_query, body).await;
    let status = response.status().as_u16();
    let header_error = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error_type = header_error.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    Reply {
        status,
        error_type,
        body,
    }
}

/// The RAW response one request gets, before anything is read off it — the same dispatch
/// [`send`] folds into a [`Reply`], for a suite whose rule is about the response ITSELF
/// (its full header set) rather than about the status, the code and the body a client
/// branches on.
pub(crate) async fn dispatch(
    h: &Harness,
    method: &str,
    path_and_query: &str,
    body: Option<&Value>,
) -> http::Response<bytes::Bytes> {
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://{SIGNING_NAME}.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/{REGION}/{SIGNING_NAME}\
                 /aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .body(bytes::Bytes::from(match body {
            Some(body) => serde_json::to_vec(body).expect("the request body serializes"),
            None => Vec::new(),
        }))
        .expect("a well-formed request");
    h.router.dispatch(request).await
}

/// A `POST` of `body` to `path` — the shape every operation of this service's RPC-style
/// surface takes.
pub(crate) async fn call(h: &Harness, path: &str, body: &Value) -> Reply {
    send(h, "POST", path, Some(body)).await
}

pub(crate) async fn register(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/RegisterIdentityProvider", body).await
}

pub(crate) async fn deregister(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/DeregisterIdentityProvider", body).await
}

pub(crate) async fn list_providers(h: &Harness, body: &Value) -> Reply {
    call(h, "/identity-provider/ListIdentityProviders", body).await
}

pub(crate) async fn start_subscription(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/StartProductSubscription", body).await
}

pub(crate) async fn stop_subscription(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/StopProductSubscription", body).await
}

pub(crate) async fn list_subscriptions(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/ListProductSubscriptions", body).await
}

/// One registration to exercise. The rows differ along what the rules generalise over:
/// the directory-id form, the product, the number of subnets, the security-group id
/// form, and whether tags are supplied.
#[derive(Debug, Clone, Copy)]
struct Registration {
    case: &'static str,
    directory_id: &'static str,
    product: &'static str,
    subnets: &'static [&'static str],
    /// `^sg-(([0-9a-z]{8})|([0-9a-z]{17}))$` — both published lengths appear below.
    security_group_id: &'static str,
    tagged: bool,
}

/// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; `Subnets` has `@length(min: 1)`.
const REGISTRATIONS: &[Registration] = &[
    Registration {
        case: "aws-managed directory, one subnet, 8-character security group",
        directory_id: "d-0123456789",
        product: PRODUCTS[0],
        subnets: &["subnet-0123456789abcdef0"],
        security_group_id: "sg-0123abcd",
        tagged: false,
    },
    Registration {
        case: "shared directory id, 17-character security group, tagged",
        directory_id: "sd-abcdef0123",
        product: PRODUCTS[1],
        subnets: &["subnet-1111111111111111a", "subnet-2222222222222222b"],
        security_group_id: "sg-0123456789abcdef0",
        tagged: true,
    },
    Registration {
        case: "same directory, a second product",
        directory_id: "d-0123456789",
        product: PRODUCTS[2],
        subnets: &["subnet-3333333333333333c"],
        security_group_id: "sg-abcd0123",
        tagged: true,
    },
    Registration {
        case: "three subnets",
        directory_id: "d-00000000ff",
        product: PRODUCTS[3],
        subnets: &[
            "subnet-4444444444444444d",
            "subnet-5555555555555555e",
            "subnet-6666666666666666f",
        ],
        security_group_id: "sg-ffffffff",
        tagged: false,
    },
    Registration {
        case: "remote desktop services",
        directory_id: "d-123456789a",
        product: PRODUCTS[4],
        subnets: &["subnet-7777777777777777a"],
        security_group_id: "sg-00000000000000001",
        tagged: false,
    },
];

/// How long a read waits before it must see a transition the clock owed.
///
/// A clock-owed transition in this emulator falls due inside 250-750 ms of the call that
/// started it, so a read this far later must observe it whatever span the service chose
/// inside that window.
pub(crate) const PAST_THE_OWED_TRANSITION: std::time::Duration =
    std::time::Duration::from_millis(900);

pub(crate) fn provider_union(directory_id: &str) -> Value {
    json!({ "ActiveDirectoryIdentityProvider": { "DirectoryId": directory_id } })
}

/// The FIXED sentence this service fails a refusal closed with
/// (`scaffolded::UNAUTHORIZED_EXCEPTION_MESSAGE`), spelled out here rather than read from
/// the production constant so a change to it fails a test instead of silently changing what
/// this suite accepts.
const UNAUTHORIZED_MESSAGE: &str =
    "Unauthorized. Please check the request and access policy and try again.";

impl Registration {
    fn request(&self) -> Value {
        let mut body = json!({
            "IdentityProvider": provider_union(self.directory_id),
            "Product": self.product,
            "Settings": {
                "Subnets": self.subnets,
                "SecurityGroupId": self.security_group_id,
            },
        });
        if self.tagged {
            body["Tags"] = json!({ "team": "licensing", "case": self.case });
        }
        body
    }
}

/// The `arn:` prefix every resource of `kind` minted for this request must carry.
pub(crate) fn arn_prefix(kind: &str) -> String {
    format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:{kind}/")
}

pub(crate) fn as_str<'a>(value: &'a Value, member: &str, case: &str) -> &'a str {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: {member} is missing from {value:?}"))
}

/// RULE: a registration is computed from, and persisted in, state — the summary echoes
/// what was stored, carries a minted `IdentityProviderArn`, and `ListIdentityProviders`
/// reports exactly the registrations that were created, in registration order.
#[tokio::test]
async fn registrations_round_trip_through_list_for_every_registration_shape() {
    let h = harness();
    let mut minted = Vec::new();

    for row in REGISTRATIONS {
        let reply = register(&h, &row.request()).await;
        let body = reply.ok(row.case);
        let summary = body
            .get("IdentityProviderSummary")
            .unwrap_or_else(|| panic!("case {:?}: no IdentityProviderSummary in {body:?}", row.case));

        let arn = as_str(summary, "IdentityProviderArn", row.case);
        assert!(
            arn.starts_with(&arn_prefix("identity-provider")),
            "case {:?}: expected an ARN under {}, got {arn:?}",
            row.case,
            arn_prefix("identity-provider")
        );
        assert_eq!(
            as_str(summary, "Product", row.case),
            row.product,
            "case {:?}: the summary must carry the registered product, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(summary, "Status", row.case),
            "REGISTERING",
            "case {:?}: a fresh registration is still provisioning, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(summary, "OwnerAccountId", row.case),
            ACCOUNT,
            "case {:?}: wrong owner account, got {summary:?}",
            row.case
        );
        assert_eq!(
            summary.get("Settings"),
            Some(&json!({
                "Subnets": row.subnets,
                "SecurityGroupId": row.security_group_id,
            })),
            "case {:?}: the summary must carry the stored settings, got {summary:?}",
            row.case
        );
        assert_eq!(
            summary.get("IdentityProvider"),
            Some(&provider_union(row.directory_id)),
            "case {:?}: the summary must carry the registered provider, got {summary:?}",
            row.case
        );
        minted.push((row.case, arn.to_string()));
    }

    let listed = list_providers(&h, &json!({})).await;
    let body = listed.ok("list every registration");
    let summaries = body
        .get("IdentityProviderSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("IdentityProviderSummaries is required, got {body:?}"));
    let listed_arns: Vec<&str> = summaries
        .iter()
        .map(|summary| as_str(summary, "IdentityProviderArn", "list every registration"))
        .collect();
    let expected: Vec<&str> = minted.iter().map(|(_, arn)| arn.as_str()).collect();
    assert_eq!(
        listed_arns, expected,
        "the list must report every registration once, in registration order"
    );
    assert!(
        body.get("NextToken").is_none_or(Value::is_null),
        "an unpaged list is complete, so it has no NextToken: {body:?}"
    );
}

/// RULE: one registration per (identity provider, product) pair — a second one conflicts
/// with the record already in state instead of shadowing it.
#[tokio::test]
async fn re_registering_the_same_provider_and_product_conflicts() {
    let h = harness();
    for row in REGISTRATIONS {
        register(&h, &row.request()).await.ok(row.case);
        let again = register(&h, &row.request()).await;
        assert_eq!(
            again.status, 500,
            "case {:?}: ConflictException is modelled at HTTP 500, got {again:?}",
            row.case
        );
        assert_eq!(
            again.error_type.as_deref(),
            Some("ConflictException"),
            "case {:?}: expected ConflictException, got {again:?}",
            row.case
        );
    }
    let listed = list_providers(&h, &json!({})).await;
    let summaries = listed.ok("no duplicates")["IdentityProviderSummaries"]
        .as_array()
        .expect("IdentityProviderSummaries is required")
        .len();
    assert_eq!(
        summaries,
        REGISTRATIONS.len(),
        "a refused re-registration must not add a record"
    );
}

// `Settings` is OPTIONAL on `RegisterIdentityProviderRequest`, so a registration that
// omits it is accepted and persisted with no subnet and no security group; that rule is
// asserted in `optional_settings_registration`.

/// RULE: a provider that names no directory at all identifies nothing to register, so it
/// is refused rather than stored under an empty key.
#[tokio::test]
async fn a_provider_naming_no_directory_is_refused() {
    let h = harness();
    let cases = [
        ("empty union member", json!({"ActiveDirectoryIdentityProvider": {}})),
        (
            "only an active-directory type",
            json!({"ActiveDirectoryIdentityProvider": {"ActiveDirectoryType": "AWS_MANAGED"}}),
        ),
    ];
    for (case, provider) in cases {
        let body = json!({
            "IdentityProvider": provider,
            "Product": PRODUCTS[0],
            "Settings": { "Subnets": ["subnet-0123456789abcdef0"], "SecurityGroupId": "sg-0123abcd" },
        });
        let reply = register(&h, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a 400 refusal, got {reply:?}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
    }
}

/// RULE: a value outside a modelled constraint is refused with `ValidationException`
/// (HTTP 400) — the published `DirectoryId` and `SecurityGroupId` patterns and the
/// `Subnets` minimum length, each tested on a value just outside the bound.
#[tokio::test]
async fn values_outside_the_modelled_constraints_are_refused() {
    let h = harness();
    let settings = json!({
        "Subnets": ["subnet-0123456789abcdef0"],
        "SecurityGroupId": "sg-0123abcd",
    });
    let cases = [
        (
            "DirectoryId with nine hex digits (pattern needs ten)",
            json!({"IdentityProvider": provider_union("d-012345678"), "Product": PRODUCTS[0], "Settings": settings}),
        ),
        (
            "DirectoryId with a non-hex digit",
            json!({"IdentityProvider": provider_union("d-0123456789g"), "Product": PRODUCTS[0], "Settings": settings}),
        ),
        (
            "DirectoryId with an unknown prefix",
            json!({"IdentityProvider": provider_union("x-0123456789"), "Product": PRODUCTS[0], "Settings": settings}),
        ),
        (
            "SecurityGroupId with seven characters (pattern allows 8 or 17)",
            json!({
                "IdentityProvider": provider_union("d-0123456789"),
                "Product": PRODUCTS[0],
                "Settings": {"Subnets": ["subnet-0123456789abcdef0"], "SecurityGroupId": "sg-0123ab"},
            }),
        ),
        (
            "SecurityGroupId with nine characters (one past the shorter bound)",
            json!({
                "IdentityProvider": provider_union("d-0123456789"),
                "Product": PRODUCTS[0],
                "Settings": {"Subnets": ["subnet-0123456789abcdef0"], "SecurityGroupId": "sg-0123abcde"},
            }),
        ),
        (
            "no subnets at all (Subnets has @length(min: 1))",
            json!({
                "IdentityProvider": provider_union("d-0123456789"),
                "Product": PRODUCTS[0],
                "Settings": {"Subnets": [], "SecurityGroupId": "sg-0123abcd"},
            }),
        ),
        (
            "no Product at all (@required)",
            json!({"IdentityProvider": provider_union("d-0123456789"), "Settings": settings}),
        ),
    ];
    for (case, body) in cases {
        let reply = register(&h, &body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a 400 refusal, got {reply:?}"
        );
    }
    let listed = list_providers(&h, &json!({})).await;
    assert_eq!(
        listed.ok("nothing registered")["IdentityProviderSummaries"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "a refused registration must store nothing"
    );
}

/// One product user to exercise, differing along the user-name form and whether a domain
/// is supplied.
#[derive(Debug, Clone, Copy)]
struct Subscriber {
    case: &'static str,
    username: &'static str,
    domain: Option<&'static str>,
}

const SUBSCRIBERS: &[Subscriber] = &[
    Subscriber {
        case: "plain user name, no domain",
        username: "alice",
        domain: None,
    },
    Subscriber {
        case: "user name with a domain",
        username: "bob",
        domain: Some("example.com"),
    },
    Subscriber {
        case: "upn-shaped user name",
        username: "carol@example.com",
        domain: Some("example.com"),
    },
    Subscriber {
        case: "user name with punctuation",
        username: "dave.o-brien",
        domain: Some("corp.example.com"),
    },
];

fn subscription_request(row: &Subscriber, directory_id: &str, product: &str) -> Value {
    let mut body = json!({
        "Username": row.username,
        "IdentityProvider": provider_union(directory_id),
        "Product": product,
    });
    if let Some(domain) = row.domain {
        body["Domain"] = json!(domain);
    }
    body
}

/// RULE: a subscription is persisted against the registration it was created through —
/// the summary carries a minted `ProductUserArn` and a start date, a list reports exactly
/// the subscriptions in state, and stopping one ends it: the ended subscription is still
/// reported while its teardown runs (see `product_subscription_teardown`) but resolves for
/// nothing, so a second stop is refused, and once the teardown the clock owes has completed
/// the list reports it no more.
#[tokio::test]
async fn product_subscriptions_round_trip_and_stop_ends_them() {
    let h = harness();
    let registration = REGISTRATIONS[0];
    register(&h, &registration.request())
        .await
        .ok(registration.case);

    let mut arns = Vec::new();
    for row in SUBSCRIBERS {
        let request = subscription_request(row, registration.directory_id, registration.product);
        let reply = start_subscription(&h, &request).await;
        let summary = reply.ok(row.case)["ProductUserSummary"].clone();
        let arn = as_str(&summary, "ProductUserArn", row.case).to_string();
        assert!(
            arn.starts_with(&arn_prefix("user")),
            "case {:?}: expected an ARN under {}, got {arn:?}",
            row.case,
            arn_prefix("user")
        );
        assert_eq!(
            as_str(&summary, "Status", row.case),
            "SUBSCRIBED",
            "case {:?}: a started subscription is SUBSCRIBED, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(&summary, "Username", row.case),
            row.username,
            "case {:?}: wrong user name, got {summary:?}",
            row.case
        );
        assert_eq!(
            summary.get("Domain").and_then(Value::as_str),
            row.domain,
            "case {:?}: the summary must carry the stored domain, got {summary:?}",
            row.case
        );
        let start = as_str(&summary, "SubscriptionStartDate", row.case);
        assert!(
            start.len() == 20 && start.ends_with('Z') && start.contains('T'),
            "case {:?}: expected an ISO-8601 UTC instant, got {start:?}",
            row.case
        );
        assert!(
            summary.get("SubscriptionEndDate").is_none_or(Value::is_null),
            "case {:?}: an active subscription has not ended: {summary:?}",
            row.case
        );
        arns.push((row, arn));
    }

    let listed = list_subscriptions(
        &h,
        &json!({
            "IdentityProvider": provider_union(registration.directory_id),
            "Product": registration.product,
        }),
    )
    .await;
    let body = listed.ok("list every subscription");
    let summaries = body["ProductUserSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected ProductUserSummaries in {body:?}"))
        .clone();
    let listed_arns: Vec<&str> = summaries
        .iter()
        .map(|summary| as_str(summary, "ProductUserArn", "list every subscription"))
        .collect();
    let expected: Vec<&str> = arns.iter().map(|(_, arn)| arn.as_str()).collect();
    assert_eq!(
        listed_arns, expected,
        "the list must report every subscription once, in subscription order"
    );

    // Half the users are stopped by ARN and half by (Username, IdentityProvider, Product),
    // because the model lets a caller address a product user either way.
    for (index, (row, arn)) in arns.iter().enumerate() {
        let request = if index % 2 == 0 {
            json!({ "ProductUserArn": arn })
        } else {
            subscription_request(row, registration.directory_id, registration.product)
        };
        let reply = stop_subscription(&h, &request).await;
        let summary = reply.ok(row.case)["ProductUserSummary"].clone();
        assert_eq!(
            as_str(&summary, "Status", row.case),
            "UNSUBSCRIBED",
            "case {:?}: a stopped subscription is UNSUBSCRIBED, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(&summary, "ProductUserArn", row.case),
            arn.as_str(),
            "case {:?}: the stop must answer for the stored product user, got {summary:?}",
            row.case
        );
        let end = as_str(&summary, "SubscriptionEndDate", row.case);
        assert!(
            end.len() == 20 && end.ends_with('Z') && end.contains('T'),
            "case {:?}: expected an ISO-8601 UTC instant, got {end:?}",
            row.case
        );

        let again = stop_subscription(&h, &request).await;
        // A subscription already ended resolves for nothing, so a second stop has nothing
        // to end. WHICH refusal it gets follows the addressing the request used: the
        // `ProductUserArn` names no live subscription of this account, so that call fails
        // closed, while the members name a user with no subscription to the product, which
        // is the modelled not-found.
        let (status, code) = if index % 2 == 0 {
            (401, "UnauthorizedException")
        } else {
            (404, "ResourceNotFoundException")
        };
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (status, Some(code)),
            "case {:?}: expected HTTP {status} {code} for a second stop, got {again:?}",
            row.case
        );
    }

    // The stop ENDED every subscription rather than deleting it: each one is still
    // reported, in the status the stop answered and with the end date it stamped, so a
    // client polling the list sees the subscription ending.
    let listed = list_subscriptions(
        &h,
        &json!({
            "IdentityProvider": provider_union(registration.directory_id),
            "Product": registration.product,
        }),
    )
    .await;
    let body = listed.ok("every subscription stopped");
    let summaries = body["ProductUserSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("expected ProductUserSummaries in {body:?}"))
        .clone();
    assert_eq!(
        summaries.len(),
        arns.len(),
        "an ended subscription stays reported until its teardown completes, got {summaries:?}"
    );
    for summary in &summaries {
        let case = as_str(summary, "ProductUserArn", "every subscription stopped");
        assert_eq!(
            as_str(summary, "Status", case),
            "UNSUBSCRIBED",
            "case {case:?}: the list reports the ended status, got {summary:?}"
        );
        assert!(
            summary.get("SubscriptionEndDate").is_some(),
            "case {case:?}: the list reports the end date the stop stamped, got {summary:?}"
        );
    }

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let listed = list_subscriptions(
        &h,
        &json!({
            "IdentityProvider": provider_union(registration.directory_id),
            "Product": registration.product,
        }),
    )
    .await;
    assert_eq!(
        listed.ok("past the owed teardown")["ProductUserSummaries"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "a subscription whose teardown completed must no longer be reported"
    );
}

/// RULE: a subscription can only be created through a registration that exists for that
/// product, and only once per user. Subscribing a user is a USER-plane call, so a provider
/// that is not registered for the requested product is not reported as missing: the call
/// fails closed (`UnauthorizedException`), disclosing nothing about what is registered.
#[tokio::test]
async fn subscriptions_require_a_registration_and_are_unique_per_user() {
    let h = harness();
    let registration = REGISTRATIONS[0];
    let subscriber = SUBSCRIBERS[0];

    // No registration yet: for the provider, and for the product.
    let reply = start_subscription(
        &h,
        &subscription_request(&subscriber, registration.directory_id, registration.product),
    )
    .await;
    assert_eq!(
        reply.status, 401,
        "an unregistered provider cannot subscribe a user, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("UnauthorizedException"),
        "an unregistered provider cannot subscribe a user, got {reply:?}"
    );

    register(&h, &registration.request())
        .await
        .ok(registration.case);
    for product in PRODUCTS.iter().filter(|p| **p != registration.product) {
        let reply = start_subscription(
            &h,
            &subscription_request(&subscriber, registration.directory_id, product),
        )
        .await;
        assert_eq!(
            reply.status, 401,
            "case {product:?}: the provider is not registered for this product, got {reply:?}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("UnauthorizedException"),
            "case {product:?}: expected UnauthorizedException, got {reply:?}"
        );
    }

    for row in SUBSCRIBERS {
        let request = subscription_request(row, registration.directory_id, registration.product);
        start_subscription(&h, &request).await.ok(row.case);
        let again = start_subscription(&h, &request).await;
        assert_eq!(
            again.status, 500,
            "case {:?}: ConflictException is modelled at HTTP 500, got {again:?}",
            row.case
        );
        assert_eq!(
            again.error_type.as_deref(),
            Some("ConflictException"),
            "case {:?}: a second subscription of the same user conflicts, got {again:?}",
            row.case
        );
    }
}

/// RULE: `MaxResults` + `NextToken` walk a list exactly once over every element,
/// whatever the page size, and the last page carries no token.
#[tokio::test]
async fn paging_walks_every_registration_exactly_once() {
    let h = harness();
    let mut all = Vec::new();
    for row in REGISTRATIONS {
        let reply = register(&h, &row.request()).await;
        all.push(
            as_str(
                &reply.ok(row.case)["IdentityProviderSummary"],
                "IdentityProviderArn",
                row.case,
            )
            .to_string(),
        );
    }

    for page_size in [1usize, 2, 3, 5, 7] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let reply = list_providers(&h, &request).await;
            let body = reply.ok(&case);
            let page = body["IdentityProviderSummaries"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no summaries in {body:?}"));
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must not exceed MaxResults, got {} entries",
                page.len()
            );
            for summary in page {
                seen.push(as_str(summary, "IdentityProviderArn", &case).to_string());
            }
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case {case:?}: paging must yield every registration exactly once, in order"
        );
        assert_eq!(
            pages,
            all.len().div_ceil(page_size),
            "case {case:?}: wrong number of pages for {} registrations",
            all.len()
        );
    }
}

/// RULE: a page size that cannot make progress, and a token this service did not mint for
/// THIS list, are refused rather than silently reinterpreted.
#[tokio::test]
async fn an_unusable_page_size_or_token_is_refused() {
    let h = harness();
    for row in REGISTRATIONS {
        register(&h, &row.request()).await.ok(row.case);
    }

    let cases = [
        ("MaxResults of zero", json!({ "MaxResults": 0 })),
        ("negative MaxResults", json!({ "MaxResults": -3 })),
        (
            "a token that is not hex",
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            "a hex token from no list of ours",
            json!({ "NextToken": "6f746865723a30" }),
        ),
    ];
    for (case, request) in cases {
        let reply = list_providers(&h, &request).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a 400 refusal, got {reply:?}"
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
    }

    // A cursor is a position in ONE list: the token minted for the unfiltered list is not
    // a position in a filtered one, and once the list has shrunk past it, it is no
    // position at all.
    let first_page = list_providers(&h, &json!({ "MaxResults": 2 })).await;
    let token = first_page.ok("first page")["NextToken"]
        .as_str()
        .expect("a truncated page carries a NextToken")
        .to_string();

    let foreign = list_providers(
        &h,
        &json!({
            "MaxResults": 2,
            "NextToken": token,
            "Filters": [{ "Attribute": "DirectoryId", "Value": REGISTRATIONS[0].directory_id }],
        }),
    )
    .await;
    assert_eq!(
        foreign.status, 400,
        "a cursor from the unfiltered list is not a position in a filtered one, got {foreign:?}"
    );

    for row in REGISTRATIONS {
        deregister(
            &h,
            &json!({
                "IdentityProvider": provider_union(row.directory_id),
                "Product": row.product,
            }),
        )
        .await
        .ok(row.case);
    }
    // Deregistration is asynchronous: the registrations are still REPORTED in
    // `DEREGISTERING`, so the list only shrinks past the cursor once the teardown the
    // clock owes has completed (see `identity_provider_deregistration`).
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let stale = list_providers(&h, &json!({ "MaxResults": 2, "NextToken": token })).await;
    assert_eq!(
        stale.status, 400,
        "a cursor past the end of the list must be refused, got {stale:?}"
    );
}

/// RULE: `Filters` select on the attributes the operation's model documents
/// (`Product`/`DirectoryId` for `ListIdentityProviders`,
/// `Status`/`Username`/`Domain` for `ListProductSubscriptions`), and an attribute outside
/// that set is refused rather than ignored.
#[tokio::test]
async fn filters_select_on_the_documented_attributes() {
    let h = harness();
    for row in REGISTRATIONS {
        register(&h, &row.request()).await.ok(row.case);
    }

    // (case, filters, how many of REGISTRATIONS must match)
    let provider_cases: Vec<(&str, Value, usize)> = vec![
        (
            "Product equals",
            json!([{ "Attribute": "Product", "Operation": "Equals", "Value": PRODUCTS[0] }]),
            REGISTRATIONS
                .iter()
                .filter(|r| r.product == PRODUCTS[0])
                .count(),
        ),
        (
            "DirectoryId equals, two products on one directory",
            json!([{ "Attribute": "DirectoryId", "Value": "d-0123456789" }]),
            REGISTRATIONS
                .iter()
                .filter(|r| r.directory_id == "d-0123456789")
                .count(),
        ),
        (
            "DirectoryId begins with the shared-directory prefix",
            json!([{ "Attribute": "DirectoryId", "Operation": "BeginsWith", "Value": "sd-" }]),
            REGISTRATIONS
                .iter()
                .filter(|r| r.directory_id.starts_with("sd-"))
                .count(),
        ),
        (
            "Product contains",
            json!([{ "Attribute": "Product", "Operation": "Contains", "Value": "OFFICE" }]),
            REGISTRATIONS
                .iter()
                .filter(|r| r.product.contains("OFFICE"))
                .count(),
        ),
        (
            "Product not equals",
            json!([{ "Attribute": "Product", "Operation": "NotEquals", "Value": PRODUCTS[0] }]),
            REGISTRATIONS
                .iter()
                .filter(|r| r.product != PRODUCTS[0])
                .count(),
        ),
        (
            "two filters are an intersection",
            json!([
                { "Attribute": "DirectoryId", "Value": "d-0123456789" },
                { "Attribute": "Product", "Value": PRODUCTS[2] },
            ]),
            REGISTRATIONS
                .iter()
                .filter(|r| r.directory_id == "d-0123456789" && r.product == PRODUCTS[2])
                .count(),
        ),
        (
            "a filter nothing matches",
            json!([{ "Attribute": "DirectoryId", "Value": "d-ffffffffff" }]),
            0,
        ),
    ];
    for (case, filters, expected) in provider_cases {
        let reply = list_providers(&h, &json!({ "Filters": filters })).await;
        let body = reply.ok(case);
        let got = body["IdentityProviderSummaries"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(got, expected, "case {case:?}: wrong match count in {body:?}");
    }

    let undocumented = list_providers(
        &h,
        &json!({ "Filters": [{ "Attribute": "SecurityGroupId", "Value": "sg-0123abcd" }] }),
    )
    .await;
    assert_eq!(
        undocumented.status, 400,
        "an undocumented filter attribute must be refused, got {undocumented:?}"
    );
    let bad_operation = list_providers(
        &h,
        &json!({ "Filters": [{ "Attribute": "Product", "Operation": "like", "Value": "A" }] }),
    )
    .await;
    assert_eq!(
        bad_operation.status, 400,
        "an unsupported filter operation must be refused, got {bad_operation:?}"
    );

    // The same rule over ListProductSubscriptions' own documented attributes.
    let registration = REGISTRATIONS[0];
    for row in SUBSCRIBERS {
        let request = subscription_request(row, registration.directory_id, registration.product);
        start_subscription(&h, &request).await.ok(row.case);
    }
    let subscription_cases: Vec<(&str, Value, usize)> = vec![
        (
            "Username equals",
            json!([{ "Attribute": "Username", "Value": SUBSCRIBERS[0].username }]),
            1,
        ),
        (
            "Status equals SUBSCRIBED",
            json!([{ "Attribute": "Status", "Value": "SUBSCRIBED" }]),
            SUBSCRIBERS.len(),
        ),
        (
            "Status equals UNSUBSCRIBED",
            json!([{ "Attribute": "Status", "Value": "UNSUBSCRIBED" }]),
            0,
        ),
        (
            "Domain equals",
            json!([{ "Attribute": "Domain", "Value": "example.com" }]),
            SUBSCRIBERS
                .iter()
                .filter(|s| s.domain == Some("example.com"))
                .count(),
        ),
        (
            "Username contains",
            json!([{ "Attribute": "Username", "Operation": "Contains", "Value": "@" }]),
            SUBSCRIBERS
                .iter()
                .filter(|s| s.username.contains('@'))
                .count(),
        ),
    ];
    for (case, filters, expected) in subscription_cases {
        let reply = list_subscriptions(
            &h,
            &json!({
                "IdentityProvider": provider_union(registration.directory_id),
                "Product": registration.product,
                "Filters": filters,
            }),
        )
        .await;
        let body = reply.ok(case);
        let got = body["ProductUserSummaries"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(got, expected, "case {case:?}: wrong match count in {body:?}");
    }

    let undocumented = list_subscriptions(
        &h,
        &json!({
            "IdentityProvider": provider_union(registration.directory_id),
            "Product": registration.product,
            "Filters": [{ "Attribute": "Product", "Value": registration.product }],
        }),
    )
    .await;
    assert_eq!(
        undocumented.status, 400,
        "an undocumented filter attribute must be refused, got {undocumented:?}"
    );
}

/// RULE: whether a filter is well formed does not depend on what is in the store — an
/// attribute the operation does not expose, or a comparison this service does not
/// implement, is refused even when no record could have matched it.
#[tokio::test]
async fn a_malformed_filter_is_refused_even_with_nothing_stored() {
    let h = harness();
    let registration = REGISTRATIONS[0];
    let malformed = [
        (
            "an attribute of another list",
            json!([{ "Attribute": "Username", "Value": "alice" }]),
            json!([{ "Attribute": "DirectoryId", "Value": registration.directory_id }]),
        ),
        (
            "an attribute no list exposes",
            json!([{ "Attribute": "Nonsense", "Value": "x" }]),
            json!([{ "Attribute": "Nonsense", "Value": "x" }]),
        ),
        (
            "a documented attribute misspelled in the wrong case",
            json!([{ "Attribute": "product", "Value": "x" }]),
            json!([{ "Attribute": "status", "Value": "x" }]),
        ),
        (
            "an unsupported comparison",
            json!([{ "Attribute": "Product", "Operation": "like", "Value": "x" }]),
            json!([{ "Attribute": "Status", "Operation": "like", "Value": "x" }]),
        ),
        (
            "no attribute at all",
            json!([{ "Value": "x" }]),
            json!([{ "Value": "x" }]),
        ),
    ];
    for (case, provider_filters, subscription_filters) in malformed {
        let listed = list_providers(&h, &json!({ "Filters": provider_filters })).await;
        assert_eq!(
            listed.status, 400,
            "case {case:?}: ListIdentityProviders must refuse it with nothing stored, got {listed:?}"
        );
        let listed = list_subscriptions(
            &h,
            &json!({
                "IdentityProvider": provider_union(registration.directory_id),
                "Product": registration.product,
                "Filters": subscription_filters,
            }),
        )
        .await;
        assert_eq!(
            listed.status, 400,
            "case {case:?}: ListProductSubscriptions must refuse it with nothing stored, got {listed:?}"
        );
    }
}

/// RULE: a resource returned in a non-terminal status carries a transition the clock
/// owes — a read before it falls due still sees the transient status, and a read after it
/// sees the settled one.
#[tokio::test]
async fn the_registering_status_settles_once_the_clock_owes_it() {
    let h = harness();
    let row = REGISTRATIONS[0];
    register(&h, &row.request()).await.ok(row.case);

    let immediate = list_providers(&h, &json!({})).await;
    assert_eq!(
        as_str(
            &immediate.ok("immediate read")["IdentityProviderSummaries"][0],
            "Status",
            "immediate read"
        ),
        "REGISTERING",
        "a caller reading straight after the call still sees the transient status"
    );

    tokio::time::sleep(std::time::Duration::from_millis(900)).await;

    let settled = list_providers(&h, &json!({})).await;
    assert_eq!(
        as_str(
            &settled.ok("settled read")["IdentityProviderSummaries"][0],
            "Status",
            "settled read"
        ),
        "REGISTERED",
        "the transition the clock owed must be materialised on a later read"
    );
}

/// RULE: a deregistration ACCEPTS an asynchronous teardown — it answers the non-terminal
/// `DEREGISTERING`, takes the product users that were only reachable through the
/// registration at once, and stops reporting the registration once the teardown the clock
/// owes has completed; a second deregistration names an identifier that is no longer one
/// of this account's identity providers. The observability of `DEREGISTERING` itself is
/// pinned by `identity_provider_deregistration`.
#[tokio::test]
async fn deregistration_removes_the_registration_and_its_product_users() {
    let h = harness();
    let mut arns = Vec::new();
    for row in REGISTRATIONS {
        let reply = register(&h, &row.request()).await;
        arns.push(
            as_str(
                &reply.ok(row.case)["IdentityProviderSummary"],
                "IdentityProviderArn",
                row.case,
            )
            .to_string(),
        );
    }
    let first = REGISTRATIONS[0];
    for row in SUBSCRIBERS {
        start_subscription(
            &h,
            &subscription_request(row, first.directory_id, first.product),
        )
        .await
        .ok(row.case);
    }

    // Half are addressed by ARN and half by (IdentityProvider, Product): the model lets a
    // caller name a registration either way.
    for (index, row) in REGISTRATIONS.iter().enumerate() {
        let by_arn = index % 2 == 0;
        let request = if by_arn {
            json!({ "IdentityProviderArn": arns[index] })
        } else {
            json!({
                "IdentityProvider": provider_union(row.directory_id),
                "Product": row.product,
            })
        };
        let reply = deregister(&h, &request).await;
        let summary = reply.ok(row.case)["IdentityProviderSummary"].clone();
        assert_eq!(
            as_str(&summary, "Status", row.case),
            "DEREGISTERING",
            "case {:?}: a deregistered registration answers DEREGISTERING, got {summary:?}",
            row.case
        );
        assert_eq!(
            as_str(&summary, "IdentityProviderArn", row.case),
            arns[index].as_str(),
            "case {:?}: the final summary must be the stored one, got {summary:?}",
            row.case
        );
        // A client reading straight after the call still sees the registration, in the
        // transient status the call announced.
        let listed = list_providers(&h, &json!({})).await;
        let body = listed.ok(row.case);
        let reported = body["IdentityProviderSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no summaries in {body:?}", row.case))
            .iter()
            .find(|summary| {
                summary.get("IdentityProviderArn").and_then(Value::as_str)
                    == Some(arns[index].as_str())
            })
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: the list must still report {} while it is deregistering, \
                     got {body:?}",
                    row.case, arns[index]
                )
            });
        assert_eq!(
            as_str(&reported, "Status", row.case),
            "DEREGISTERING",
            "case {:?}: the list reports the stored status, got {reported:?}",
            row.case
        );

        let again = deregister(&h, &request).await;
        if by_arn {
            // The ARN names no registration of this account any more: an invalid parameter.
            assert_eq!(
                again.status, 400,
                "case {:?}: a second deregistration names an ARN that is no longer an \
                 identity provider of this account, got {again:?}",
                row.case
            );
            assert_eq!(
                again.error_type.as_deref(),
                Some("ValidationException"),
                "case {:?}: expected ValidationException, got {again:?}",
                row.case
            );
            assert_eq!(
                again.body.get("message").and_then(Value::as_str),
                Some("Identity provider is not valid"),
                "case {:?}: expected the service's own sentence for an ARN that names \
                 no registration, got {again:?}",
                row.case
            );
        } else {
            // The request named the identity provider ITSELF, which this account has no
            // registration under any more: the fail-closed refusal.
            assert_eq!(
                again.status, 401,
                "case {:?}: a pair that resolves to nothing fails closed, got {again:?}",
                row.case
            );
            assert_eq!(
                again.error_type.as_deref(),
                Some("UnauthorizedException"),
                "case {:?}: expected UnauthorizedException, got {again:?}",
                row.case
            );
            assert_eq!(
                again.body.get("message").and_then(Value::as_str),
                Some(UNAUTHORIZED_MESSAGE),
                "case {:?}: expected the fixed fail-closed sentence, got {again:?}",
                row.case
            );
        }
    }

    let listed = list_providers(&h, &json!({})).await;
    let still_reported = listed.ok("everything deregistering")["IdentityProviderSummaries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert!(
        still_reported > 0,
        "a deregistering provider is still reported until the teardown completes, got {:?}",
        listed.body
    );
    // Its product users went at once: the provider they hung off is no longer registered.
    let orphaned = list_subscriptions(
        &h,
        &json!({
            "IdentityProvider": provider_union(first.directory_id),
            "Product": first.product,
        }),
    )
    .await;
    assert_eq!(
        orphaned.status, 401,
        "listing through a provider that is no longer registered names the identity \
         provider itself, which fails closed, got {orphaned:?}"
    );
    assert_eq!(
        orphaned.error_type.as_deref(),
        Some("UnauthorizedException"),
        "listing through a deregistered provider must fail closed, got {orphaned:?}"
    );
    {
        let state = h
            .engine
            .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
        let guard = state.lock();
        assert!(
            guard.product_users.is_empty() && guard.product_user_index.is_empty(),
            "no product user may outlive its identity provider: {:?}",
            guard.product_users
        );
    }

    // Once the clock owes the teardown's completion, the registration stops being
    // reported and every index drops it.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let listed = list_providers(&h, &json!({})).await;
    assert_eq!(
        listed.ok("everything deregistered")["IdentityProviderSummaries"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "a deregistered provider must be gone from the list once the teardown completed"
    );
    let state = h
        .engine
        .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
    let guard = state.lock();
    assert!(
        guard.product_users.is_empty() && guard.product_user_index.is_empty(),
        "no product user may outlive its identity provider: {:?}",
        guard.product_users
    );
    assert!(
        guard.identity_providers.is_empty() && guard.identity_provider_index.is_empty(),
        "every index must drop the removed registration: {:?}",
        guard.identity_providers
    );
}

/// RULE: a minted ARN is RECORDED, not just returned — the store holds the resource under
/// the ARN the caller was given, together with the tags that were supplied with it.
#[tokio::test]
async fn minted_arns_and_tags_are_recorded_in_the_store() {
    let h = harness();
    for row in REGISTRATIONS {
        let reply = register(&h, &row.request()).await;
        let arn = as_str(
            &reply.ok(row.case)["IdentityProviderSummary"],
            "IdentityProviderArn",
            row.case,
        )
        .to_string();

        let state = h
            .engine
            .state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>();
        let guard = state.lock();
        let record = guard
            .identity_providers
            .get(&arn)
            .unwrap_or_else(|| panic!("case {:?}: {arn} was returned but not stored", row.case));
        assert_eq!(
            record.product, row.product,
            "case {:?}: the stored record must carry the registered product",
            row.case
        );
        assert_eq!(
            record.settings.subnets,
            row.subnets.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "case {:?}: the stored record must carry the registered subnets",
            row.case
        );
        let tags = guard.tags.get(&arn);
        if row.tagged {
            assert_eq!(
                tags.and_then(|tags| tags.get("team")).map(String::as_str),
                Some("licensing"),
                "case {:?}: the supplied tags must be stored against the minted ARN",
                row.case
            );
        } else {
            assert!(
                tags.is_none(),
                "case {:?}: no tags were supplied, so none may appear: {tags:?}",
                row.case
            );
        }
    }
}
