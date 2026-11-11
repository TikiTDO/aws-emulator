// SPDX-License-Identifier: CC0-1.0

//! The credentials secret a request names is resolved against the secrets the ACCOUNT
//! holds, asked of the service that owns secrets.
//!
//! The rule under test: a `CredentialsProvider` a request supplies names a Secrets Manager
//! secret of the caller's own account — `SecretsManagerCredentialsProvider$SecretId` is
//! "the ID of the Secrets Manager secret that contains credentials" and this service
//! creates no secret — so a request is served exactly when the account holds a secret of
//! that id whose credentials can still be retrieved, and one naming a secret the account
//! has none of, or one it has marked for deletion, is refused with the modelled
//! `ValidationException` (HTTP 400) and persists nothing.
//!
//! The rule is asserted on BOTH members that carry such a reference
//! (`smithy_query(query="shape", shape="CredentialsProvider")` has exactly two referrers):
//!
//! * `CreateLicenseServerEndpoint`'s
//!   `LicenseServerSettings.ServerSettings.RdsSalSettings.RdsSalCredentialsProvider`
//!   (`@required`), "the credentials provider that's used for RDS license server user
//!   administration";
//! * `RegisterIdentityProvider`'s
//!   `IdentityProvider.ActiveDirectoryIdentityProvider.ActiveDirectorySettings.DomainCredentialsProvider`,
//!   "the credential provider for user administration" of a self-managed Active Directory.
//!
//! A secret is Secrets Manager's resource, so Secrets Manager is mounted beside this
//! service on ONE engine and driven through the AWS SDK over the same in-process callback
//! this service's own resolution reaches it through: every secret a row names was created
//! by `secretsmanager:CreateSecret` itself, and the ones a row deletes were deleted through
//! `secretsmanager:DeleteSecret`. Nothing here reaches into either store.
//!
//! The rows differ along what the rule generalises over: how the held secret is ADDRESSED
//! (its name, a name with slashes, the full ARN Secrets Manager minted, the partial ARN a
//! client that never saw the full one can build), what the secret holds (a string value, a
//! binary value), and the way a reference fails to resolve (a name no secret has, a near
//! miss, a case difference, another Region's ARN, an ARN with other trailing characters, a
//! secret scheduled for deletion, and one deleted without recovery and already purged).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::EmulatedService;

// ONE wire harness per service: the dispatch plumbing, the account's directory and network
// fleets and the reply reader come from the sibling suites rather than a second copy.
use crate::identity_provider_subscriptions::{
    as_str, call, create_directories, create_network, provider_union, register, Harness, Reply,
    ACCOUNT_SECURITY_GROUPS, ACCOUNT_SUBNETS, ACCOUNT_VPC, PAST_THE_OWED_TRANSITION, PRODUCTS,
    REGION,
};
use crate::license_server_endpoints::{create_endpoint, list_endpoints, SERVER_TYPE};

/// The AWS-managed directory the endpoint rows' registrations are for.
const DIRECTORY: &str = "d-0123456789";

/// The domain a self-managed Active Directory row registers, which is what identifies that
/// registration (`ActiveDirectorySettings.DomainName`, no `DirectoryId`).
const DOMAIN: &str = "credentials.example";

/// The two sentences a refused reference is stated in, spelled out here rather than read
/// from the production constants so a change to them fails a test instead of silently
/// changing what this suite accepts. Both are the wording the service that OWNS secrets
/// answers the same fact with.
const SECRET_NOT_FOUND: &str = "Secrets Manager can't find the specified secret.";
const SECRET_MARKED_FOR_DELETION: &str =
    "You can't perform this operation on the secret because it was marked for deletion.";

/// The `LicenseServerEndpointProvisioningStatus` an accepted endpoint is answered in, and
/// the terminal one it settles into once the clock owes the transition. Spelled out for the
/// same reason.
const PROVISIONING: &str = "PROVISIONING";
const PROVISIONED: &str = "PROVISIONED";

/// This service, Amazon EC2 (the owner of the network a registration names) and Secrets
/// Manager (the owner of the credentials secret) over ONE shared engine, plus a Secrets
/// Manager client over the router's own in-process callback: the very seam this service's
/// credentials resolution reaches Secrets Manager through.
struct Vault {
    h: Harness,
    secrets: ::aws_sdk_secretsmanager::Client,
}

impl Vault {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The tests sign nothing; authentication is the framework's concern, not this
        // service's.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        // A registration resolves the directory it is for and the network its `Settings`
        // name, so the account holds both before any row runs.
        create_directories(&engine, &[DIRECTORY]);
        create_network(
            &engine,
            ACCOUNT_VPC,
            ACCOUNT_SUBNETS,
            ACCOUNT_SECURITY_GROUPS,
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        let config = ::aws_sdk_secretsmanager::Config::builder()
            .behavior_version(::aws_sdk_secretsmanager::config::BehaviorVersion::latest())
            .region(::aws_sdk_secretsmanager::config::Region::new(
                REGION.to_string(),
            ))
            .endpoint_url(format!("https://secretsmanager.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(::aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_secretsmanager::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        Self {
            h: Harness { router, engine },
            secrets: ::aws_sdk_secretsmanager::Client::from_conf(config),
        }
    }

    /// Create the secret `name` through Secrets Manager itself and answer the ARN it
    /// minted. `binary` holds the administrator credentials as a blob instead of a string,
    /// which is the other kind of value a secret can hold.
    async fn create_secret(&self, case: &str, name: &str, binary: bool) -> String {
        let request = self.secrets.create_secret().name(name);
        let request = if binary {
            request.secret_binary(::aws_smithy_types::Blob::new(
                b"{\"username\":\"admin\"}".to_vec(),
            ))
        } else {
            request.secret_string("{\"username\":\"admin\",\"password\":\"p\"}")
        };
        let created = request.send().await.unwrap_or_else(|error| {
            panic!("case {case:?}: Secrets Manager must create {name}: {error:?}")
        });
        created
            .arn()
            .unwrap_or_else(|| panic!("case {case:?}: CreateSecret must report the ARN it minted"))
            .to_string()
    }

    /// Delete the secret `secret_id` names through Secrets Manager itself: scheduled with a
    /// recovery window, or without recovery when `force` — the two deletions the owner
    /// models.
    async fn delete_secret(&self, case: &str, secret_id: &str, force: bool) {
        let request = self.secrets.delete_secret().secret_id(secret_id);
        // "The number of days from 7 to 30 that Secrets Manager waits before permanently
        // deleting the secret" (`DeleteSecretRequest$RecoveryWindowInDays`): the published
        // minimum, so the secret stays marked for deletion for the whole run.
        let request = if force {
            request.force_delete_without_recovery(true)
        } else {
            request.recovery_window_in_days(7)
        };
        request.send().await.unwrap_or_else(|error| {
            panic!("case {case:?}: Secrets Manager must delete {secret_id}: {error:?}")
        });
    }
}

/// How a row addresses the secret the account holds, or fails to.
#[derive(Debug, Clone, Copy)]
enum Named {
    /// The name the secret was created under.
    Name,
    /// The full ARN Secrets Manager minted for it.
    Arn,
    /// That ARN without the trailing hyphen and six characters — the partial ARN a client
    /// that never saw the full one can still build.
    PartialArn,
    /// That ARN with other trailing characters in place of the six it ends in.
    ArnOtherSuffix,
    /// The same resource name under another Region's ARN: a `SecretId` resolves only
    /// within the Region the request was addressed to.
    ArnElsewhere,
    /// The held name with one more character: a near miss.
    NearMiss,
    /// The held name in upper case.
    UpperCase,
    /// A name of the account's own form that no secret was created under.
    Absent(&'static str),
}

/// What the row did to the secret it created, if anything.
#[derive(Debug, Clone, Copy)]
enum Disposition {
    /// It is still held.
    Held,
    /// Its deletion was scheduled with a recovery window, so it is marked for deletion.
    ScheduledForDeletion,
    /// It was deleted without recovery, and the row waits for the purge the owner owes.
    Purged,
}

/// One reference the rule is asserted on, over both members that carry one.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The name the row's secret is created under; `None` for a row that creates none.
    secret_name: Option<&'static str>,
    /// Whether that secret holds a binary value rather than a string one.
    binary: bool,
    disposition: Disposition,
    named: Named,
    /// The sentence the refusal must state, or `None` for a reference that resolves.
    refusal: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "the name the secret was created under",
        secret_name: Some("rds-sal-admin"),
        binary: false,
        disposition: Disposition::Held,
        named: Named::Name,
        refusal: None,
    },
    Row {
        case: "a secret name carrying slashes",
        secret_name: Some("rds/sal/office/admin"),
        binary: false,
        disposition: Disposition::Held,
        named: Named::Name,
        refusal: None,
    },
    Row {
        case: "the full ARN Secrets Manager minted",
        secret_name: Some("rds-sal-by-arn"),
        binary: false,
        disposition: Disposition::Held,
        named: Named::Arn,
        refusal: None,
    },
    Row {
        case: "the partial ARN of the held secret",
        secret_name: Some("rds-sal-by-partial-arn"),
        binary: false,
        disposition: Disposition::Held,
        named: Named::PartialArn,
        refusal: None,
    },
    Row {
        case: "a secret holding a binary value",
        secret_name: Some("rds-sal-binary"),
        binary: true,
        disposition: Disposition::Held,
        named: Named::Name,
        refusal: None,
    },
    Row {
        case: "a name no secret of the account was created under",
        secret_name: None,
        binary: false,
        disposition: Disposition::Held,
        named: Named::Absent("no-such-secret"),
        refusal: Some(SECRET_NOT_FOUND),
    },
    Row {
        case: "a name of the ARN form that names no secret",
        secret_name: None,
        binary: false,
        disposition: Disposition::Held,
        named: Named::Absent(
            "arn:aws:secretsmanager:us-west-2:000000000000:secret:never-created-AbCdEf",
        ),
        refusal: Some(SECRET_NOT_FOUND),
    },
    Row {
        case: "a near miss on the held secret's name",
        secret_name: Some("rds-sal-near"),
        binary: false,
        disposition: Disposition::Held,
        named: Named::NearMiss,
        refusal: Some(SECRET_NOT_FOUND),
    },
    Row {
        case: "an upper-case spelling of the held secret's name",
        secret_name: Some("rds-sal-case"),
        binary: false,
        disposition: Disposition::Held,
        named: Named::UpperCase,
        refusal: Some(SECRET_NOT_FOUND),
    },
    Row {
        case: "the held secret's ARN with other trailing characters",
        secret_name: Some("rds-sal-suffix"),
        binary: false,
        disposition: Disposition::Held,
        named: Named::ArnOtherSuffix,
        refusal: Some(SECRET_NOT_FOUND),
    },
    Row {
        case: "the held secret's name under another Region's ARN",
        secret_name: Some("rds-sal-region"),
        binary: false,
        disposition: Disposition::Held,
        named: Named::ArnElsewhere,
        refusal: Some(SECRET_NOT_FOUND),
    },
    Row {
        case: "a secret whose deletion was scheduled with a recovery window",
        secret_name: Some("rds-sal-scheduled"),
        binary: false,
        disposition: Disposition::ScheduledForDeletion,
        named: Named::Name,
        refusal: Some(SECRET_MARKED_FOR_DELETION),
    },
    Row {
        case: "a secret deleted without recovery and already purged",
        secret_name: Some("rds-sal-purged"),
        binary: false,
        disposition: Disposition::Purged,
        named: Named::Name,
        refusal: Some(SECRET_NOT_FOUND),
    },
    Row {
        case: "a secret deleted without recovery and already purged, by ARN",
        secret_name: Some("rds-sal-purged-arn"),
        binary: false,
        disposition: Disposition::Purged,
        named: Named::Arn,
        refusal: Some(SECRET_NOT_FOUND),
    },
];

/// The partial ARN of `arn`: the ARN without the trailing hyphen and the six characters
/// Secrets Manager appends to it.
fn partial_arn(case: &str, arn: &str) -> String {
    let (head, suffix) = arn
        .rsplit_once('-')
        .unwrap_or_else(|| panic!("case {case:?}: a minted secret ARN ends in -<six characters>"));
    assert_eq!(
        suffix.len(),
        6,
        "case {case:?}: expected six trailing characters on {arn}, got {suffix:?}"
    );
    head.to_string()
}

/// Bring the row's secret into the state it describes and answer the `SecretId` its request
/// names.
async fn secret_id(vault: &Vault, row: &Row) -> String {
    let arn = match row.secret_name {
        Some(name) => Some(vault.create_secret(row.case, name, row.binary).await),
        None => None,
    };
    match row.disposition {
        Disposition::Held => {}
        Disposition::ScheduledForDeletion => {
            let name = row.secret_name.expect("a disposition row creates a secret");
            vault.delete_secret(row.case, name, false).await;
        }
        Disposition::Purged => {
            let name = row.secret_name.expect("a disposition row creates a secret");
            vault.delete_secret(row.case, name, true).await;
            // A deletion without recovery skips the recovery window, and the owner purges
            // the record a short span later: the row waits for the purge the clock owes
            // rather than for a human timescale.
            tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        }
    }
    let name = row.secret_name.unwrap_or_default();
    match row.named {
        Named::Name => name.to_string(),
        Named::Arn => arn.expect("an ARN row creates a secret"),
        Named::PartialArn => partial_arn(row.case, &arn.expect("an ARN row creates a secret")),
        Named::ArnOtherSuffix => {
            format!(
                "{}-ZzZzZz",
                partial_arn(row.case, &arn.expect("an ARN row creates a secret"))
            )
        }
        Named::ArnElsewhere => arn
            .expect("an ARN row creates a secret")
            .replace(REGION, "us-east-1"),
        Named::NearMiss => format!("{name}x"),
        Named::UpperCase => name.to_ascii_uppercase(),
        Named::Absent(named) => named.to_string(),
    }
}

/// Assert `reply` is the modelled `ValidationException` (HTTP 400) stating `message`.
fn assert_refused(reply: &Reply, message: &str, case: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: ValidationException is modelled at HTTP 400, got {reply:?}"
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected ValidationException, got {reply:?}"
    );
    assert_eq!(
        reply.message(),
        Some(message),
        "case {case:?}: wrong refusal sentence in {reply:?}"
    );
}

/// The registration an endpoint row is created for, and the ARN it minted.
async fn registration_arn(vault: &Vault, case: &str) -> String {
    let registered = register(
        &vault.h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": PRODUCTS[4],
            "Settings": {
                "Subnets": [ACCOUNT_SUBNETS[0]],
                "SecurityGroupId": ACCOUNT_SECURITY_GROUPS[0],
            },
        }),
    )
    .await;
    as_str(
        &registered.ok(case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        case,
    )
    .to_string()
}

/// The `CreateLicenseServerEndpoint` request naming `secret_id` as its RDS SAL credentials
/// provider.
fn endpoint_request(identity_provider_arn: &str, secret_id: &str) -> Value {
    json!({
        "IdentityProviderArn": identity_provider_arn,
        "LicenseServerSettings": {
            "ServerType": SERVER_TYPE,
            "ServerSettings": {
                "RdsSalSettings": {
                    "RdsSalCredentialsProvider": {
                        "SecretsManagerCredentialsProvider": { "SecretId": secret_id },
                    },
                },
            },
        },
    })
}

/// The `RegisterIdentityProvider` request whose self-managed Active Directory names
/// `secret_id` as its domain credentials provider.
fn self_managed_registration(secret_id: &str) -> Value {
    json!({
        "IdentityProvider": {
            "ActiveDirectoryIdentityProvider": {
                "ActiveDirectorySettings": {
                    "DomainName": DOMAIN,
                    // `ActiveDirectorySettings$DomainIpv4List` (@length(min: 1, max: 2)).
                    "DomainIpv4List": ["10.200.0.10"],
                    "DomainCredentialsProvider": {
                        "SecretsManagerCredentialsProvider": { "SecretId": secret_id },
                    },
                    "DomainNetworkSettings": { "Subnets": [ACCOUNT_SUBNETS[1]] },
                },
                "ActiveDirectoryType": "SELF_MANAGED",
            },
        },
        "Product": PRODUCTS[4],
    })
}

/// Every endpoint the account holds, as its `(ARN, ProvisioningStatus)` pairs.
async fn listed_endpoints(h: &Harness, case: &str) -> Vec<(String, String)> {
    let listed = list_endpoints(h, &json!({})).await;
    let body = listed.ok(case);
    body.get("LicenseServerEndpoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no LicenseServerEndpoints in {body:?}"))
        .iter()
        .map(|endpoint| {
            (
                as_str(endpoint, "LicenseServerEndpointArn", case).to_string(),
                as_str(endpoint, "LicenseServerEndpointProvisioningStatus", case).to_string(),
            )
        })
        .collect()
}

/// RULE, on `CreateLicenseServerEndpoint`: an endpoint is created exactly when the account
/// holds the secret its `RdsSalCredentialsProvider` names, whichever way that secret is
/// addressed — and the created endpoint is a healthy one, settling into the terminal
/// `PROVISIONED` the clock owes it. A reference the account holds no retrievable secret for
/// is refused with the modelled `ValidationException` stating what the owner of secrets
/// states, and persists nothing.
#[tokio::test]
async fn a_license_server_endpoint_resolves_the_credentials_secret_it_names() {
    for row in ROWS {
        // A fresh emulator per row, so what the account holds is exactly this row's.
        let vault = Vault::new();
        let provider_arn = registration_arn(&vault, row.case).await;
        let secret_id = secret_id(&vault, row).await;

        let reply = create_endpoint(&vault.h, &endpoint_request(&provider_arn, &secret_id)).await;
        match row.refusal {
            Some(message) => {
                assert_refused(&reply, message, row.case);
                assert_eq!(
                    listed_endpoints(&vault.h, row.case).await,
                    Vec::new(),
                    "case {:?}: a refused create must persist nothing",
                    row.case
                );
                // The control: the same request succeeds for a secret the account does hold,
                // so the refusal was about the reference and nothing else.
                let held = vault.create_secret(row.case, "rds-sal-control", false).await;
                create_endpoint(&vault.h, &endpoint_request(&provider_arn, &held))
                    .await
                    .ok(row.case);
            }
            None => {
                let body = reply.ok(row.case);
                let endpoint_arn = as_str(body, "LicenseServerEndpointArn", row.case).to_string();
                let listed = listed_endpoints(&vault.h, row.case).await;
                assert_eq!(
                    listed,
                    vec![(endpoint_arn.clone(), PROVISIONING.to_string())],
                    "case {:?}: the created endpoint must be the one reported, in \
                     {PROVISIONING}",
                    row.case
                );
                // Past the transition the clock owes it, an endpoint whose credentials
                // secret resolved settles into the terminal status, with nothing to report
                // as a failure.
                tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
                assert_eq!(
                    listed_endpoints(&vault.h, row.case).await,
                    vec![(endpoint_arn, PROVISIONED.to_string())],
                    "case {:?}: expected the endpoint to settle {PROVISIONED}",
                    row.case
                );
            }
        }
    }
}

/// RULE, on `RegisterIdentityProvider`: the same existence rule decides the other member
/// that carries a `CredentialsProvider` — a self-managed Active Directory's
/// `DomainCredentialsProvider` — so a registration naming a secret the account holds is
/// stored and reported, and one naming a secret it has none of is refused with the same
/// sentence and persists nothing.
#[tokio::test]
async fn a_registration_resolves_the_domain_credentials_secret_it_names() {
    for row in ROWS {
        let vault = Vault::new();
        let secret_id = secret_id(&vault, row).await;

        let reply = register(&vault.h, &self_managed_registration(&secret_id)).await;
        let listed = call(
            &vault.h,
            "/identity-provider/ListIdentityProviders",
            &json!({}),
        )
        .await;
        let body = listed.ok(row.case);
        let summaries = body
            .get("IdentityProviderSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {:?}: no summaries in {body:?}", row.case))
            .clone();

        match row.refusal {
            Some(message) => {
                assert_refused(&reply, message, row.case);
                assert!(
                    summaries.is_empty(),
                    "case {:?}: a refused registration must persist nothing, got {summaries:?}",
                    row.case
                );
            }
            None => {
                let summary = &reply.ok(row.case)["IdentityProviderSummary"];
                let provider_arn = as_str(summary, "IdentityProviderArn", row.case).to_string();
                assert_eq!(
                    summaries.len(),
                    1,
                    "case {:?}: the accepted registration must be the one reported, got \
                     {summaries:?}",
                    row.case
                );
                assert_eq!(
                    as_str(&summaries[0], "IdentityProviderArn", row.case),
                    provider_arn,
                    "case {:?}: a later read must report the registration that was stored, got \
                     {summaries:?}",
                    row.case
                );
            }
        }
    }
}

/// RULE, the other half of the resolution: a `CredentialsProvider` that names NO secret
/// resolves nothing, so a request carrying one is decided by the rest of its own rules.
/// `SecretsManagerCredentialsProvider$SecretId` is not `@required`
/// (`smithy_query(query="shape", shape="SecretsManagerCredentialsProvider")`), so such a
/// request is one a client can send.
#[tokio::test]
async fn a_credentials_provider_naming_no_secret_resolves_nothing() {
    let vault = Vault::new();
    let provider_arn = registration_arn(&vault, "no secret named").await;

    let reply = create_endpoint(
        &vault.h,
        &json!({
            "IdentityProviderArn": provider_arn,
            "LicenseServerSettings": {
                "ServerType": SERVER_TYPE,
                "ServerSettings": {
                    "RdsSalSettings": {
                        "RdsSalCredentialsProvider": {
                            "SecretsManagerCredentialsProvider": {},
                        },
                    },
                },
            },
        }),
    )
    .await;
    let body = reply.ok("no secret named");
    let endpoint_arn = as_str(body, "LicenseServerEndpointArn", "no secret named").to_string();
    assert_eq!(
        listed_endpoints(&vault.h, "no secret named").await,
        vec![(endpoint_arn, PROVISIONING.to_string())],
        "an endpoint whose credentials provider names no secret is created like any other"
    );
}
