// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of the Device Farm **Project** lifecycle: `CreateProject`,
//! `GetProject`, `ListProjects`, `UpdateProject`, `DeleteProject`.
//!
//! Every case drives the real wire path — `POST /` with
//! `X-Amz-Target: DeviceFarm_20150623.<Operation>` through the multi-service router —
//! so what is asserted is what an SDK client sees, not a handler's return value.
//!
//! The tables below span the classes the handlers generalise over (name length,
//! presence/absence of each optional member, list size at both modelled bounds, signing
//! region, page count), rather than re-asserting one recorded request.
//!
//! The wire harness here ([`Emulator`] and the small helpers beside it) is this crate's
//! one test harness: a sibling suite under `tests/all/` reaches it as
//! `crate::projects::<item>`.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devicefarm_devicefarm_20150623::state::{
    PROJECTS_PAGE_SIZE, PROJECT_QUOTA,
};
use service_com_amazonaws_devicefarm_devicefarm_20150623::EmulatedService;

/// The signing region most cases use; the create table varies it deliberately.
pub(crate) const REGION: Option<&str> = Some("us-west-2");

/// The region an unsigned request's resources land in, per `state::request_region`.
pub(crate) const DEFAULT_REGION: &str = "us-east-1";

/// One emulator: a shared engine, the Device Farm service, and the router that
/// dispatches to it — the same path `core/router` boots in production.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4 verification, so requests
        // are not signed for real; the `authorization` header is still the only place a
        // request carries its region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// As [`Emulator::new`], with Amazon EC2 mounted beside this service on the SAME
    /// engine, and the in-process callback a suite drives EC2's own operations through.
    ///
    /// A project's `vpcConfig` names EC2's resources, so a suite covering it needs the
    /// owner of those resources answering beside this service — which is why that case
    /// reaches for this constructor rather than a harness of its own.
    pub(crate) fn with_ec2() -> (Self, ::smithy_server_runtime::EmbeddedEmulator) {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(
            Arc::clone(&engine),
        ));
        let callback = ::smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
        (Self { router }, callback)
    }

    /// Call `op` with `body`, signed for `region` (`None` sends it unsigned).
    pub(crate) async fn call(&self, op: &str, region: Option<&str>, body: &Value) -> (u16, Value) {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri("/")
            // Every client addresses the emulator by host, and a handler that publishes a
            // URL on the endpoint the request arrived at reads it from here.
            .header("host", EMULATOR_HOST)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("DeviceFarm_20150623.{op}"));
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/devicefarm/\
                     aws4_request, SignedHeaders=host, Signature=abc"
                ),
            );
        }
        let request = builder
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the test body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a project and return its stored `project` object.
    pub(crate) async fn create(&self, body: &Value) -> Value {
        let (status, response) = self.call("CreateProject", REGION, body).await;
        assert_eq!(status, 200, "CreateProject({body}) answered {status}: {response}");
        response["project"].clone()
    }

    /// Dispatch one request to a URL this emulator PUBLISHED, the way a client follows it:
    /// the method and body of its own choosing, no `X-Amz-Target`, and no credential —
    /// which is how a presigned URL is followed. Used by the upload content plane's suite.
    ///
    /// `declared` overrides the `Content-Length` the request states, so a case can state a
    /// length other than the one it sends; `None` states the body's own length.
    pub(crate) async fn request_url(
        &self,
        method: &str,
        url: &str,
        body: Vec<u8>,
        declared: Option<u64>,
    ) -> (u16, Vec<u8>) {
        let length = declared.unwrap_or(body.len() as u64);
        let request = ::http::Request::builder()
            .method(method)
            .uri(url)
            .header("host", EMULATOR_HOST)
            .header("content-length", length.to_string())
            .header("content-type", "application/octet-stream")
            .body(Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().to_vec())
    }
}

/// The endpoint every request in this crate's suites is addressed to, and therefore the
/// host a URL this emulator publishes carries.
pub(crate) const EMULATOR_HOST: &str = "devicefarm.us-west-2.emulator.test";

/// The wire error code of an awsJson1_1 error body (`__type`).
pub(crate) fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// `n` distinct environment variables, for the list-size rows.
fn env_vars(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|i| json!({ "name": format!("VAR_{i}"), "value": format!("value-{i}") }))
            .collect(),
    )
}

/// The `arn` of a returned resource object, as a `String`.
pub(crate) fn arn_of(resource: &Value) -> String {
    resource["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("resource carries no arn: {resource}"))
        .to_string()
}

/// The resource id a Device Farm ARN carries, which the ARN of a resource living inside a
/// project embeds ahead of its own id (`<type>:<project-id>/<resource-id>`).
pub(crate) fn resource_id(arn: &str) -> String {
    arn.split(':')
        .next_back()
        .unwrap_or_else(|| panic!("ARN carries no resource id: {arn}"))
        .to_string()
}

/// A Device Farm ARN this emulator never minted, of resource type `resource_type`.
pub(crate) fn foreign_arn(resource_type: &str) -> String {
    format!(
        "arn:aws:devicefarm:us-west-2:{}:{resource_type}:11111111-2222-3333-4444-555555555555",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// CreateProject accepts every shape of input the model allows, mints an ARN in the
/// request's own region, defaults the job timeout to the model's documented 150, and
/// GetProject answers with the IDENTICAL stored project.
///
/// The rows differ along what the rule generalises over: which optional members are
/// present, the name's length at both modelled bounds, the environment-variable list's
/// size at both modelled bounds, and the region the request is signed for (including
/// none at all).
#[tokio::test]
async fn created_projects_are_persisted_and_read_back_identically() {
    let emu = Emulator::new();

    // (case, signing region, CreateProject input, expected defaultJobTimeoutMinutes)
    let cases: Vec<(String, Option<&str>, Value, i64)> = vec![
        (
            "only the required name; timeout defaults".to_string(),
            REGION,
            json!({ "name": "bootstrap-demo" }),
            150,
        ),
        (
            "explicit defaultJobTimeoutMinutes".to_string(),
            REGION,
            json!({ "name": "explicit-timeout", "defaultJobTimeoutMinutes": 30 }),
            30,
        ),
        (
            "empty name (modelled Name length minimum of 0)".to_string(),
            REGION,
            json!({ "name": "" }),
            150,
        ),
        (
            "256-character name (modelled Name length maximum)".to_string(),
            Some("eu-west-1"),
            json!({ "name": "p".repeat(256) }),
            150,
        ),
        (
            "one environment variable (modelled list minimum of 1)".to_string(),
            Some("ap-south-1"),
            json!({ "name": "one-env", "environmentVariables": env_vars(1) }),
            150,
        ),
        (
            "32 environment variables (modelled list maximum)".to_string(),
            REGION,
            json!({ "name": "many-env", "environmentVariables": env_vars(32) }),
            150,
        ),
        (
            "vpcConfig and executionRoleArn together".to_string(),
            REGION,
            json!({
                "name": "fully-configured",
                "defaultJobTimeoutMinutes": 600,
                "vpcConfig": {
                    "vpcId": "vpc-0123456789abcdef0",
                    "subnetIds": ["subnet-0123456789abcdef0", "subnet-0123456789abcdef1"],
                    "securityGroupIds": ["sg-0123456789abcdef0"]
                },
                "executionRoleArn": format!(
                    "arn:aws:iam::{}:role/DeviceFarmTestHost", engine::EMULATOR_ACCOUNT_ID
                )
            }),
            600,
        ),
        (
            "unsigned request: the ARN carries the default region".to_string(),
            None,
            json!({ "name": "unsigned-caller" }),
            150,
        ),
    ];

    for (case, region, input, expected_timeout) in &cases {
        let (status, response) = emu.call("CreateProject", *region, input).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateProject to answer 200, got {status:?} with {response}"
        );

        let project = response["project"].clone();
        let arn = arn_of(&project);
        let expected_prefix = format!(
            "arn:aws:devicefarm:{}:{}:project:",
            region.unwrap_or(DEFAULT_REGION),
            engine::EMULATOR_ACCOUNT_ID
        );
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected an ARN starting {expected_prefix:?}, got {arn:?}"
        );
        assert!(
            arn.len() >= 32 && arn.len() <= 1011,
            "case {case:?}: expected the ARN within the modelled length 32..1011, got {:?}",
            arn.len()
        );
        assert_eq!(
            project["name"], input["name"],
            "case {case:?}: expected the stored name {:?}, got {:?}",
            input["name"], project["name"]
        );
        assert_eq!(
            project["defaultJobTimeoutMinutes"].as_i64(),
            Some(*expected_timeout),
            "case {case:?}: expected defaultJobTimeoutMinutes {expected_timeout:?}, got {:?}",
            project["defaultJobTimeoutMinutes"]
        );
        assert!(
            project["created"].as_f64().is_some_and(|secs| secs > 0.0),
            "case {case:?}: expected a created timestamp, got {:?}",
            project["created"]
        );
        for member in ["vpcConfig", "environmentVariables", "executionRoleArn"] {
            assert_eq!(
                project[member], input[member],
                "case {case:?}: expected {member} {:?}, got {:?}",
                input[member], project[member]
            );
        }

        // The round trip: GetProject reports the persisted record, byte for byte —
        // same ARN, same name, same created instant.
        let (status, response) = emu.call("GetProject", REGION, &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProject to answer 200, got {status:?} with {response}"
        );
        assert_eq!(
            response["project"], project,
            "case {case:?}: expected GetProject to return {project}, got {}",
            response["project"]
        );
    }

    // Every created project is listed, each ARN distinct.
    let (status, response) = emu.call("ListProjects", REGION, &json!({})).await;
    assert_eq!(status, 200, "ListProjects answered {status}: {response}");
    let listed: BTreeSet<String> = response["projects"]
        .as_array()
        .unwrap_or_else(|| panic!("ListProjects returned no projects array: {response}"))
        .iter()
        .map(arn_of)
        .collect();
    assert_eq!(
        listed.len(),
        cases.len(),
        "expected {} distinct listed projects, got {:?}",
        cases.len(),
        listed
    );
}

/// The `arn` filter selects exactly the named project, and never the others.
#[tokio::test]
async fn list_projects_filters_by_arn() {
    let emu = Emulator::new();
    let mut projects = Vec::new();
    for name in ["alpha", "beta", "gamma"] {
        projects.push(emu.create(&json!({ "name": name })).await);
    }

    for project in &projects {
        let arn = arn_of(project);
        let (status, response) = emu
            .call("ListProjects", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: expected ListProjects to answer 200, got {status:?} with {response}"
        );
        assert_eq!(
            response["projects"],
            json!([project]),
            "case {arn:?}: expected exactly that project, got {}",
            response["projects"]
        );
        assert!(
            response["nextToken"].is_null(),
            "case {arn:?}: a single-project answer is not truncated, got nextToken {:?}",
            response["nextToken"]
        );
    }
}

/// ListProjects emits `nextToken` only on a truncated page, honours the token it handed
/// back, walks every stored project exactly once, and does so in ARN order.
#[tokio::test]
async fn list_projects_pages_through_every_project_and_resumes_from_its_token() {
    let emu = Emulator::new();

    // One more than a full page, so at least one page is truncated and the last is not.
    let total = PROJECTS_PAGE_SIZE + 3;
    let mut created = BTreeSet::new();
    for i in 0..total {
        let project = emu.create(&json!({ "name": format!("project-{i}") })).await;
        created.insert(arn_of(&project));
    }

    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let (status, response) = emu.call("ListProjects", REGION, &request).await;
        assert_eq!(
            status, 200,
            "case page {pages}: expected 200, got {status:?} with {response}"
        );
        let page: Vec<String> = response["projects"]
            .as_array()
            .unwrap_or_else(|| panic!("page {pages} returned no projects array: {response}"))
            .iter()
            .map(arn_of)
            .collect();
        assert!(
            page.len() <= PROJECTS_PAGE_SIZE,
            "case page {pages}: expected at most {PROJECTS_PAGE_SIZE} projects, got {}",
            page.len()
        );
        seen.extend(page);
        pages += 1;
        token = response["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            pages <= total,
            "case page {pages}: pagination did not terminate after {pages} pages"
        );
    }

    assert_eq!(
        pages, 2,
        "expected {total} projects to span 2 pages of {PROJECTS_PAGE_SIZE}, got {pages}"
    );
    assert_eq!(
        seen.len(),
        total,
        "expected every project exactly once, got {} entries",
        seen.len()
    );
    assert_eq!(
        seen.iter().cloned().collect::<BTreeSet<_>>(),
        created,
        "expected the paged ARNs to be exactly the created ones, got {seen:?}"
    );
    let mut ordered = seen.clone();
    ordered.sort();
    assert_eq!(
        seen, ordered,
        "expected the pages in ARN order, got {seen:?}"
    );
}

/// UpdateProject writes the members the request carries and leaves every other stored
/// member — including the ARN and the creation instant — exactly as it was; the next
/// read shows the update.
#[tokio::test]
async fn update_project_changes_only_the_members_the_request_carries() {
    let emu = Emulator::new();
    let base = json!({
        "name": "base",
        "defaultJobTimeoutMinutes": 45,
        "environmentVariables": [{ "name": "E1", "value": "v1" }],
        "vpcConfig": {
            "vpcId": "vpc-0123456789abcdef0",
            "subnetIds": ["subnet-0123456789abcdef0"],
            "securityGroupIds": ["sg-0123456789abcdef0"]
        },
        "executionRoleArn": format!(
            "arn:aws:iam::{}:role/BaseRole", engine::EMULATOR_ACCOUNT_ID
        )
    });

    // Each row is the patch UpdateProject is called with; the expected answer is the
    // stored project with exactly those members overwritten.
    let patches: Vec<(&str, Value)> = vec![
        ("name only", json!({ "name": "renamed" })),
        ("name emptied to the modelled minimum length", json!({ "name": "" })),
        (
            "defaultJobTimeoutMinutes only",
            json!({ "defaultJobTimeoutMinutes": 600 }),
        ),
        (
            "environmentVariables replaced wholesale",
            json!({ "environmentVariables": env_vars(32) }),
        ),
        (
            "vpcConfig replaced",
            json!({ "vpcConfig": {
                "vpcId": "vpc-0fedcba9876543210",
                "subnetIds": ["subnet-0fedcba9876543210", "subnet-0fedcba9876543211"],
                "securityGroupIds": ["sg-0fedcba9876543210", "sg-0fedcba9876543211"]
            }}),
        ),
        (
            "executionRoleArn replaced",
            json!({ "executionRoleArn": format!(
                "arn:aws:iam::{}:role/OtherRole", engine::EMULATOR_ACCOUNT_ID
            )}),
        ),
        (
            "every member at once",
            json!({
                "name": "everything",
                "defaultJobTimeoutMinutes": 1,
                "environmentVariables": env_vars(2),
                "vpcConfig": {
                    "vpcId": "vpc-0aaaaaaaaaaaaaaaa",
                    "subnetIds": ["subnet-0aaaaaaaaaaaaaaaa"],
                    "securityGroupIds": ["sg-0aaaaaaaaaaaaaaaa"]
                },
                "executionRoleArn": format!(
                    "arn:aws:iam::{}:role/EveryRole", engine::EMULATOR_ACCOUNT_ID
                )
            }),
        ),
        ("no updatable member at all", json!({})),
    ];

    for (case, patch) in &patches {
        // A fresh project per row, so the rows cannot influence one another.
        let stored = emu.create(&base).await;
        let arn = arn_of(&stored);

        let mut expected = stored.clone();
        for (member, value) in patch
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the patch must be an object"))
        {
            expected[member.as_str()] = value.clone();
        }

        let mut request = patch.clone();
        request["arn"] = json!(arn);
        let (status, response) = emu.call("UpdateProject", REGION, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateProject to answer 200, got {status:?} with {response}"
        );
        assert_eq!(
            response["project"], expected,
            "case {case:?}: expected {expected}, got {}",
            response["project"]
        );

        let (status, response) = emu.call("GetProject", REGION, &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetProject to answer 200, got {status:?} with {response}"
        );
        assert_eq!(
            response["project"], expected,
            "case {case:?}: expected the update to persist as {expected}, got {}",
            response["project"]
        );
    }
}

/// DeleteProject removes the record: the delete answers an empty success, a later read
/// is not found, the list no longer carries it, and the projects beside it are untouched.
#[tokio::test]
async fn delete_project_removes_it_from_every_later_read() {
    let emu = Emulator::new();
    let mut projects = Vec::new();
    for name in ["first", "second", "third"] {
        projects.push(emu.create(&json!({ "name": name })).await);
    }

    // Delete each in turn, checking the survivors after every step.
    for (index, project) in projects.iter().enumerate() {
        let arn = arn_of(project);
        let (status, response) = emu
            .call("DeleteProject", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {arn:?}: expected DeleteProject to answer 200, got {status:?} with {response}"
        );
        assert_eq!(
            response,
            json!({}),
            "case {arn:?}: expected an empty DeleteProject body, got {response}"
        );

        let (status, response) = emu.call("GetProject", REGION, &json!({ "arn": arn })).await;
        assert_eq!(
            status, 400,
            "case {arn:?}: expected GetProject on a deleted project to answer 400, got \
             {status:?} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "NotFoundException",
            "case {arn:?}: expected NotFoundException, got {response}"
        );

        // Deleting it again is itself a not-found.
        let (status, response) = emu
            .call("DeleteProject", REGION, &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {arn:?}: expected a repeated DeleteProject to answer 400, got {status:?} \
             with {response}"
        );
        assert_eq!(
            error_code(&response),
            "NotFoundException",
            "case {arn:?}: expected NotFoundException, got {response}"
        );

        let (status, response) = emu.call("ListProjects", REGION, &json!({})).await;
        assert_eq!(status, 200, "case {arn:?}: ListProjects answered {status}: {response}");
        let survivors: Vec<Value> = response["projects"]
            .as_array()
            .unwrap_or_else(|| panic!("ListProjects returned no projects array: {response}"))
            .clone();
        let expected: Vec<Value> = projects[index + 1..].to_vec();
        let expected_arns: BTreeSet<String> = expected.iter().map(arn_of).collect();
        let survivor_arns: BTreeSet<String> = survivors.iter().map(arn_of).collect();
        assert_eq!(
            survivor_arns, expected_arns,
            "case {arn:?}: expected the survivors {expected_arns:?}, got {survivor_arns:?}"
        );
        for survivor in &survivors {
            let original = expected
                .iter()
                .find(|p| arn_of(p) == arn_of(survivor))
                .unwrap_or_else(|| panic!("case {arn:?}: unexpected survivor {survivor}"));
            assert_eq!(
                survivor, original,
                "case {arn:?}: expected the survivor unchanged as {original}, got {survivor}"
            );
        }
    }
}

/// Every ARN-addressed Project operation answers the modelled Device Farm errors, with
/// the modelled HTTP 400, rather than a 501 or a generic framework failure:
/// `ArgumentException` for an ARN that does not name a project at all, and
/// `NotFoundException` for a project ARN no project has.
#[tokio::test]
async fn arn_addressed_operations_answer_the_modelled_device_farm_errors() {
    let emu = Emulator::new();
    // A live project, so a wrong ARN cannot be "not found" merely because the store is empty.
    emu.create(&json!({ "name": "bystander" })).await;

    // Device Farm ARNs the model's `^arn:aws:devicefarm:.+$` pattern accepts and that
    // therefore reach the handler, but which name no project.
    let not_a_project = [
        ("another resource type", foreign_arn("upload")),
        ("a run", foreign_arn("run")),
        ("a device pool", foreign_arn("devicepool")),
    ];
    let unknown_project = foreign_arn("project");

    for op in ["GetProject", "ListProjects", "UpdateProject", "DeleteProject"] {
        for (kind, arn) in &not_a_project {
            let (status, response) = emu.call(op, REGION, &json!({ "arn": arn })).await;
            assert_eq!(
                status, 400,
                "case {op} with {kind}: expected 400, got {status:?} with {response}"
            );
            assert_eq!(
                error_code(&response),
                "ArgumentException",
                "case {op} with {kind}: expected ArgumentException, got {response}"
            );
        }

        let (status, response) = emu
            .call(op, REGION, &json!({ "arn": unknown_project }))
            .await;
        assert_eq!(
            status, 400,
            "case {op} with an unknown project ARN: expected 400, got {status:?} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "NotFoundException",
            "case {op} with an unknown project ARN: expected NotFoundException, got {response}"
        );
    }
}

/// The store carries its own bound: `CreateProject` refuses with the modelled
/// `LimitExceededException` once the store holds `PROJECT_QUOTA` projects, and the bound
/// is on the store rather than on the number of calls — deleting one makes room again.
#[tokio::test]
async fn create_project_refuses_once_the_store_quota_is_reached() {
    let emu = Emulator::new();

    let mut first = None;
    for i in 0..PROJECT_QUOTA {
        let (status, response) = emu
            .call("CreateProject", REGION, &json!({ "name": format!("p-{i}") }))
            .await;
        assert_eq!(
            status, 200,
            "case project {i} of the quota {PROJECT_QUOTA}: expected 200, got {status:?} with \
             {response}"
        );
        if first.is_none() {
            first = Some(arn_of(&response["project"]));
        }
    }
    let first = first.unwrap_or_else(|| panic!("the quota must be at least one project"));

    // One past the bound.
    let (status, response) = emu
        .call("CreateProject", REGION, &json!({ "name": "one-too-many" }))
        .await;
    assert_eq!(
        status, 400,
        "case one past the quota of {PROJECT_QUOTA}: expected 400, got {status:?} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case one past the quota of {PROJECT_QUOTA}: expected LimitExceededException, got \
         {response}"
    );

    // The refused call stored nothing, and the projects already stored are untouched.
    let (status, response) = emu
        .call("GetProject", REGION, &json!({ "arn": first }))
        .await;
    assert_eq!(
        status, 200,
        "case after the refusal: expected the stored projects to still be readable, got \
         {status:?} with {response}"
    );

    // The bound is on the store: releasing a record makes room for another.
    let (status, response) = emu
        .call("DeleteProject", REGION, &json!({ "arn": first }))
        .await;
    assert_eq!(
        status, 200,
        "case freeing a slot: expected DeleteProject to answer 200, got {status:?} with {response}"
    );
    let (status, response) = emu
        .call("CreateProject", REGION, &json!({ "name": "back-under-the-quota" }))
        .await;
    assert_eq!(
        status, 200,
        "case back under the quota: expected 200, got {status:?} with {response}"
    );
    // A released id is never handed out again: the replacement project gets a fresh ARN,
    // so a client holding the deleted one can never reach the new record.
    let replacement = arn_of(&response["project"]);
    assert_ne!(
        replacement, first,
        "case reusing a released slot: expected a fresh ARN, got the deleted {first:?}"
    );
    let (status, response) = emu
        .call("GetProject", REGION, &json!({ "arn": first }))
        .await;
    assert_eq!(
        status, 400,
        "case the deleted ARN after a replacement was created: expected 400, got {status:?} \
         with {response}"
    );
    assert_eq!(
        error_code(&response),
        "NotFoundException",
        "case the deleted ARN after a replacement was created: expected NotFoundException, got \
         {response}"
    );
}

/// The modelled constraints on CreateProject's members are refused at the boundary with
/// HTTP 400 — never answered, and never a 501 — while each published bound itself is
/// accepted.
#[tokio::test]
async fn create_project_refuses_input_outside_the_modelled_constraints() {
    let emu = Emulator::new();

    // Bounds from `smithy_query(query="operation", operation="CreateProject")`:
    // name len 0..256, environmentVariables len 1..32,
    // executionRoleArn len 20..2048 pattern ^arn:aws:iam::[0-9]{12}:role/.+$
    let refused: Vec<(&str, Value)> = vec![
        (
            "name one past the modelled maximum of 256",
            json!({ "name": "p".repeat(257) }),
        ),
        (
            "environmentVariables one below the modelled minimum of 1",
            json!({ "name": "x", "environmentVariables": env_vars(0) }),
        ),
        (
            "environmentVariables one past the modelled maximum of 32",
            json!({ "name": "x", "environmentVariables": env_vars(33) }),
        ),
        (
            "executionRoleArn outside the modelled role-ARN pattern",
            json!({ "name": "x", "executionRoleArn": "arn:aws:iam::000000000000:user/not-a-role" }),
        ),
        ("the required name absent", json!({})),
    ];

    for (case, body) in &refused {
        let (status, response) = emu.call("CreateProject", REGION, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status:?} with {response}"
        );
    }

    let accepted: Vec<(&str, Value)> = vec![
        (
            "name at the modelled minimum length of 0",
            json!({ "name": "" }),
        ),
        (
            "name at the modelled maximum length of 256",
            json!({ "name": "p".repeat(256) }),
        ),
        (
            "environmentVariables at the modelled minimum of 1",
            json!({ "name": "x", "environmentVariables": env_vars(1) }),
        ),
        (
            "environmentVariables at the modelled maximum of 32",
            json!({ "name": "x", "environmentVariables": env_vars(32) }),
        ),
    ];

    for (case, body) in &accepted {
        let (status, response) = emu.call("CreateProject", REGION, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the modelled bound to be accepted, got {status:?} with \
             {response}"
        );
    }
}

/// A pagination token `ListProjects` never issued is refused as a bad argument rather
/// than answered from an arbitrary point in the store; a token it did issue is a project
/// ARN and resumes from where that ARN sorts, even once that project is deleted.
#[tokio::test]
async fn list_projects_refuses_a_token_it_never_issued() {
    let emu = Emulator::new();
    let mut arns: Vec<String> = Vec::new();
    for i in 0..3 {
        arns.push(arn_of(&emu.create(&json!({ "name": format!("p-{i}") })).await));
    }
    arns.sort();

    // The rows differ along the FORM of the token. The model constrains
    // `PaginationToken` to length 4..1024, so every row is at least four characters and
    // therefore reaches the handler rather than the framework's length check.
    let refused: Vec<(&str, String)> = vec![
        ("not an ARN at all", "page-2".to_string()),
        ("another Device Farm resource type", foreign_arn("upload")),
        (
            "a project ARN with an empty resource id",
            format!(
                "arn:aws:devicefarm:us-west-2:{}:project:",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        (
            "another service's ARN",
            format!("arn:aws:iam::{}:role/SomeRole", engine::EMULATOR_ACCOUNT_ID),
        ),
        ("an opaque base64-looking token", "eyJhIjoxfQ==".to_string()),
    ];

    for (case, token) in &refused {
        let (status, response) = emu
            .call("ListProjects", REGION, &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status:?} with {response}"
        );
        assert_eq!(
            error_code(&response),
            "ArgumentException",
            "case {case:?}: expected ArgumentException, got {response}"
        );
    }

    // A token this operation issues is a project ARN: it resumes from that ARN.
    let (status, response) = emu
        .call("ListProjects", REGION, &json!({ "nextToken": arns[1] }))
        .await;
    assert_eq!(
        status, 200,
        "case a token ListProjects issued: expected 200, got {status:?} with {response}"
    );
    let listed: Vec<String> = response["projects"]
        .as_array()
        .unwrap_or_else(|| panic!("ListProjects returned no projects array: {response}"))
        .iter()
        .map(arn_of)
        .collect();
    assert_eq!(
        listed,
        arns[1..].to_vec(),
        "case a token ListProjects issued: expected the page {:?}, got {listed:?}",
        &arns[1..]
    );

    // The token names an ARN, not a live record: once that project is deleted the same
    // token still resumes, from where its ARN sorts.
    let (status, response) = emu
        .call("DeleteProject", REGION, &json!({ "arn": arns[1] }))
        .await;
    assert_eq!(
        status, 200,
        "case freeing the token's project: expected 200, got {status:?} with {response}"
    );
    let (status, response) = emu
        .call("ListProjects", REGION, &json!({ "nextToken": arns[1] }))
        .await;
    assert_eq!(
        status, 200,
        "case a token naming a deleted project: expected 200, got {status:?} with {response}"
    );
    let listed: Vec<String> = response["projects"]
        .as_array()
        .unwrap_or_else(|| panic!("ListProjects returned no projects array: {response}"))
        .iter()
        .map(arn_of)
        .collect();
    assert_eq!(
        listed,
        arns[2..].to_vec(),
        "case a token naming a deleted project: expected the page {:?}, got {listed:?}",
        &arns[2..]
    );
}
