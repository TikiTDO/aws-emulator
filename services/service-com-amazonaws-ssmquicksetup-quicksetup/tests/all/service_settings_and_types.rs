// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for this service's two account-level reads and the update
//! that feeds one of them: `ListQuickSetupTypes`, `GetServiceSettings` and
//! `UpdateServiceSettings`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what
//! is asserted is what a signed AWS SDK request observes.
//!
//! Two rules are under test:
//!
//! * `ListQuickSetupTypes` answers the service's whole configuration-type catalog —
//!   one `Type` / `LatestVersion` pair per type it offers — and that answer depends on
//!   nothing in the request and nothing in the store.
//! * Quick Setup's service settings are persisted per account and Region: a read
//!   reports exactly what the last accepted update wrote for the Region the request
//!   was signed for, every read carries the `ServiceSettings` structure (a Region no
//!   update has configured carries it with `ExplorerEnablingRoleArn` unset), and a
//!   read changes nothing a later caller sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::QUICK_SETUP_TYPE_CATALOG;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
}

/// A request signed for `ssm-quicksetup` in `region` — the signal the router routes
/// on and the handlers read the Region from (the SigV4 credential scope).
fn request(
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{region}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, region, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

async fn list_types(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call(router, "GET", "/listQuickSetupTypes", region, None).await
}

async fn get_settings(router: &MultiServiceRouter, region: &str) -> (u16, Value) {
    call(router, "GET", "/serviceSettings", region, None).await
}

async fn update_settings(
    router: &MultiServiceRouter,
    region: &str,
    body: &Value,
) -> (u16, Value) {
    call(router, "PUT", "/serviceSettings", region, Some(body)).await
}

/// Create one configuration manager, so a case can assert the catalog does not
/// depend on what the store holds.
async fn create_manager(router: &MultiServiceRouter, region: &str, type_: &str) {
    let (status, answer) = call(
        router,
        "POST",
        "/configurationManager",
        region,
        Some(&json!({
            "Name": "catalog-probe",
            "ConfigurationDefinitions": [{
                "Type": type_,
                // Every accepted definition's parameters name somewhere to deploy to
                // and the Regions to deploy into, and provide every parameter the table
                // documented for its own type marks `(Required)`.
                "Parameters": super::type_parameter_fixtures::with_required_parameters(
                    type_,
                    json!({
                        "TargetOrganizationalUnits": "ou-1",
                        "TargetRegions": "us-east-1",
                    }),
                ),
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
}

/// The `Type` of a catalog entry satisfies the modelled
/// `ConfigurationDefinitionInput$Type` pattern `^[a-zA-Z0-9_\-.:/]{3,200}$` — the one
/// `smithy_query(query="shape", shape="ConfigurationDefinitionInput")` reports — so a
/// client can hand any published type straight back to a create.
fn assert_type_pattern(case: &str, type_: &str) {
    assert!(
        (3..=200).contains(&type_.len())
            && type_.bytes().all(|b| {
                b.is_ascii_alphanumeric() || b"_-.:/".contains(&b)
            }),
        "case {case}: expected a Type matching ^[a-zA-Z0-9_\\-.:/]{{3,200}}$, got {type_:?}"
    );
}

/// The `LatestVersion` of a catalog entry satisfies the modelled
/// `ConfigurationDefinition$TypeVersion` `@length(1, 128)`, the only constraint the
/// model puts on a type version.
fn assert_version_length(case: &str, version: &str) {
    assert!(
        (1..=128).contains(&version.chars().count()),
        "case {case}: expected a LatestVersion of 1..=128 characters, got {version:?}"
    );
}

/// The `(Type, LatestVersion)` pairs a `ListQuickSetupTypes` answer carries, in the
/// order it reported them.
fn reported_catalog(case: &str, answer: &Value) -> Vec<(String, String)> {
    answer
        .get("QuickSetupTypeList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: expected a QuickSetupTypeList, got {answer:?}")
        })
        .iter()
        .map(|entry| {
            let type_ = entry
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: an entry must carry a Type, got {entry:?}"))
                .to_string();
            let version = entry
                .get("LatestVersion")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case}: entry {type_:?} must carry a LatestVersion, got {entry:?}")
                })
                .to_string();
            (type_, version)
        })
        .collect()
}

/// The role ARN a `GetServiceSettings` answer reports, or `None` when its
/// `ServiceSettings` structure names none.
fn reported_role_arn(answer: &Value) -> Option<String> {
    answer
        .get("ServiceSettings")
        .filter(|settings| !settings.is_null())
        .and_then(|settings| settings.get("ExplorerEnablingRoleArn"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `ServiceSettings` structure a `GetServiceSettings` answer carries.
///
/// `GetServiceSettings` binds no not-found and no validation error
/// (`smithy_query(query="operation", operation="GetServiceSettings")` reports only
/// AccessDenied/Conflict/InternalServer/Throttling), so the structure is part of
/// every answer and this panics rather than returning an absence.
fn reported_settings(case: &str, answer: &Value) -> Value {
    let settings = answer.get("ServiceSettings").cloned().unwrap_or(Value::Null);
    assert!(
        settings.is_object(),
        "case {case}: expected a ServiceSettings structure in every answer, got {answer:?}"
    );
    settings
}

// ---------------------------------------------------------------------------
// ListQuickSetupTypes: the whole catalog, independent of request and store.
// ---------------------------------------------------------------------------

/// Every call reports the service's full type catalog: a non-empty list of
/// `Type` / `LatestVersion` pairs with no duplicate type, each satisfying the
/// modelled constraints on the members a client copies them into. The rows differ
/// along what the rule generalises over — the signing Region and what the store
/// holds — because neither takes part in the answer.
#[tokio::test]
async fn the_quick_setup_type_catalog_is_reported_in_full_whatever_the_request_and_store_hold() {
    let router = emulator();

    let expected: Vec<(String, String)> = QUICK_SETUP_TYPE_CATALOG
        .iter()
        .map(|entry| (entry.type_.to_string(), entry.latest_version().to_string()))
        .collect();
    assert!(
        !expected.is_empty(),
        "the catalog this service publishes must not be empty"
    );
    // A total, stable order, so paging-free as it is, two reads cannot disagree.
    let mut sorted = expected.clone();
    sorted.sort();
    assert_eq!(
        expected, sorted,
        "the published catalog must be in a stable sorted order, got {expected:?}"
    );

    // Each row is the state of the emulator when the catalog is asked for, and the
    // Region it is asked in.
    let rows: [(&str, &str); 4] = [
        ("an empty store, us-west-2", "us-west-2"),
        ("after a configuration manager was created, eu-west-1", "eu-west-1"),
        ("after service settings were configured, ap-south-1", "ap-south-1"),
        ("a second read in a Region already asked, us-west-2", "us-west-2"),
    ];

    for (index, (case, region)) in rows.iter().enumerate() {
        match index {
            1 => create_manager(&router, region, "AWSQuickSetupType-SSMHostMgmt").await,
            2 => {
                let (status, answer) = update_settings(
                    &router,
                    region,
                    &json!({ "ExplorerEnablingRoleArn": "arn:aws:iam::000000000000:role/Explorer" }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: UpdateServiceSettings must succeed, got {status} / {answer:?}"
                );
            }
            _ => {}
        }

        let (status, answer) = list_types(&router, region).await;
        assert_eq!(
            status, 200,
            "case {case}: ListQuickSetupTypes must answer 200, got {status} / {answer:?}"
        );
        let reported = reported_catalog(case, &answer);
        assert_eq!(
            reported, expected,
            "case {case}: expected the published catalog {expected:?}, got {reported:?}"
        );

        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for (type_, version) in &reported {
            assert_type_pattern(case, type_);
            assert_version_length(case, version);
            assert!(
                seen.insert(type_.as_str()),
                "case {case}: type {type_:?} is reported twice in {reported:?}"
            );
        }
    }
}

/// A published type is usable where a client copies it to: its `Type` is accepted by
/// `CreateConfigurationManager` and its `LatestVersion` by that definition's
/// `TypeVersion`. Checked on `AWSQuickSetupType-SSMChangeMgr`, an entry the issue
/// that prompted this suite did not cite (the catalog's source is the model's own
/// `ConfigurationDefinitionInput$Parameters` parameter tables, one per type).
#[tokio::test]
async fn a_published_catalog_entry_is_accepted_as_a_configuration_definition_type() {
    let router = emulator();

    let entry = QUICK_SETUP_TYPE_CATALOG
        .iter()
        .find(|entry| entry.type_ == "AWSQuickSetupType-SSMChangeMgr")
        .expect("the catalog must publish AWSQuickSetupType-SSMChangeMgr");

    let (status, answer) = call(
        &router,
        "POST",
        "/configurationManager",
        "us-west-2",
        Some(&json!({
            "Name": "change-manager",
            "ConfigurationDefinitions": [{
                "Type": entry.type_,
                "TypeVersion": entry.latest_version(),
                "Parameters": {
                    "DelegatedAccountId": "000000000000",
                    // This type's documented table marks the job function `(Required)`
                    // too.
                    "JobFunction": "operators",
                    // Every accepted definition's parameters name somewhere to deploy
                    // to and the Regions to deploy into.
                    "TargetOrganizationalUnits": "ou-1",
                    "TargetRegions": "us-east-1",
                },
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {}: a create naming a published type and its published LatestVersion must \
         succeed, got {status} / {answer:?}",
        entry.type_
    );
}

// ---------------------------------------------------------------------------
// Service settings: persisted per account and Region.
// ---------------------------------------------------------------------------

/// An update is read back exactly as it was written, whatever the role ARN's form:
/// the rows differ along partition, account, role path and name length — everything
/// the `IAMRoleArn` reference leaves free.
#[tokio::test]
async fn configured_service_settings_are_read_back_exactly_as_the_update_wrote_them() {
    let router = emulator();

    let long_name = "r".repeat(64);
    let cases: Vec<(&str, String)> = vec![
        (
            "a plain role in the emulator account",
            "arn:aws:iam::000000000000:role/QuickSetupExplorerRole".to_string(),
        ),
        (
            "a role reached through a path",
            "arn:aws:iam::123456789012:role/quicksetup/explorer/Role".to_string(),
        ),
        (
            "a service-linked role",
            "arn:aws:iam::123456789012:role/aws-service-role/ssm.amazonaws.com/AWSServiceRoleForAmazonSSM"
                .to_string(),
        ),
        (
            "another partition",
            "arn:aws-us-gov:iam::123456789012:role/ExplorerRole".to_string(),
        ),
        (
            "the China partition",
            "arn:aws-cn:iam::123456789012:role/ExplorerRole".to_string(),
        ),
        (
            "a 64-character role name",
            format!("arn:aws:iam::000000000000:role/{long_name}"),
        ),
    ];

    // Every row writes over the previous one in the SAME Region, so the read also
    // pins last-write-wins rather than first-write-sticks.
    for (case, role_arn) in &cases {
        let (status, answer) = update_settings(
            &router,
            "us-west-2",
            &json!({ "ExplorerEnablingRoleArn": role_arn }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: UpdateServiceSettings must answer 200, got {status} / {answer:?}"
        );

        let (status, answer) = get_settings(&router, "us-west-2").await;
        assert_eq!(
            status, 200,
            "case {case}: GetServiceSettings must answer 200, got {status} / {answer:?}"
        );
        assert_eq!(
            reported_role_arn(&answer).as_deref(),
            Some(role_arn.as_str()),
            "case {case}: expected the role the update wrote, got {answer:?}"
        );
    }
}

/// A Region that was never configured reports an EMPTY `ServiceSettings` structure —
/// a 200 carrying the structure with `ExplorerEnablingRoleArn` unset, because the
/// operation models no way for settings to be absent — and configuring one Region
/// leaves every other Region unconfigured.
#[tokio::test]
async fn an_unconfigured_region_reports_empty_service_settings() {
    let router = emulator();

    let regions = ["us-west-2", "eu-west-1", "ap-southeast-2"];
    for region in regions {
        let (status, answer) = get_settings(&router, region).await;
        assert_eq!(
            status, 200,
            "case {region}: a read before any update must answer 200, got {status} / {answer:?}"
        );
        assert_eq!(
            reported_settings(region, &answer),
            json!({}),
            "case {region}: an unconfigured Region must report an empty ServiceSettings \
             structure, got {answer:?}"
        );
    }

    let role_arn = "arn:aws:iam::000000000000:role/OnlyInUsWest2";
    let (status, answer) = update_settings(
        &router,
        "us-west-2",
        &json!({ "ExplorerEnablingRoleArn": role_arn }),
    )
    .await;
    assert_eq!(status, 200, "the update must succeed, got {status} / {answer:?}");

    for region in regions {
        let (status, answer) = get_settings(&router, region).await;
        assert_eq!(
            status, 200,
            "case {region}: GetServiceSettings must answer 200, got {status} / {answer:?}"
        );
        let expected = (region == "us-west-2").then_some(role_arn);
        assert_eq!(
            reported_role_arn(&answer).as_deref(),
            expected,
            "case {region}: settings are per Region; expected {expected:?}, got {answer:?}"
        );
        assert_eq!(
            reported_settings(region, &answer),
            match expected {
                Some(arn) => json!({ "ExplorerEnablingRoleArn": arn }),
                None => json!({}),
            },
            "case {region}: every answer carries the ServiceSettings structure, got {answer:?}"
        );
    }
}

/// Every `GetServiceSettings` answer carries the `ServiceSettings` structure, and the
/// unconfigured state is expressed INSIDE it by leaving `ExplorerEnablingRoleArn`
/// unset — never by omitting the structure.
///
/// The rows differ along everything the rule generalises over: whether this Region
/// was ever written to at all, whether the write named a member, whether a DIFFERENT
/// Region holds settings, how many reads precede this one, and the Region string
/// itself.
#[tokio::test]
async fn every_read_reports_a_service_settings_structure() {
    /// What happened in the store before the read under test.
    #[derive(Debug, Clone, Copy)]
    enum Setup {
        /// Nothing at all: an empty store.
        Untouched,
        /// A `PUT /serviceSettings` naming no member, which writes nothing.
        EmptyUpdate,
        /// Another Region was configured; this one never was.
        OnlyAnotherRegionConfigured,
        /// This Region was configured with the given role ARN.
        Configured(&'static str),
    }

    let cases: [(&str, &str, Setup, usize); 7] = [
        ("untouched store", "us-west-2", Setup::Untouched, 1),
        ("untouched store, read twice", "eu-west-1", Setup::Untouched, 2),
        (
            "no-member update writes nothing",
            "ap-southeast-2",
            Setup::EmptyUpdate,
            1,
        ),
        (
            "another Region configured",
            "eu-central-1",
            Setup::OnlyAnotherRegionConfigured,
            1,
        ),
        ("us-east-1, untouched", "us-east-1", Setup::Untouched, 1),
        (
            "configured here",
            "us-west-2",
            Setup::Configured("arn:aws:iam::000000000000:role/Explorer"),
            1,
        ),
        (
            "configured here, read three times",
            "sa-east-1",
            Setup::Configured("arn:aws-cn:iam::123456789012:role/path/Explorer"),
            3,
        ),
    ];

    for (case, region, setup, reads) in cases {
        let router = emulator();
        match setup {
            Setup::Untouched => {}
            Setup::EmptyUpdate => {
                let (status, answer) = update_settings(&router, region, &json!({})).await;
                assert_eq!(
                    status, 200,
                    "case {case}: the setup update must answer 200, got {status} / {answer:?}"
                );
            }
            Setup::OnlyAnotherRegionConfigured => {
                let (status, answer) = update_settings(
                    &router,
                    "ca-central-1",
                    &json!({ "ExplorerEnablingRoleArn": "arn:aws:iam::000000000000:role/Elsewhere" }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: the setup update must answer 200, got {status} / {answer:?}"
                );
            }
            Setup::Configured(role_arn) => {
                let (status, answer) = update_settings(
                    &router,
                    region,
                    &json!({ "ExplorerEnablingRoleArn": role_arn }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case}: the setup update must answer 200, got {status} / {answer:?}"
                );
            }
        }

        let expected = match setup {
            Setup::Configured(role_arn) => json!({ "ExplorerEnablingRoleArn": role_arn }),
            _ => json!({}),
        };
        for read in 1..=reads {
            let (status, answer) = get_settings(&router, region).await;
            assert_eq!(
                status, 200,
                "case {case} (read {read}): GetServiceSettings must answer 200, got \
                 {status} / {answer:?}"
            );
            assert_eq!(
                reported_settings(case, &answer),
                expected,
                "case {case} (read {read}): expected ServiceSettings {expected:?}, got {answer:?}"
            );
        }
    }
}

/// Reading the settings reports them; it never changes them. Two reads of the same
/// Region answer identically, before and after a configuring update, and an update
/// that names no member leaves the stored settings — and the absence of any — exactly
/// as they were.
#[tokio::test]
async fn a_read_and_an_empty_update_change_nothing_a_later_caller_sees() {
    let router = emulator();

    let (_, before_first) = get_settings(&router, "us-west-2").await;
    let (_, before_second) = get_settings(&router, "us-west-2").await;
    assert_eq!(
        before_first, before_second,
        "case unconfigured: two reads must answer the same, got {before_first:?} then {before_second:?}"
    );

    // An update naming no member writes nothing, so a Region that was never
    // configured still reports no settings.
    let (status, answer) = update_settings(&router, "us-west-2", &json!({})).await;
    assert_eq!(
        status, 200,
        "case empty update: UpdateServiceSettings must answer 200, got {status} / {answer:?}"
    );
    let (status, answer) = get_settings(&router, "us-west-2").await;
    assert_eq!(status, 200, "got {status} / {answer:?}");
    assert_eq!(
        reported_role_arn(&answer),
        None,
        "case empty update: an update that names nothing must not configure a Region, got {answer:?}"
    );

    let role_arn = "arn:aws:iam::000000000000:role/StableRole";
    let (status, answer) = update_settings(
        &router,
        "us-west-2",
        &json!({ "ExplorerEnablingRoleArn": role_arn }),
    )
    .await;
    assert_eq!(status, 200, "the update must succeed, got {status} / {answer:?}");

    let (_, after_first) = get_settings(&router, "us-west-2").await;
    let (_, after_second) = get_settings(&router, "us-west-2").await;
    assert_eq!(
        after_first, after_second,
        "case configured: two reads must answer the same, got {after_first:?} then {after_second:?}"
    );
    assert_eq!(
        reported_role_arn(&after_second).as_deref(),
        Some(role_arn),
        "case configured: the second read must still report the configured role, got {after_second:?}"
    );

    // An empty update after a configuring one leaves the stored role alone.
    let (status, answer) = update_settings(&router, "us-west-2", &json!({})).await;
    assert_eq!(status, 200, "got {status} / {answer:?}");
    let (_, after_empty) = get_settings(&router, "us-west-2").await;
    assert_eq!(
        reported_role_arn(&after_empty).as_deref(),
        Some(role_arn),
        "case empty update over a configured Region: the stored role must be unchanged, got {after_empty:?}"
    );
}

/// A value that is not an IAM role ARN is refused with the modelled 400
/// `ValidationException` — never stored, and never a 501 — because `IAMRoleArn`
/// carries an `@arnReference` to `AWS::IAM::Role`. The rows differ along which part
/// of the form that reference names is wrong.
#[tokio::test]
async fn a_value_that_is_not_an_iam_role_arn_is_refused_and_stored_settings_survive() {
    let router = emulator();

    let good = "arn:aws:iam::000000000000:role/KeepMe";
    let (status, answer) =
        update_settings(&router, "us-west-2", &json!({ "ExplorerEnablingRoleArn": good })).await;
    assert_eq!(status, 200, "the first update must succeed, got {status} / {answer:?}");

    let cases: [(&str, &str); 9] = [
        ("empty", ""),
        ("not an ARN at all", "QuickSetupExplorerRole"),
        ("an ARN prefix only", "arn:aws:iam"),
        ("an IAM user, not a role", "arn:aws:iam::000000000000:user/bob"),
        ("another service", "arn:aws:ssm:us-west-2:000000000000:role/Explorer"),
        ("an empty partition", "arn::iam::000000000000:role/Explorer"),
        (
            "a partition the ARN formats do not publish",
            "arn:aws-moon:iam::000000000000:role/Explorer",
        ),
        (
            // An IAM ARN carries no Region of its own.
            "a Region in the IAM field",
            "arn:aws:iam:us-west-2:000000000000:role/Explorer",
        ),
        (
            "an account that is not twelve digits",
            "arn:aws:iam::12345:role/Explorer",
        ),
    ];

    for (case, value) in cases {
        let (status, answer) = update_settings(
            &router,
            "us-west-2",
            &json!({ "ExplorerEnablingRoleArn": value }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: {value:?} must be refused with 400, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected a ValidationException, got {answer:?}"
        );

        let (status, read) = get_settings(&router, "us-west-2").await;
        assert_eq!(status, 200, "case {case}: got {status} / {read:?}");
        assert_eq!(
            reported_role_arn(&read).as_deref(),
            Some(good),
            "case {case}: a refused update must leave the stored role alone, got {read:?}"
        );
    }
}
