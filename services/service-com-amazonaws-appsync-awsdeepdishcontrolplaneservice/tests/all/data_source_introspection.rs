// SPDX-License-Identifier: CC0-1.0

//! Data source introspections, end to end over HTTP: `StartDataSourceIntrospection` and
//! the `GetDataSourceIntrospection` that reads the record it created back.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. introspecting a data source is ASYNCHRONOUS: the call creates ONE record, mints its
//!    opaque `introspectionId` and answers the non-terminal `PROCESSING` status with no
//!    detail and no result, and the clock then carries that record to a terminal status
//!    with no further call — and two reads of it agree;
//! 2. an introspection that read the schema is in `SUCCESS` and carries one model per
//!    table of the database it named, each with the fields that table's columns declare
//!    (the AppSync scalar the column's type maps to, a non-null type for a column that
//!    admits no null, and the length its declaration bounds a character column to), the
//!    primary key those columns form and the indexes the schema declares over them —
//!    asserted over a schema spanning the type classes PostgreSQL holds, a single and a
//!    composite primary key, a table with no key at all and a declared index;
//! 3. `includeModelsSDL` is what adds each model's SDL, and adds nothing else;
//! 4. `maxResults` and `nextToken` page those models, and the pages concatenate to exactly
//!    the whole set — over every page size from one model to more than there are;
//! 5. an introspection the owner of the cluster would not serve is in `FAILED`, carries
//!    the reason it was refused with and reports no result — over a table spanning a
//!    cluster that does not exist, one whose Data API endpoint is off and a database the
//!    cluster does not have;
//! 6. a call that names no data source to introspect is a 400 `BadRequestException`, an
//!    `introspectionId` no introspection was created for is a 404 `NotFoundException`, and
//!    a `nextToken` that resumes nothing is a 400.
//!
//! Every resource a case needs is created through its OWNER's own operations — the cluster
//! with `rds:CreateDBCluster` (RDS managing the master user password, so the secret is
//! RDS's own), its tables with the Data API's `ExecuteStatement` — so what an introspection
//! reports is the schema of a real database of this emulator and not a fixture written
//! into its store.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call below is signed for, and the Region every resource lives in.
const REGION: &str = "us-east-1";

/// The master user every cluster here is created with.
const MASTER_USER: &str = "pgadmin";

/// The database every cluster here is created with (`DBName`), which is the one an
/// introspection names.
const DATABASE: &str = "appdb";

/// The access key id every call presents: a key neither STS nor IAM holds a record for,
/// which is the position AWS account root credentials are in.
const ACCOUNT_KEY_ID: &str = "AKIAEMULATOR";

/// How long a case waits for the status the clock owes, at the outside. Generous on
/// purpose: the case waits for the STATUS to settle rather than for a span of time, so a
/// loaded host only polls more often, never fails.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a case leaves between two looks at a record that has not settled yet.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One answer from the emulator: the parts a client branches on.
#[derive(Debug, Clone)]
struct Answer {
    status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves the error shape from.
    error_type: String,
    body: Value,
}

/// One emulator: AppSync with the owners of what an introspection reads mounted beside it
/// — the Data API that serves the cluster's databases, the RDS control plane that holds the
/// cluster, and the Secrets Manager (with the KMS its value is sealed under) the login
/// comes from — over ONE shared engine, exactly as the assembled router mounts them.
struct Cloud {
    router: MultiServiceRouter,
}

impl Cloud {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Verifying signatures is the emulator's concern, not this service's; these tests
        // sign nothing, so the check is off and the credential scope is read for routing
        // and for the access key id alone.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_rdsdata_rdsdataservice::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// An Amazon RDS client over this emulator's own in-process callback.
    fn rds(&self) -> aws_sdk_rds::Client {
        let callback = smithy_server_runtime::EmbeddedEmulator::from_router(self.router.clone());
        let config = aws_sdk_rds::Config::builder()
            .behavior_version(aws_sdk_rds::config::BehaviorVersion::latest())
            .region(aws_sdk_rds::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://rds.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(aws_sdk_rds::config::retry::RetryConfig::disabled())
            .timeout_config(aws_sdk_rds::config::timeout::TimeoutConfig::disabled())
            .identity_cache(aws_sdk_rds::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                aws_sdk_rds::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        aws_sdk_rds::Client::from_conf(config)
    }

    /// Create a cluster the way a Data API caller does — its HTTP endpoint on or off as
    /// `http_endpoint` says, RDS managing the master user password — and answer its ARN
    /// and the ARN of the secret RDS minted for it.
    async fn create_cluster(&self, identifier: &str, http_endpoint: bool) -> (String, String) {
        let created = self
            .rds()
            .create_db_cluster()
            .db_cluster_identifier(identifier)
            .engine("aurora-postgresql")
            .master_username(MASTER_USER)
            .manage_master_user_password(true)
            .database_name(DATABASE)
            .enable_http_endpoint(http_endpoint)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "case {identifier}: CreateDBCluster must be served: {}",
                    aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            });
        let cluster = created
            .db_cluster()
            .unwrap_or_else(|| panic!("case {identifier}: a created cluster is reported"));
        (
            cluster
                .db_cluster_arn()
                .unwrap_or_else(|| panic!("case {identifier}: a created cluster carries its ARN"))
                .to_string(),
            cluster
                .master_user_secret()
                .and_then(aws_sdk_rds::types::MasterUserSecret::secret_arn)
                .unwrap_or_else(|| {
                    panic!("case {identifier}: a managed-password cluster reports its secret")
                })
                .to_string(),
        )
    }

    /// Run one statement against `cluster` through the Data API, which must serve it.
    async fn execute(&self, cluster: &str, secret: &str, sql: &str) {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://rds-data.{REGION}.amazonaws.com/Execute"))
            .header("host", format!("rds-data.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization("rds-data"))
            .body(bytes::Bytes::from(
                json!({
                    "resourceArn": cluster,
                    "secretArn": secret,
                    "database": DATABASE,
                    "sql": sql,
                })
                .to_string(),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(status, 200, "case {sql}: expected 200, got {status} {body}");
    }

    /// Send one AppSync request and return what the emulator answered.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(value.to_string())
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
            .header("host", format!("appsync.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header("authorization", authorization("appsync"))
            .body(payload)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Answer {
            status,
            error_type,
            body,
        }
    }

    /// `StartDataSourceIntrospection` with the configuration `config` names.
    async fn start(&self, config: Value) -> Answer {
        self.call("POST", INTROSPECTIONS_PATH, Some(config)).await
    }

    /// `StartDataSourceIntrospection` of `cluster`'s own database, which must be served,
    /// and the `introspectionId` it minted.
    async fn start_on(&self, case: &str, cluster: &str, secret: &str) -> String {
        let started = self
            .start(json!({"rdsDataApiConfig": rds_config(cluster, secret, DATABASE)}))
            .await;
        assert_eq!(
            started.status, 200,
            "case {case}: expected StartDataSourceIntrospection 200, got {} {}",
            started.status, started.body
        );
        assert_eq!(
            started.body["introspectionStatus"].as_str(),
            Some(PROCESSING),
            "case {case}: a new introspection is reported as scheduled, got {}",
            started.body
        );
        started.body["introspectionId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a started introspection carries its id"))
            .to_string()
    }

    /// `GetDataSourceIntrospection` of `introspection_id`, with the query `query` carries.
    async fn get(&self, introspection_id: &str, query: &str) -> Answer {
        self.call(
            "GET",
            &format!("{INTROSPECTIONS_PATH}/{introspection_id}{query}"),
            None,
        )
        .await
    }

    /// Read `introspection_id` back once the clock has made its introspection due.
    ///
    /// Waits for the CONDITION — a status that is no longer the non-terminal one — by
    /// looking again until it holds or [`SETTLE_DEADLINE`] passes, so nothing here depends
    /// on how fast the host runs.
    async fn settled(&self, case: &str, introspection_id: &str, query: &str) -> Answer {
        let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
        loop {
            let answer = self.get(introspection_id, query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected GetDataSourceIntrospection 200, got {} {}",
                answer.status, answer.body
            );
            if answer.body["introspectionStatus"].as_str() != Some(PROCESSING) {
                return answer;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "case {case}: the introspection never left {PROCESSING}: {}",
                answer.body
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The path both introspection operations are served on.
const INTROSPECTIONS_PATH: &str = "/v1/datasources/introspections";

/// The non-terminal `DataSourceIntrospectionStatus`, as the service names it.
const PROCESSING: &str = state::INTROSPECTION_PROCESSING;

/// One request's SigV4 authorization header, for the service `signing_name` claims.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential={ACCOUNT_KEY_ID}/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=unverified"
    )
}

/// The `rdsDataApiConfig` of one call.
fn rds_config(cluster: &str, secret: &str, database: &str) -> Value {
    json!({
        "resourceArn": cluster,
        "secretArn": secret,
        "databaseName": database,
    })
}

/// The schema every case that introspects a real database creates first.
///
/// One table per class the rule generalises over: a table with a single-column key and a
/// column of every PostgreSQL type class, one with a COMPOSITE key, and one with no key at
/// all — plus an index the schema declares itself.
const SCHEMA: &[&str] = &[
    "CREATE TABLE todos (id serial PRIMARY KEY, title varchar(50) NOT NULL, done boolean, \
     score numeric(5,2), tags text, due timestamp, payload jsonb)",
    "CREATE TABLE notes (owner varchar(10), seq int, body text, PRIMARY KEY (owner, seq))",
    "CREATE TABLE visits (seen date, note text)",
    "CREATE INDEX todos_title_idx ON todos (title, done)",
];

/// Every model [`SCHEMA`] defines, in the order an introspection reports them.
fn expected_models() -> Value {
    json!([
        {
            "name": "notes",
            "fields": [
                field("owner", "String", true, 10),
                field("seq", "Int", true, 0),
                field("body", "String", false, 0),
            ],
            "primaryKey": {"name": "notes_pkey", "fields": ["owner", "seq"]},
            "indexes": [],
        },
        {
            "name": "todos",
            "fields": [
                field("id", "Int", true, 0),
                field("title", "String", true, 50),
                field("done", "Boolean", false, 0),
                field("score", "Float", false, 0),
                field("tags", "String", false, 0),
                field("due", "AWSDateTime", false, 0),
                field("payload", "AWSJSON", false, 0),
            ],
            "primaryKey": {"name": "todos_pkey", "fields": ["id"]},
            "indexes": [{"name": "todos_title_idx", "fields": ["title", "done"]}],
        },
        {
            "name": "visits",
            "fields": [
                field("seen", "AWSDate", false, 0),
                field("note", "String", false, 0),
            ],
            "indexes": [],
        },
    ])
}

/// One field of a model as the introspection reports it: the scalar its column's type maps
/// to, wrapped in a non-null type for a column that admits no null.
fn field(name: &str, scalar: &str, non_null: bool, length: i64) -> Value {
    let scalar = json!({"kind": "Scalar", "name": scalar});
    let field_type = if non_null {
        json!({"kind": "NonNull", "type": scalar})
    } else {
        scalar
    };
    let mut field = json!({"name": name, "type": field_type});
    field["length"] = json!(length);
    field
}

/// The models one answer reports.
fn models(answer: &Answer) -> Value {
    answer.body["introspectionResult"]["models"].clone()
}

/// RULE: an introspection reports one model per table of the database it read, with the
/// fields that table's columns declare, the primary key they form and the indexes its
/// schema declares — and the record is created, scheduled and settled by the clock alone.
#[tokio::test]
async fn an_introspection_reports_the_schema_of_the_database_it_read() {
    let cloud = Cloud::new();
    let (cluster, secret) = cloud.create_cluster("introspected", true).await;
    for statement in SCHEMA {
        cloud.execute(&cluster, &secret, statement).await;
    }

    let started = cloud
        .start(json!({"rdsDataApiConfig": rds_config(&cluster, &secret, DATABASE)}))
        .await;
    assert_eq!(
        started.status, 200,
        "case start: expected 200, got {} {}",
        started.status, started.body
    );
    let introspection_id = started.body["introspectionId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !introspection_id.is_empty(),
        "case start: expected a minted introspectionId, got {}",
        started.body
    );
    assert_eq!(
        started.body["introspectionStatus"].as_str(),
        Some(PROCESSING),
        "case start: expected the scheduled status, got {}",
        started.body
    );
    assert_eq!(
        started.body["introspectionStatusDetail"],
        Value::Null,
        "case start: a scheduled introspection details nothing, got {}",
        started.body
    );

    // Before the clock has made it due, the record is readable and reports the same
    // non-terminal status — and no result, because nothing has been introspected yet.
    let scheduled = cloud.get(&introspection_id, "").await;
    assert_eq!(
        scheduled.status, 200,
        "case scheduled read: expected 200, got {} {}",
        scheduled.status, scheduled.body
    );
    assert_eq!(
        scheduled.body["introspectionId"].as_str(),
        Some(introspection_id.as_str()),
        "case scheduled read: expected the record that was created, got {}",
        scheduled.body
    );
    assert_eq!(
        scheduled.body["introspectionResult"],
        Value::Null,
        "case scheduled read: a scheduled introspection has read no schema, got {}",
        scheduled.body
    );

    let settled = cloud.settled("schema", &introspection_id, "").await;
    assert_eq!(
        settled.body["introspectionStatus"].as_str(),
        Some(state::INTROSPECTION_SUCCESS),
        "case schema: expected the introspection to have been performed, got {}",
        settled.body
    );
    assert_eq!(
        models(&settled),
        expected_models(),
        "case schema: expected the database's own schema, got {}",
        settled.body
    );
    assert_eq!(
        settled.body["introspectionResult"]["nextToken"],
        Value::Null,
        "case schema: the whole schema fits one page, got {}",
        settled.body
    );

    // A read reports the record and changes nothing a later caller sees.
    let again = cloud.get(&introspection_id, "").await;
    assert_eq!(
        again.body, settled.body,
        "case re-read: expected the same record, got {}",
        again.body
    );
}

/// RULE: `includeModelsSDL` is what adds each model's SDL — the type its fields declare and
/// nothing else — and the models are otherwise reported identically.
#[tokio::test]
async fn the_sdl_of_a_model_is_added_only_when_the_call_asks_for_it() {
    let cloud = Cloud::new();
    let (cluster, secret) = cloud.create_cluster("introspected-sdl", true).await;
    for statement in SCHEMA {
        cloud.execute(&cluster, &secret, statement).await;
    }
    let introspection_id = cloud.start_on("sdl", &cluster, &secret).await;
    let settled = cloud.settled("sdl", &introspection_id, "").await;

    // (the query, whether every model carries its SDL)
    let rows = [
        ("", false),
        ("?includeModelsSDL=false", false),
        ("?includeModelsSDL=true", true),
    ];
    for (query, carries_sdl) in rows {
        let answer = cloud.get(&introspection_id, query).await;
        assert_eq!(
            answer.status, 200,
            "case {query:?}: expected 200, got {} {}",
            answer.status, answer.body
        );
        let reported = models(&answer);
        let listed = reported
            .as_array()
            .unwrap_or_else(|| panic!("case {query:?}: expected models, got {}", answer.body));
        for model in listed {
            let sdl = model["sdl"].as_str();
            assert_eq!(
                sdl.is_some(),
                carries_sdl,
                "case {query:?}: expected sdl present={carries_sdl}, got {model}"
            );
            if let Some(sdl) = sdl {
                let name = model["name"].as_str().unwrap_or_default();
                assert!(
                    sdl.starts_with(&format!("type {name} {{")) && sdl.ends_with('}'),
                    "case {query:?}: expected the SDL of type {name}, got {sdl:?}"
                );
                for field in model["fields"].as_array().unwrap_or(&Vec::new()) {
                    let field_name = field["name"].as_str().unwrap_or_default();
                    assert!(
                        sdl.contains(&format!("{field_name}: ")),
                        "case {query:?}: expected {field_name} in the SDL of {name}, got {sdl:?}"
                    );
                }
                assert!(
                    !sdl.contains('@'),
                    "case {query:?}: the SDL carries no directives, got {sdl:?}"
                );
            }
        }
        // The SDL is the only difference: with it stripped, the models are the same.
        let mut stripped = reported.clone();
        if let Some(listed) = stripped.as_array_mut() {
            for model in listed {
                if let Some(model) = model.as_object_mut() {
                    model.remove("sdl");
                }
            }
        }
        assert_eq!(
            stripped,
            models(&settled),
            "case {query:?}: expected the same models, got {reported}"
        );
    }
}

/// RULE: `maxResults` and `nextToken` page the models, and the pages concatenate to
/// exactly the set one unpaged call reports.
#[tokio::test]
async fn the_models_are_paged_by_max_results_and_resumed_by_the_token() {
    let cloud = Cloud::new();
    let (cluster, secret) = cloud.create_cluster("introspected-pages", true).await;
    for statement in SCHEMA {
        cloud.execute(&cluster, &secret, statement).await;
    }
    let introspection_id = cloud.start_on("pages", &cluster, &secret).await;
    let whole = models(&cloud.settled("pages", &introspection_id, "").await);
    let expected = whole
        .as_array()
        .unwrap_or_else(|| panic!("case pages: expected models, got {whole}"))
        .clone();

    // Every page size from one model to more than there are, so the cut is asserted
    // wherever it can fall.
    for page_size in 1..=expected.len() + 1 {
        let mut collected: Vec<Value> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let answer = cloud.get(&introspection_id, &query).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}: expected 200, got {} {}",
                answer.status, answer.body
            );
            let page = models(&answer);
            let page = page.as_array().unwrap_or_else(|| {
                panic!("case page size {page_size}: expected models, got {}", answer.body)
            });
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} models, got {}",
                answer.body
            );
            collected.extend(page.iter().cloned());
            match answer.body["introspectionResult"]["nextToken"].as_str() {
                Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case page size {page_size}: expected the whole schema across the pages, got {collected:?}"
        );
    }
}

/// RULE: an introspection the owner of the cluster would not serve is in `FAILED`, carries
/// the reason it was refused with and reports no result.
#[tokio::test]
async fn an_introspection_the_data_api_will_not_serve_settles_to_failed() {
    let cloud = Cloud::new();
    let (served, secret) = cloud.create_cluster("introspected-failures", true).await;
    let (unserved, unserved_secret) = cloud.create_cluster("endpoint-off", false).await;
    let absent = served.replace("introspected-failures", "never-created");

    // (case, the configuration, the words the reason must carry)
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "a cluster that was never created",
            rds_config(&absent, &secret, DATABASE),
            absent.clone(),
        ),
        (
            "a cluster whose Data API endpoint is off",
            rds_config(&unserved, &unserved_secret, DATABASE),
            unserved.clone(),
        ),
        (
            "a database the cluster does not have",
            rds_config(&served, &secret, "nosuchdb"),
            "nosuchdb".to_string(),
        ),
    ];
    for (case, config, reason) in rows {
        let started = cloud.start(json!({"rdsDataApiConfig": config})).await;
        assert_eq!(
            started.status, 200,
            "case {case}: an introspection is accepted and reports asynchronously, got {} {}",
            started.status, started.body
        );
        let introspection_id = started.body["introspectionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let settled = cloud.settled(case, &introspection_id, "").await;
        assert_eq!(
            settled.body["introspectionStatus"].as_str(),
            Some(state::INTROSPECTION_FAILED),
            "case {case}: expected a failed introspection, got {}",
            settled.body
        );
        let detail = settled.body["introspectionStatusDetail"]
            .as_str()
            .unwrap_or_default();
        assert!(
            detail.contains(&reason),
            "case {case}: expected the reason to name {reason:?}, got {detail:?}"
        );
        assert_eq!(
            settled.body["introspectionResult"],
            Value::Null,
            "case {case}: a failed introspection read no schema, got {}",
            settled.body
        );
    }
}

/// RULE: a call that names no data source to introspect is a 400 `BadRequestException`, an
/// `introspectionId` no introspection was created for is a 404 `NotFoundException`, and a
/// `nextToken` that resumes nothing is a 400.
#[tokio::test]
async fn a_call_that_names_nothing_to_read_is_refused() {
    let cloud = Cloud::new();
    let (cluster, secret) = cloud.create_cluster("introspected-refusals", true).await;
    cloud.execute(&cluster, &secret, SCHEMA[0]).await;
    let introspection_id = cloud.start_on("refusals", &cluster, &secret).await;
    cloud.settled("refusals", &introspection_id, "").await;

    let missing_config = cloud.start(json!({})).await;
    assert_eq!(
        missing_config.status, 400,
        "case no configuration: expected 400, got {} {}",
        missing_config.status, missing_config.body
    );
    assert_eq!(
        missing_config.error_type, "BadRequestException",
        "case no configuration: got {} {}",
        missing_config.error_type, missing_config.body
    );

    // Every id class that names no record of this account: one of the UUID form this
    // service mints but never minted, one of another form, and one too short to be any
    // reference at all.
    let unknown = [
        (
            "an unminted id of the form this service mints",
            "0191a5a4-0000-4000-8000-000000000000",
        ),
        ("an id of another form", "introspection-7"),
        ("a single character", "x"),
    ];
    for (case, id) in unknown {
        let answer = cloud.get(id, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected 404, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "NotFoundException",
            "case {case}: got {} {}",
            answer.error_type, answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(state::INTROSPECTION_NOT_FOUND_MESSAGE),
            "case {case}: expected the absent-record sentence, got {}",
            answer.body
        );
    }

    let unresumable = cloud.get(&introspection_id, "?nextToken=nosuchmodel").await;
    assert_eq!(
        unresumable.status, 400,
        "case an unresumable token: expected 400, got {} {}",
        unresumable.status, unresumable.body
    );
    assert_eq!(
        unresumable.error_type, "BadRequestException",
        "case an unresumable token: got {} {}",
        unresumable.error_type, unresumable.body
    );
}
