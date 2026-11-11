// SPDX-License-Identifier: CC0-1.0

//! `RestoreFromSnapshot`, end to end over the wire, with the Data API mounted beside the
//! control plane so a restore can be shown to have moved real data.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than the one request
//! the issue quoted:
//!
//! 1. A snapshot holds the namespace's databases as they stood when it was taken, and
//!    `RestoreFromSnapshot` makes the workgroup it names hold exactly that — so a
//!    statement submitted against the restored workgroup afterwards reads the rows that
//!    were written before the snapshot, whatever those rows are, in whatever database,
//!    and whether the snapshot is addressed by name or by ARN.
//! 2. What the restore writes back is the SNAPSHOT, not the live namespace: work done
//!    after the snapshot was taken — more rows, fewer rows, another table — is not in the
//!    restored workgroup.
//! 3. The call answers `{snapshotName, ownerAccount, namespace}` out of the stored
//!    records, so a later `GetNamespace` reports the same namespace; a `namespaceName`
//!    that resolves to nothing is restored into a namespace minted from the snapshot's
//!    own properties.
//! 4. A request that addresses no single snapshot, or names a snapshot, an owner or a
//!    workgroup that resolves to nothing, or a namespace with a change already in flight,
//!    is refused with the operation's own modelled error and restores nothing.
//!
//! The control-plane harness (the signed call, the error-code reader, the create helpers)
//! is shared with `namespace_workgroup_lifecycle`, which owns it; only the two-service
//! emulator and the Data API calls are new here.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::namespace_workgroup_lifecycle::{
    call, create_namespace, create_workgroup, error_code, is_uuid, ACCOUNT,
};

/// Comfortably longer than the span a clock-owed status transition falls due after
/// (`crate::state`'s `SETTLE_AFTER`, 500 ms).
const LONGER_THAN_SETTLE: Duration = Duration::from_millis(900);

/// The `WaitTimeSeconds` every statement asks for: inside the modelled 1..30 range and
/// far longer than the SQL engine's own deadline, so what decides a statement's outcome
/// is the engine and never this request's patience on a loaded machine.
const WAIT_SECONDS: i32 = 10;

/// The `DbUser` every statement authenticates as. Any name does: the Data API's engine
/// addresses a database by workgroup and database name.
const DB_USER: &str = "awsuser";

/// The database every namespace here is provisioned with: `CreateNamespace` names none, so
/// the control plane names its first database itself. A warehouse holds that database plus
/// the ones statements CREATED in it, so a case using another one creates it first.
const PROVISIONED_DATABASE: &str = "dev";

/// One emulator carrying BOTH services over ONE shared engine, the way the router mounts
/// them in production.
///
/// `namespace_workgroup_lifecycle::emulator` mounts the control plane alone, which cannot
/// show that a restore moved any data: the databases a snapshot copies live in the SQL
/// engine the Redshift Data API runs statements on.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_redshiftserverless_redshiftserverless::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_redshiftdata_redshiftdata::EmulatedService::new(engine),
    );
    router
}

/// Call a Redshift Data API operation and answer its HTTP status and parsed body. The
/// credential scope names `redshift-data`, which is what routes it to the other service.
async fn data_call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("RedshiftData.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDTEST/20260101/us-east-1/redshift-data/aws4_request, \
             SignedHeaders=host, Signature=deadbeef",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Run `sql` against `workgroup`/`database`, waiting for it, and answer the statement's
/// `Id`. A statement that does not finish fails the test here rather than later.
async fn run_sql(
    router: &MultiServiceRouter,
    workgroup: &str,
    database: &str,
    sql: &str,
) -> String {
    let (status, body) = data_call(
        router,
        "ExecuteStatement",
        json!({
            "WorkgroupName": workgroup,
            "Database": database,
            "DbUser": DB_USER,
            "Sql": sql,
            "WaitTimeSeconds": WAIT_SECONDS,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "statement {sql:?} on {workgroup}/{database}: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["Status"],
        json!("FINISHED"),
        "statement {sql:?} on {workgroup}/{database}: expected FINISHED, got {body}"
    );
    body["Id"].as_str().unwrap_or_default().to_string()
}

/// The `Records` `sql` answers against `workgroup`/`database`.
async fn query(
    router: &MultiServiceRouter,
    workgroup: &str,
    database: &str,
    sql: &str,
) -> Value {
    let id = run_sql(router, workgroup, database, sql).await;
    let (status, body) = data_call(router, "GetStatementResult", json!({ "Id": id })).await;
    assert_eq!(
        status, 200,
        "GetStatementResult for {sql:?} on {workgroup}/{database}: expected 200, got \
         {status} with {body}"
    );
    body["Records"].clone()
}

/// The status and body of `sql` against `workgroup`/`database`, without requiring it to
/// finish: a statement against a table that is no longer there FAILS, and that failure is
/// what proves the data is gone.
async fn try_sql(
    router: &MultiServiceRouter,
    workgroup: &str,
    database: &str,
    sql: &str,
) -> Value {
    let (status, body) = data_call(
        router,
        "ExecuteStatement",
        json!({
            "WorkgroupName": workgroup,
            "Database": database,
            "DbUser": DB_USER,
            "Sql": sql,
            "WaitTimeSeconds": WAIT_SECONDS,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "statement {sql:?} on {workgroup}/{database}: expected 200, got {status} with {body}"
    );
    body
}

/// Snapshot `namespace` under `snapshot`, asserting it succeeded, and answer the snapshot
/// the call stored.
async fn create_snapshot(
    router: &MultiServiceRouter,
    namespace: &str,
    snapshot: &str,
) -> Value {
    let (status, body) = call(
        router,
        "CreateSnapshot",
        json!({ "namespaceName": namespace, "snapshotName": snapshot }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateSnapshot {snapshot}: expected 200, got {status} with {body}"
    );
    body["snapshot"].clone()
}

// ---------------------------------------------------------------------------
// Rule 1 — the restore makes the named workgroup serve the snapshot's databases.
// ---------------------------------------------------------------------------

/// One row of the restore table: the data written before the snapshot, how the restore
/// addresses the snapshot, and what the restored workgroup must then serve.
#[derive(Debug, Clone, Copy)]
struct RestoreCase {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// The database the writes and the read-back run in.
    database: &'static str,
    /// The statements that write the data the snapshot has to capture.
    writes: &'static [&'static str],
    /// Statements run against the TARGET workgroup BEFORE the restore, so the row also
    /// shows what the restore replaced.
    stale_writes: &'static [&'static str],
    /// The read-back, run against the restored workgroup.
    read: &'static str,
    /// The `Records` that read-back must answer.
    expected: fn() -> Value,
    /// Address the snapshot by `snapshotArn` rather than by `snapshotName`.
    by_arn: bool,
}

#[tokio::test]
async fn a_restore_makes_the_named_workgroup_serve_the_snapshots_databases() {
    // The rows differ along what the rule generalises over: the value and type of the
    // data, the number of rows, the number of tables, the database name, whether the
    // target workgroup already held data of its own, and how the snapshot is addressed.
    let cases = [
        RestoreCase {
            label: "integers and text in three rows",
            database: "dev",
            writes: &[
                "CREATE TABLE people (id INTEGER, name VARCHAR(16))",
                "INSERT INTO people (id, name) VALUES (1, 'ada'), (2, 'alan'), (3, 'grace')",
            ],
            stale_writes: &[],
            read: "SELECT id, name FROM people ORDER BY id",
            expected: || {
                json!([
                    [{"longValue": 1}, {"stringValue": "ada"}],
                    [{"longValue": 2}, {"stringValue": "alan"}],
                    [{"longValue": 3}, {"stringValue": "grace"}]
                ])
            },
            by_arn: false,
        },
        RestoreCase {
            label: "two tables, addressed by snapshotArn",
            database: "dev",
            writes: &[
                "CREATE TABLE orders (id INTEGER)",
                "CREATE TABLE items (id INTEGER)",
                "INSERT INTO orders (id) VALUES (1), (2)",
                "INSERT INTO items (id) VALUES (7)",
            ],
            stale_writes: &[],
            read: "SELECT (SELECT COUNT(*) FROM orders), (SELECT COUNT(*) FROM items)",
            expected: || json!([[{"longValue": 2}, {"longValue": 1}]]),
            by_arn: true,
        },
        RestoreCase {
            label: "a hundred rows",
            database: "dev",
            writes: &[
                "CREATE TABLE series (n INTEGER)",
                "INSERT INTO series (n) WITH RECURSIVE counter(n) AS \
                 (SELECT 1 UNION ALL SELECT n + 1 FROM counter WHERE n < 100) \
                 SELECT n FROM counter",
            ],
            stale_writes: &[],
            read: "SELECT COUNT(*), MAX(n) FROM series",
            expected: || json!([[{"longValue": 100}, {"longValue": 100}]]),
            by_arn: false,
        },
        RestoreCase {
            label: "a row of every value kind",
            database: "dev",
            writes: &[
                "CREATE TABLE mixed (i INTEGER, r REAL, t VARCHAR(8), b BLOB, n INTEGER)",
                "INSERT INTO mixed (i, r, t, b, n) VALUES (7, 1.5, 'x', x'6162', NULL)",
            ],
            stale_writes: &[],
            read: "SELECT i, r, t, b, n FROM mixed",
            expected: || {
                json!([[
                    {"longValue": 7},
                    {"doubleValue": 1.5},
                    {"stringValue": "x"},
                    {"blobValue": "YWI="},
                    {"isNull": true}
                ]])
            },
            by_arn: false,
        },
        RestoreCase {
            label: "a database that is not the first one",
            database: "reporting",
            writes: &[
                "CREATE TABLE totals (amount INTEGER)",
                "INSERT INTO totals (amount) VALUES (42)",
            ],
            stale_writes: &[],
            read: "SELECT amount FROM totals",
            expected: || json!([[{"longValue": 42}]]),
            by_arn: false,
        },
        RestoreCase {
            label: "a target workgroup that already held data of its own",
            database: "dev",
            writes: &[
                "CREATE TABLE ledger (entry VARCHAR(8))",
                "INSERT INTO ledger (entry) VALUES ('kept')",
            ],
            stale_writes: &[
                "CREATE TABLE ledger (entry VARCHAR(8))",
                "INSERT INTO ledger (entry) VALUES ('stale'), ('also-stale')",
            ],
            read: "SELECT entry FROM ledger ORDER BY entry",
            expected: || json!([[{"stringValue": "kept"}]]),
            by_arn: false,
        },
    ];

    let router = emulator();
    for (index, case) in cases.iter().enumerate() {
        let source_namespace = format!("restore-src-ns-{index}");
        let source_workgroup = format!("restore-src-wg-{index}");
        let target_namespace = format!("restore-dst-ns-{index}");
        let target_workgroup = format!("restore-dst-wg-{index}");
        let snapshot_name = format!("restore-snap-{index}");

        create_namespace(&router, &source_namespace).await;
        create_workgroup(&router, &source_workgroup, &source_namespace).await;
        // A case that writes to a database other than the one the namespace was
        // provisioned with CREATES that database first: a warehouse holds the database its
        // control plane gave it plus the ones callers created, and nothing else.
        if case.database != PROVISIONED_DATABASE {
            let created = format!("CREATE DATABASE {}", case.database);
            run_sql(&router, &source_workgroup, PROVISIONED_DATABASE, &created).await;
        }
        for sql in case.writes {
            run_sql(&router, &source_workgroup, case.database, sql).await;
        }

        let snapshot = create_snapshot(&router, &source_namespace, &snapshot_name).await;
        let snapshot_arn = snapshot["snapshotArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The compute the restore runs through has to exist first: this operation creates
        // no workgroup, and the workgroup is what serves the restored databases.
        create_namespace(&router, &target_namespace).await;
        create_workgroup(&router, &target_workgroup, &target_namespace).await;
        if !case.stale_writes.is_empty() && case.database != PROVISIONED_DATABASE {
            let created = format!("CREATE DATABASE {}", case.database);
            run_sql(&router, &target_workgroup, PROVISIONED_DATABASE, &created).await;
        }
        for sql in case.stale_writes {
            run_sql(&router, &target_workgroup, case.database, sql).await;
        }

        let mut request = json!({
            "namespaceName": target_namespace,
            "workgroupName": target_workgroup,
        });
        if case.by_arn {
            request["snapshotArn"] = json!(snapshot_arn);
        } else {
            request["snapshotName"] = json!(snapshot_name);
        }
        let (status, body) = call(&router, "RestoreFromSnapshot", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: RestoreFromSnapshot {request} expected 200, got {status} with \
             {body}",
            label = case.label
        );
        // The answer names the snapshot that was restored and its owner, out of the
        // stored record — so an ARN-addressed restore reports the snapshot's own NAME.
        assert_eq!(
            body["snapshotName"],
            json!(snapshot_name),
            "case {label:?}: expected snapshotName {snapshot_name}, got {body}",
            label = case.label
        );
        assert_eq!(
            body["ownerAccount"],
            json!(ACCOUNT),
            "case {label:?}: expected ownerAccount {ACCOUNT}, got {body}",
            label = case.label
        );
        assert_eq!(
            body["namespace"]["namespaceName"],
            json!(target_namespace),
            "case {label:?}: expected the restored namespace {target_namespace}, got {body}",
            label = case.label
        );

        // The point of the restore: the rows are served by the workgroup it named.
        let records = query(&router, &target_workgroup, case.database, case.read).await;
        assert_eq!(
            records,
            (case.expected)(),
            "case {label:?}: {read:?} on the restored workgroup expected {expected}, got \
             {records}",
            label = case.label,
            read = case.read,
            expected = (case.expected)()
        );
        // And the source is untouched by its own snapshot being restored elsewhere.
        let source_records = query(&router, &source_workgroup, case.database, case.read).await;
        assert_eq!(
            source_records,
            (case.expected)(),
            "case {label:?}: the source workgroup must still serve its own rows, got \
             {source_records}",
            label = case.label
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — what comes back is the snapshot, not the live namespace.
// ---------------------------------------------------------------------------

/// One row of the point-in-time table: work done AFTER the snapshot was taken, and the
/// read-back that must not show it.
#[derive(Debug, Clone, Copy)]
struct AfterCase {
    label: &'static str,
    /// Statements run against the SOURCE workgroup after the snapshot was taken.
    after_snapshot: &'static [&'static str],
    /// The read-back against the restored workgroup.
    read: &'static str,
    /// What that read-back must answer: the pre-snapshot state.
    expected: fn() -> Value,
}

#[tokio::test]
async fn a_restore_brings_back_the_snapshot_and_not_the_later_writes() {
    // The rows differ along the kind of change made after the snapshot: rows added, rows
    // removed, a table dropped, a table created, and a whole second database written.
    let cases = [
        AfterCase {
            label: "rows inserted after the snapshot",
            after_snapshot: &["INSERT INTO facts (n) VALUES (99)"],
            read: "SELECT n FROM facts ORDER BY n",
            expected: || json!([[{"longValue": 1}], [{"longValue": 2}]]),
        },
        AfterCase {
            label: "rows deleted after the snapshot",
            after_snapshot: &["DELETE FROM facts"],
            read: "SELECT n FROM facts ORDER BY n",
            expected: || json!([[{"longValue": 1}], [{"longValue": 2}]]),
        },
        AfterCase {
            label: "the table dropped after the snapshot",
            after_snapshot: &["DROP TABLE facts"],
            read: "SELECT COUNT(*) FROM facts",
            expected: || json!([[{"longValue": 2}]]),
        },
        AfterCase {
            label: "another table created after the snapshot",
            after_snapshot: &["CREATE TABLE later (n INTEGER)"],
            read: "SELECT COUNT(*) FROM sqlite_schema WHERE name = 'later'",
            expected: || json!([[{"longValue": 0}]]),
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        // One emulator per row: the rows change the SOURCE in ways that must not leak
        // into each other.
        let router = emulator();
        let source_namespace = format!("after-src-ns-{index}");
        let source_workgroup = format!("after-src-wg-{index}");
        let target_namespace = format!("after-dst-ns-{index}");
        let target_workgroup = format!("after-dst-wg-{index}");
        let snapshot_name = format!("after-snap-{index}");

        create_namespace(&router, &source_namespace).await;
        create_workgroup(&router, &source_workgroup, &source_namespace).await;
        for sql in [
            "CREATE TABLE facts (n INTEGER)",
            "INSERT INTO facts (n) VALUES (1), (2)",
        ] {
            run_sql(&router, &source_workgroup, "dev", sql).await;
        }
        create_snapshot(&router, &source_namespace, &snapshot_name).await;
        for sql in case.after_snapshot {
            run_sql(&router, &source_workgroup, "dev", sql).await;
        }

        create_namespace(&router, &target_namespace).await;
        create_workgroup(&router, &target_workgroup, &target_namespace).await;
        let (status, body) = call(
            &router,
            "RestoreFromSnapshot",
            json!({
                "namespaceName": target_namespace,
                "workgroupName": target_workgroup,
                "snapshotName": snapshot_name,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: RestoreFromSnapshot expected 200, got {status} with {body}",
            label = case.label
        );

        let records = query(&router, &target_workgroup, "dev", case.read).await;
        assert_eq!(
            records,
            (case.expected)(),
            "case {label:?}: the restore must answer the snapshot's state, expected \
             {expected}, got {records}",
            label = case.label,
            expected = (case.expected)()
        );
    }
}

#[tokio::test]
async fn a_restore_leaves_the_workgroup_holding_only_the_snapshots_databases() {
    // A restore replaces a namespace's contents: a database the snapshot does not hold is
    // not in the restored workgroup either, so a table in it is gone afterwards.
    let router = emulator();
    create_namespace(&router, "only-src-ns").await;
    create_workgroup(&router, "only-src-wg", "only-src-ns").await;
    run_sql(&router, "only-src-wg", "dev", "CREATE TABLE kept (n INTEGER)").await;
    run_sql(&router, "only-src-wg", "dev", "INSERT INTO kept (n) VALUES (5)").await;
    create_snapshot(&router, "only-src-ns", "only-snap").await;

    create_namespace(&router, "only-dst-ns").await;
    create_workgroup(&router, "only-dst-wg", "only-dst-ns").await;
    // The target holds a database the snapshot never had. A database is a database of the
    // warehouse because it was CREATED there, so the statement that creates it runs on the
    // one its namespace was provisioned with.
    run_sql(&router, "only-dst-wg", "dev", "CREATE DATABASE sandbox").await;
    run_sql(
        &router,
        "only-dst-wg",
        "sandbox",
        "CREATE TABLE scratch (n INTEGER)",
    )
    .await;

    let (status, body) = call(
        &router,
        "RestoreFromSnapshot",
        json!({
            "namespaceName": "only-dst-ns",
            "workgroupName": "only-dst-wg",
            "snapshotName": "only-snap",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "RestoreFromSnapshot expected 200, got {status} with {body}"
    );

    let kept = query(&router, "only-dst-wg", "dev", "SELECT n FROM kept").await;
    assert_eq!(
        kept,
        json!([[{"longValue": 5}]]),
        "the snapshot's database must be served after the restore, got {kept}"
    );
    let scratch = try_sql(
        &router,
        "only-dst-wg",
        "sandbox",
        "SELECT n FROM scratch",
    )
    .await;
    assert_eq!(
        scratch["Status"],
        json!("FAILED"),
        "a database the snapshot does not hold must be gone after the restore, got \
         {scratch}"
    );
}

#[tokio::test]
async fn a_restored_database_is_still_bounded_by_the_engines_storage_limit() {
    // The copy moves PAGES, not connection settings, so a restored database is bounded by
    // the same `max_page_count` the engine installs on every database it opens: a
    // statement that would grow it past that bound ends FAILED rather than consuming the
    // emulator, exactly as it does on a database nothing restored into.
    let router = emulator();
    create_namespace(&router, "bound-src-ns").await;
    create_workgroup(&router, "bound-src-wg", "bound-src-ns").await;
    run_sql(&router, "bound-src-wg", "dev", "CREATE TABLE small (n INTEGER)").await;
    run_sql(&router, "bound-src-wg", "dev", "INSERT INTO small (n) VALUES (1)").await;
    create_snapshot(&router, "bound-src-ns", "bound-snap").await;

    create_namespace(&router, "bound-dst-ns").await;
    create_workgroup(&router, "bound-dst-wg", "bound-dst-ns").await;
    let (status, body) = call(
        &router,
        "RestoreFromSnapshot",
        json!({
            "namespaceName": "bound-dst-ns",
            "workgroupName": "bound-dst-wg",
            "snapshotName": "bound-snap",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "RestoreFromSnapshot expected 200, got {status} with {body}"
    );

    // The same statement the Data API's own bound test uses: a table far larger than a
    // database may hold, each row carrying a value just under the engine's own per-value
    // ceiling so the bound is reached in few rows rather than many.
    let overflowing = try_sql(
        &router,
        "bound-dst-wg",
        "dev",
        "CREATE TABLE bulk AS WITH RECURSIVE counter(x) AS (SELECT 1 UNION ALL \
         SELECT x + 1 FROM counter WHERE x < 400) SELECT x, randomblob(900000) AS pad \
         FROM counter",
    )
    .await;
    assert_eq!(
        overflowing["Status"],
        json!("FAILED"),
        "a restored database must still refuse to grow past the engine's storage bound, \
         got {overflowing}"
    );
    // And the restored rows are still there to read.
    let records = query(&router, "bound-dst-wg", "dev", "SELECT n FROM small").await;
    assert_eq!(
        records,
        json!([[{"longValue": 1}]]),
        "the restored rows must survive a statement that hit the bound, got {records}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3 — the namespace the restore answers with, and the one it mints.
// ---------------------------------------------------------------------------
/// One row of the minted-namespace table: the source namespace's own properties, which
/// the snapshot records and the restore has to bring to a namespace that does not exist
/// yet.
#[derive(Debug, Clone, Copy)]
struct MintCase {
    label: &'static str,
    /// `adminUsername` on the source `CreateNamespace`, absent for the default.
    admin_username: Option<&'static str>,
    /// `kmsKeyId` on the source `CreateNamespace`, absent for the AWS owned key.
    kms_key_id: Option<&'static str>,
    /// The `adminUsername` the restored namespace must report.
    expected_admin_username: &'static str,
    /// The `kmsKeyId` the restored namespace must report.
    expected_kms_key_id: &'static str,
}

#[tokio::test]
async fn a_restore_into_an_absent_namespace_mints_it_from_the_snapshot() {
    // The rows differ along the namespace properties the snapshot records: the admin
    // username the namespace had when the snapshot was taken, and its encryption key —
    // including the `AWS_OWNED_KMS_KEY` sentinel a create that names no key reports.
    let cases = [
        MintCase {
            label: "the service-supplied admin username and the AWS owned key",
            admin_username: None,
            kms_key_id: None,
            expected_admin_username: "admin",
            expected_kms_key_id: "AWS_OWNED_KMS_KEY",
        },
        MintCase {
            label: "an admin username and a customer managed key the create named",
            admin_username: Some("dbadmin"),
            kms_key_id: Some(
                "arn:aws:kms:us-east-1:000000000000:key/\
                 11111111-2222-3333-4444-555555555555",
            ),
            expected_admin_username: "dbadmin",
            expected_kms_key_id: "arn:aws:kms:us-east-1:000000000000:key/\
                                  11111111-2222-3333-4444-555555555555",
        },
    ];

    for (index, case) in cases.iter().enumerate() {
        let router = emulator();
        let source_namespace = format!("mint-src-ns-{index}");
        let source_workgroup = format!("mint-src-wg-{index}");
        let minted_namespace = format!("mint-dst-ns-{index}");
        let snapshot_name = format!("mint-snap-{index}");

        let mut create = json!({ "namespaceName": source_namespace });
        if let Some(admin_username) = case.admin_username {
            create["adminUsername"] = json!(admin_username);
        }
        if let Some(kms_key_id) = case.kms_key_id {
            create["kmsKeyId"] = json!(kms_key_id);
        }
        let (status, body) = call(&router, "CreateNamespace", create.clone()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: CreateNamespace {create} expected 200, got {status} with {body}",
            label = case.label
        );
        create_workgroup(&router, &source_workgroup, &source_namespace).await;
        run_sql(
            &router,
            &source_workgroup,
            "dev",
            "CREATE TABLE seeded (n INTEGER)",
        )
        .await;
        run_sql(
            &router,
            &source_workgroup,
            "dev",
            "INSERT INTO seeded (n) VALUES (3)",
        )
        .await;
        create_snapshot(&router, &source_namespace, &snapshot_name).await;

        // The namespace the restore is pointed at does not exist; the compute does.
        let (status, body) = call(
            &router,
            "RestoreFromSnapshot",
            json!({
                "namespaceName": minted_namespace,
                "workgroupName": source_workgroup,
                "snapshotName": snapshot_name,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: RestoreFromSnapshot expected 200, got {status} with {body}",
            label = case.label
        );
        let namespace = body["namespace"].clone();
        assert_eq!(
            namespace["adminUsername"],
            json!(case.expected_admin_username),
            "case {label:?}: expected adminUsername {expected}, got {namespace}",
            label = case.label,
            expected = case.expected_admin_username
        );
        assert_eq!(
            namespace["kmsKeyId"],
            json!(case.expected_kms_key_id),
            "case {label:?}: expected kmsKeyId {expected}, got {namespace}",
            label = case.label,
            expected = case.expected_kms_key_id
        );
        let namespace_id = namespace["namespaceId"].as_str().unwrap_or_default();
        assert!(
            is_uuid(namespace_id),
            "case {label:?}: the minted namespaceId must be a UUID, got {namespace_id:?}",
            label = case.label
        );
        assert_eq!(
            namespace["namespaceArn"],
            json!(format!(
                "arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:namespace/{namespace_id}"
            )),
            "case {label:?}: the minted namespaceArn must name that id, got {namespace}",
            label = case.label
        );
        // A restore is a change to the namespace, so it reports MODIFYING and settles
        // into AVAILABLE on its own.
        assert_eq!(
            namespace["status"],
            json!("MODIFYING"),
            "case {label:?}: a restored namespace reports MODIFYING, got {namespace}",
            label = case.label
        );

        // The round trip: the namespace the restore answered with is the one stored.
        let (status, read_back) = call(
            &router,
            "GetNamespace",
            json!({ "namespaceName": minted_namespace }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: GetNamespace expected 200, got {status} with {read_back}",
            label = case.label
        );
        assert_eq!(
            read_back["namespace"]["namespaceId"], namespace["namespaceId"],
            "case {label:?}: GetNamespace must report the minted namespace, got {read_back}",
            label = case.label
        );
        tokio::time::sleep(LONGER_THAN_SETTLE).await;
        let (_, settled) = call(
            &router,
            "GetNamespace",
            json!({ "namespaceName": minted_namespace }),
        )
        .await;
        assert_eq!(
            settled["namespace"]["status"],
            json!("AVAILABLE"),
            "case {label:?}: the restored namespace settles into AVAILABLE, got {settled}",
            label = case.label
        );

        // And the minted namespace is a namespace like any other: it lists.
        let (_, listed) = call(&router, "ListNamespaces", json!({})).await;
        let names: Vec<String> = listed["namespaces"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|namespace| namespace["namespaceName"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(
            names.contains(&minted_namespace),
            "case {label:?}: ListNamespaces must report the minted namespace, got {names:?}",
            label = case.label
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4 — the refusals, and that each one restores nothing.
// ---------------------------------------------------------------------------

/// One row of the refusal table: how the request is wrong, and the error the operation
/// models for it.
#[derive(Debug, Clone, Copy)]
struct RefusalCase {
    label: &'static str,
    /// The request, built from the fixture's names.
    request: fn(&str, &str, &str, &str) -> Value,
    expected_status: u16,
    expected_code: &'static str,
}

#[tokio::test]
async fn a_restore_that_addresses_nothing_is_refused_and_restores_nothing() {
    // The rows differ along which part of the request resolves to nothing, and along the
    // two ways a request can fail to address ONE snapshot.
    let cases = [
        RefusalCase {
            label: "both snapshotName and snapshotArn",
            request: |namespace, workgroup, snapshot, snapshot_arn| {
                json!({
                    "namespaceName": namespace,
                    "workgroupName": workgroup,
                    "snapshotName": snapshot,
                    "snapshotArn": snapshot_arn,
                })
            },
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            label: "neither snapshotName nor snapshotArn",
            request: |namespace, workgroup, _snapshot, _arn| {
                json!({ "namespaceName": namespace, "workgroupName": workgroup })
            },
            expected_status: 400,
            expected_code: "ValidationException",
        },
        RefusalCase {
            label: "a snapshotName that resolves to nothing",
            request: |namespace, workgroup, _snapshot, _arn| {
                json!({
                    "namespaceName": namespace,
                    "workgroupName": workgroup,
                    "snapshotName": "no-such-snapshot",
                })
            },
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "a snapshotArn that resolves to nothing",
            request: |namespace, workgroup, _snapshot, _arn| {
                json!({
                    "namespaceName": namespace,
                    "workgroupName": workgroup,
                    "snapshotArn": format!(
                        "arn:aws:redshift-serverless:us-east-1:{ACCOUNT}:snapshot/\
                         3f8c1a7e-5b2d-4c6e-9a10-7d4e2f6b8c90"
                    ),
                })
            },
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "an ownerAccount that does not own the snapshot",
            request: |namespace, workgroup, snapshot, _arn| {
                json!({
                    "namespaceName": namespace,
                    "workgroupName": workgroup,
                    "snapshotName": snapshot,
                    "ownerAccount": "999999999999",
                })
            },
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "a workgroupName that resolves to nothing",
            request: |namespace, _workgroup, snapshot, _arn| {
                json!({
                    "namespaceName": namespace,
                    "workgroupName": "no-such-workgroup",
                    "snapshotName": snapshot,
                })
            },
            // A resource NAME that resolves to nothing carries the modelled
            // `ResourceNotFoundException` with HTTP 400, the status this service answers a
            // name refusal with.
            expected_status: 400,
            expected_code: "ResourceNotFoundException",
        },
    ];

    let router = emulator();
    create_namespace(&router, "refusal-src-ns").await;
    create_workgroup(&router, "refusal-src-wg", "refusal-src-ns").await;
    run_sql(
        &router,
        "refusal-src-wg",
        "dev",
        "CREATE TABLE guarded (n INTEGER)",
    )
    .await;
    run_sql(
        &router,
        "refusal-src-wg",
        "dev",
        "INSERT INTO guarded (n) VALUES (1)",
    )
    .await;
    let snapshot = create_snapshot(&router, "refusal-src-ns", "refusal-snap").await;
    let snapshot_arn = snapshot["snapshotArn"].as_str().unwrap_or_default().to_string();

    create_namespace(&router, "refusal-dst-ns").await;
    create_workgroup(&router, "refusal-dst-wg", "refusal-dst-ns").await;

    for case in cases {
        let request = (case.request)(
            "refusal-dst-ns",
            "refusal-dst-wg",
            "refusal-snap",
            &snapshot_arn,
        );
        let (status, body) = call(&router, "RestoreFromSnapshot", request.clone()).await;
        assert_eq!(
            status, case.expected_status,
            "case {label:?}: {request} expected HTTP {expected}, got {status} with {body}",
            label = case.label,
            expected = case.expected_status
        );
        assert_eq!(
            error_code(&body),
            case.expected_code,
            "case {label:?}: expected {expected}, got {body}",
            label = case.label,
            expected = case.expected_code
        );
        // Nothing was restored: the target workgroup still serves no table of its own.
        let attempted = try_sql(
            &router,
            "refusal-dst-wg",
            "dev",
            "SELECT n FROM guarded",
        )
        .await;
        assert_eq!(
            attempted["Status"],
            json!("FAILED"),
            "case {label:?}: a refused restore must move no data, got {attempted}",
            label = case.label
        );
    }
}

#[tokio::test]
async fn a_restore_into_a_namespace_with_a_change_in_flight_conflicts() {
    let router = emulator();
    create_namespace(&router, "conflict-src-ns").await;
    create_workgroup(&router, "conflict-src-wg", "conflict-src-ns").await;
    run_sql(
        &router,
        "conflict-src-wg",
        "dev",
        "CREATE TABLE t (n INTEGER)",
    )
    .await;
    create_snapshot(&router, "conflict-src-ns", "conflict-snap").await;

    create_namespace(&router, "conflict-dst-ns").await;
    create_workgroup(&router, "conflict-dst-wg", "conflict-dst-ns").await;
    // An update puts the namespace in MODIFYING for the span the clock owes.
    let (status, body) = call(
        &router,
        "UpdateNamespace",
        json!({ "namespaceName": "conflict-dst-ns", "logExports": ["userlog"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateNamespace expected 200, got {status} with {body}"
    );

    let request = json!({
        "namespaceName": "conflict-dst-ns",
        "workgroupName": "conflict-dst-wg",
        "snapshotName": "conflict-snap",
    });
    let (status, body) = call(&router, "RestoreFromSnapshot", request.clone()).await;
    assert_eq!(
        status, 409,
        "a restore into a namespace being modified expected HTTP 409, got {status} with \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "expected ConflictException, got {body}"
    );

    // Once the change the clock owed has fallen due, the same request is accepted.
    tokio::time::sleep(LONGER_THAN_SETTLE).await;
    let (status, body) = call(&router, "RestoreFromSnapshot", request).await;
    assert_eq!(
        status, 200,
        "once the namespace is AVAILABLE the restore expected 200, got {status} with {body}"
    );
}
