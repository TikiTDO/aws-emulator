// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Lake Formation governance of a Data Catalog DATABASE READ:
//! `GetDatabase` and the `GetDatabases` listing beside it.
//!
//! The rule under test: a database's metadata is answered only to a principal that holds a
//! Lake Formation permission ON that database, on a resource INSIDE it (a grant on a table
//! makes the table's parent database describable), or over the whole data lake as an
//! administrator — the permission being held in the principal's own name or through the
//! `IAM_ALLOWED_PRINCIPALS` group every IAM user and role belongs to. A read by a principal
//! none of those cover is refused with HTTP 400 `AccessDeniedException`; a listing OMITS
//! such a database rather than refusing the call.
//!
//! Lake Formation OWNS those grants, so it is mounted beside Glue on the SAME engine (the
//! pair the production router builds) and every grant, revoke and settings write here goes
//! through Lake Formation's own wire API: what the catalog consults is the state those calls
//! wrote, never a copy kept inside Glue. IAM and STS are mounted too, because the reader of
//! these rows is an assumed-role session — a principal other than the one that created the
//! catalog, which is what makes a refusal observable at all.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! resource the grant names (the database, a table in it, that table's columns, a table of
//! another database), which permission it carries, which principal holds it, whether the
//! holder is an administrator instead of a grantee, and which reader asks.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, ACCOUNT_ID, AKID, REGION};

/// The Lake Formation group every IAM user and role belongs to, spelled as Lake Formation
/// spells it on the wire. Source: `DataLakeSettings$CreateDatabaseDefaultPermissions`,
/// which documents the `ALL`-to-`IAM_ALLOWED_PRINCIPALS` value a catalog starts under.
const IAM_ALLOWED_PRINCIPALS: &str = "IAM_ALLOWED_PRINCIPALS";

/// The database every row reads.
const DATABASE: &str = "lake";

/// The table that database holds.
const TABLE: &str = "usage_events";

/// A second database, so a row can show that a decision about one says nothing about
/// another.
const OTHER_DATABASE: &str = "warehouse";

/// The role the reader of these rows assumes.
const READER_ROLE: &str = "reader";

/// A role of this account nothing in these tests signs as, so a row can hold a grant for a
/// principal other than the reader. It is CREATED in IAM like the reader's, because Lake
/// Formation resolves a grant's principal against this account's IAM.
const ANOTHER_ROLE: &str = "someone-else";

/// That role's ARN, the identity a Lake Formation grant to the reader names.
fn reader_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/{READER_ROLE}")
}

/// A principal other than the reader: a role of this account nothing in these tests signs
/// as.
fn another_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT_ID}:role/{ANOTHER_ROLE}")
}

/// An emulator with Glue, Lake Formation, IAM and STS over ONE shared engine, with request
/// authentication off (these tests send no real signature), driven through the same router
/// the emulator boots.
fn emulator_with_lake_formation() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lakeformation_awslakeformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine),
    );
    router
}

/// One Lake Formation call (`restJson1`: `POST /<Operation>`), answering its status and
/// parsed body. The router routes on the SigV4 credential scope's service field, which is
/// why this harness differs from the Glue one only in that header.
async fn lake_formation(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://lakeformation.{REGION}.amazonaws.com/{operation}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={AKID}/20260101/{REGION}/lakeformation/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the Lake Formation request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One Lake Formation call that must succeed.
async fn lake_formation_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = lake_formation(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: Lake Formation {operation} must answer 200, got {status} with {parsed}"
    );
    parsed
}

/// One Glue call signed with `access_key_id` in the credential scope, so a row speaks as the
/// session STS minted rather than as the account's own credential.
async fn glue_as(
    router: &MultiServiceRouter,
    access_key_id: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSGlue.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/glue/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the Glue request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One awsQuery call to IAM or STS, signed with the account's own credential, asserting it
/// succeeded and answering the XML body.
async fn query(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, String)],
) -> String {
    let mut body = format!("Action={action}&Version={version}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.amazonaws.com/"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={AKID}/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body))
        .expect("build the awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {action:?}: the setup call must succeed, got {status} with {text}"
    );
    text
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC
/// 3986's unreserved characters literal — so a policy document's braces and quotes survive
/// the round trip.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// The text of the first `<name>…</name>` element of an awsQuery response.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// The emulator each row runs against: a governed catalog, and the access key id of the
/// reader's role session.
struct Governed {
    router: MultiServiceRouter,
    /// The access key id `sts:AssumeRole` minted for the reader's session.
    session_key: String,
}

/// A catalog holding [`DATABASE`] with [`TABLE`] in it and an [`OTHER_DATABASE`] holding one
/// too, created through Glue's own wire API by the ACCOUNT's own credential — and GOVERNED,
/// i.e. each database hands the tables created in it NO `IAM_ALLOWED_PRINCIPALS` default,
/// which is the state a client that has handed its catalog to Lake Formation is in.
///
/// The reader is a SESSION of [`READER_ROLE`], a principal that created none of it: what a
/// table's creator holds on the tables it created is a grant of its own
/// (Lake Formation reports it), so a suite about what a grant confers has to read as someone
/// else.
async fn governed_catalog() -> Governed {
    let router = emulator_with_lake_formation();
    for name in [DATABASE, OTHER_DATABASE] {
        let (status, body) = call(
            &router,
            "CreateDatabase",
            &json!({
                "DatabaseInput": { "Name": name, "CreateTableDefaultPermissions": [] },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case create database {name:?}: expected 200, got {status} with {body}"
        );
        let (status, body) = call(
            &router,
            "CreateTable",
            &json!({ "DatabaseName": name, "TableInput": { "Name": TABLE } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case create table {name}.{TABLE}: expected 200, got {status} with {body}"
        );
    }

    let trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":\"arn:aws:iam::{ACCOUNT_ID}:root\"}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    );
    // Both roles a row may name are created, because a Lake Formation grant is resolved
    // against this account's IAM: the reader every row reads as, and the OTHER principal
    // one row holds a grant for.
    for role_name in [READER_ROLE, ANOTHER_ROLE] {
        query(
            &router,
            "iam",
            "2010-05-08",
            "CreateRole",
            &[
                ("RoleName", role_name.to_string()),
                ("AssumeRolePolicyDocument", trust.clone()),
            ],
        )
        .await;
    }
    let assumed = query(
        &router,
        "sts",
        "2011-06-15",
        "AssumeRole",
        &[
            ("RoleArn", reader_arn()),
            ("RoleSessionName", "probe".to_string()),
        ],
    )
    .await;
    let session_key = xml_element(&assumed, "AccessKeyId")
        .unwrap_or_else(|| panic!("sts:AssumeRole must answer an AccessKeyId, got {assumed}"));
    Governed {
        router,
        session_key,
    }
}

/// Revoke the grant `database` was created carrying: `ALL` to `IAM_ALLOWED_PRINCIPALS`, the
/// account's "use only IAM access control" default, which is the first step of locking a
/// database down.
async fn revoke_default_grant(router: &MultiServiceRouter, case: &str, database: &str) {
    lake_formation_ok(
        router,
        case,
        "RevokePermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": IAM_ALLOWED_PRINCIPALS },
            "Resource": { "Database": { "Name": database } },
            "Permissions": ["ALL"],
        }),
    )
    .await;
}

/// Grant `permissions` on `resource` to `principal`.
async fn grant(
    router: &MultiServiceRouter,
    case: &str,
    principal: &str,
    resource: Value,
    permissions: &[&str],
) {
    lake_formation_ok(
        router,
        case,
        "GrantPermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": principal },
            "Resource": resource,
            "Permissions": permissions,
        }),
    )
    .await;
}

/// `Resource` naming [`DATABASE`] itself.
fn on_database() -> Value {
    json!({ "Database": { "Name": DATABASE } })
}

/// `Resource` naming the table inside [`DATABASE`].
fn on_table() -> Value {
    json!({ "Table": { "DatabaseName": DATABASE, "Name": TABLE } })
}

/// `Resource` naming named COLUMNS of the table inside [`DATABASE`].
fn on_table_columns() -> Value {
    json!({
        "TableWithColumns": {
            "DatabaseName": DATABASE,
            "Name": TABLE,
            "ColumnNames": ["id"],
        }
    })
}

/// `Resource` naming the table of the OTHER database.
fn on_other_database_table() -> Value {
    json!({ "Table": { "DatabaseName": OTHER_DATABASE, "Name": TABLE } })
}

/// What the Lake Formation permission state of one row is: what it does after the catalog is
/// created, which is what the read is then judged against.
enum Held {
    /// Nothing is revoked: the database still carries the grant it was created with.
    CreationDefault,
    /// The default is revoked and nothing is granted.
    Nothing,
    /// The default is revoked and `permissions` are granted to `principal` on `resource`.
    Granted {
        principal: fn() -> String,
        resource: fn() -> Value,
        permissions: &'static [&'static str],
    },
    /// The default is revoked and the reader is made an administrator of the data lake
    /// instead of a grantee.
    DataLakeAdmin,
}

impl Held {
    /// Put this row's permission state in place, through Lake Formation's own API.
    async fn apply(&self, router: &MultiServiceRouter, case: &str) {
        match self {
            Self::CreationDefault => {}
            Self::Nothing => revoke_default_grant(router, case, DATABASE).await,
            Self::Granted {
                principal,
                resource,
                permissions,
            } => {
                revoke_default_grant(router, case, DATABASE).await;
                grant(router, case, &principal(), resource(), permissions).await;
            }
            Self::DataLakeAdmin => {
                revoke_default_grant(router, case, DATABASE).await;
                lake_formation_ok(
                    router,
                    case,
                    "PutDataLakeSettings",
                    &json!({
                        "DataLakeSettings": {
                            "DataLakeAdmins": [
                                { "DataLakePrincipalIdentifier": reader_arn() },
                            ],
                        },
                    }),
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn a_database_read_is_answered_only_where_lake_formation_grants_it() {
    // The rows differ along what the rule generalises over: whether the creation-time grant
    // is still there, which resource a grant names (the database, a table in it, that
    // table's columns, a table of ANOTHER database), which permission it carries, which
    // principal holds it, and whether the reader is an administrator instead of a grantee.
    let cases: Vec<(&str, Held, u16)> = vec![
        (
            "the creation-time IAM_ALLOWED_PRINCIPALS grant the database carries",
            Held::CreationDefault,
            200,
        ),
        (
            "nothing at all, once that grant is revoked",
            Held::Nothing,
            400,
        ),
        (
            "DESCRIBE on the database itself",
            Held::Granted {
                principal: reader_arn,
                resource: on_database,
                permissions: &["DESCRIBE"],
            },
            200,
        ),
        (
            "SELECT on the database, a permission other than DESCRIBE",
            Held::Granted {
                principal: reader_arn,
                resource: on_database,
                permissions: &["SELECT"],
            },
            200,
        ),
        (
            "DESCRIBE on ONE TABLE in the database, with no database-level grant",
            Held::Granted {
                principal: reader_arn,
                resource: on_table,
                permissions: &["DESCRIBE"],
            },
            200,
        ),
        (
            "SELECT on named COLUMNS of a table in the database",
            Held::Granted {
                principal: reader_arn,
                resource: on_table_columns,
                permissions: &["SELECT"],
            },
            200,
        ),
        (
            "the group every IAM principal belongs to, granted on a table in the database",
            Held::Granted {
                principal: || IAM_ALLOWED_PRINCIPALS.to_string(),
                resource: on_table,
                permissions: &["DESCRIBE"],
            },
            200,
        ),
        (
            "DESCRIBE on a table of ANOTHER database",
            Held::Granted {
                principal: reader_arn,
                resource: on_other_database_table,
                permissions: &["DESCRIBE"],
            },
            400,
        ),
        (
            "DESCRIBE on this database, held by ANOTHER principal",
            Held::Granted {
                principal: another_principal,
                resource: on_database,
                permissions: &["DESCRIBE"],
            },
            400,
        ),
        (
            "nothing granted, the reader being an administrator of the data lake",
            Held::DataLakeAdmin,
            200,
        ),
    ];

    for (case, held, expected) in &cases {
        let governed = governed_catalog().await;
        held.apply(&governed.router, case).await;

        let (status, body) = glue_as(
            &governed.router,
            &governed.session_key,
            "GetDatabase",
            &json!({ "Name": DATABASE }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {body}"
        );
        if *expected == 200 {
            assert_eq!(
                body["Database"]["Name"].as_str(),
                Some(DATABASE),
                "case {case:?}: expected the database to be answered, got {body}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "AccessDeniedException",
                "case {case:?}: expected AccessDeniedException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "Insufficient Lake Formation permission(s): Required Describe on {DATABASE}"
                ),
                "case {case:?}: expected the Lake Formation refusal naming the database, got \
                 {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_table_grant_makes_its_database_describable_while_the_database_holds_no_grant() {
    // The implicit parent visibility on its own: after the revoke the reader holds NOTHING
    // on the database — which Lake Formation's own listing of the Database resource confirms
    // — and a grant on one table inside it is what makes the database readable again.
    let case = "a table grant confers database visibility";
    let governed = governed_catalog().await;
    revoke_default_grant(&governed.router, case, DATABASE).await;

    let (status, body) = glue_as(
        &governed.router,
        &governed.session_key,
        "GetDatabase",
        &json!({ "Name": DATABASE }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: with nothing held the read must be refused, got {status} with {body}"
    );

    grant(&governed.router, case, &reader_arn(), on_table(), &["DESCRIBE"]).await;

    let listed = lake_formation_ok(
        &governed.router,
        case,
        "ListPermissions",
        &json!({
            "Principal": { "DataLakePrincipalIdentifier": reader_arn() },
            "Resource": on_database(),
        }),
    )
    .await;
    assert_eq!(
        listed["PrincipalResourcePermissions"],
        json!([]),
        "case {case:?}: the database itself must hold no permission entry, got {listed}"
    );

    let (status, body) = glue_as(
        &governed.router,
        &governed.session_key,
        "GetDatabase",
        &json!({ "Name": DATABASE }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the table grant must make the database readable, got {status} with \
         {body}"
    );
}

#[tokio::test]
async fn the_principal_that_created_a_table_still_reads_its_database() {
    // The same rule, read by another class of principal: the account's own credential created
    // the catalog, so Lake Formation reports it holding the creator's grant on each table —
    // and that grant on a resource INSIDE the database is what keeps the database readable to
    // it after the database's own creation-time grant is revoked.
    let case = "the creator of the tables";
    let governed = governed_catalog().await;
    revoke_default_grant(&governed.router, case, DATABASE).await;

    let (status, body) = call(&governed.router, "GetDatabase", &json!({ "Name": DATABASE })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the creator to still read the database, got {status} with \
         {body}"
    );
}

#[tokio::test]
async fn the_listing_omits_a_database_the_caller_may_not_describe() {
    // A listing reports the databases its caller may see: it omits the one nothing grants
    // rather than refusing the call, and reports it again once a grant exists. The second
    // database, whose own creation-time grant nothing touched, is listed throughout.
    let case = "the listing follows the grants";
    let governed = governed_catalog().await;

    let names = |page: &Value| -> Vec<String> {
        page["DatabaseList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: GetDatabases must answer a list, got {page}"))
            .iter()
            .map(|database| database["Name"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    let (status, page) = glue_as(
        &governed.router,
        &governed.session_key,
        "GetDatabases",
        &json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {page}"
    );
    assert_eq!(
        names(&page),
        vec![DATABASE.to_string(), OTHER_DATABASE.to_string()],
        "case {case:?}: both databases carry their creation-time grant, got {page}"
    );

    revoke_default_grant(&governed.router, case, DATABASE).await;
    let (status, page) = glue_as(
        &governed.router,
        &governed.session_key,
        "GetDatabases",
        &json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a listing is not refused for a database it omits, got {status} with \
         {page}"
    );
    assert_eq!(
        names(&page),
        vec![OTHER_DATABASE.to_string()],
        "case {case:?}: expected only the database still granted, got {page}"
    );

    grant(
        &governed.router,
        case,
        &reader_arn(),
        on_database(),
        &["DESCRIBE"],
    )
    .await;
    let (status, page) = glue_as(
        &governed.router,
        &governed.session_key,
        "GetDatabases",
        &json!({}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} with {page}"
    );
    assert_eq!(
        names(&page),
        vec![DATABASE.to_string(), OTHER_DATABASE.to_string()],
        "case {case:?}: the granted database must be listed again, got {page}"
    );
}

#[tokio::test]
async fn a_name_the_catalog_does_not_hold_is_still_reported_as_a_missing_entity() {
    // Lake Formation governs what may be read OF A DATABASE THAT EXISTS: a name the catalog
    // holds nothing under is the catalog's own miss, after a revoke as before one, so a
    // client asking for a database that is not there is told so.
    let case = "a name the catalog does not hold";
    let governed = governed_catalog().await;
    revoke_default_grant(&governed.router, case, DATABASE).await;

    let (status, body) = glue_as(
        &governed.router,
        &governed.session_key,
        "GetDatabase",
        &json!({ "Name": "absent" }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "EntityNotFoundException",
        "case {case:?}: expected EntityNotFoundException, got {body}"
    );
}
