// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KMS key every operation that encrypts a resource under a
//! CALLER-NAMED key holds to.
//!
//! A `KmsKeyId` / `KMSKeyId` is "the Amazon Web Services KMS key identifier" of the key the
//! resource is encrypted under, and a key cannot encrypt anything unless KMS resolves it and
//! reports it ENABLED — a key this account holds no such key for, and a key it holds in a
//! state that performs no cryptographic operation, are the two halves of the one modelled
//! `KMSKeyNotAccessibleFault` ("An error occurred accessing an Amazon Web Services KMS key",
//! HTTP 400, `Error/Type` `Sender`). So every such operation is refused with that fault, and
//! creates nothing, before it resolves anything the request NAMES.
//!
//! Asserted over a TABLE spanning
//!
//! * every operation of this service that takes a caller-named encryption key and models the
//!   fault — the two creates of a DB instance (`CreateDBInstance`,
//!   `CreateDBInstanceReadReplica`), both snapshot copies (`CopyDBSnapshot`,
//!   `CopyDBClusterSnapshot`), the custom engine version (`CreateCustomDBEngineVersion`) and
//!   the zero-ETL integration (`CreateIntegration`) — which between them spell the member
//!   both ways the model spells it (`KmsKeyId` and `KMSKeyId`);
//! * crossed with every form the member accepts ("the key ARN, key ID, alias ARN, or alias
//!   name for the KMS key") naming no key of this account, and a key KMS HOLDS but has
//!   disabled, named both by its key id and by its ARN.
//!
//! Each row's `<Message>` is asserted in full: a create is refused about the key it
//! SPECIFIED, and a copy — which also has the source snapshot's key — about its TARGET key.
//!
//! A control suite then pins the other side: under a key KMS holds ENABLED, not one of the
//! same calls is refused with that fault, and an encrypted DB instance and an encrypted
//! snapshot copy created under it are readable back through their own describes.
//!
//! Every call goes over the wire as a real awsQuery form POST, and KMS is mounted beside
//! this service on ONE engine exactly as the assembled router mounts them, so the key each
//! row names is resolved against the service that OWNS the answer.

use crate::db_instances::{
    call, expect_error, expect_ok, optional_text, text, texts, wait_for_settle, REGION,
};
use crate::rds_s3_integration::{available_cluster, available_instance, emulator, kms};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The DB instance every row that needs a source instance reads.
const SOURCE_INSTANCE: &str = "emu-kms-source-db";

/// The DB snapshot every `CopyDBSnapshot` row copies.
const SOURCE_SNAPSHOT: &str = "emu-kms-source-snap";

/// The DB cluster the integration rows name as their source.
const SOURCE_CLUSTER: &str = "emu-kms-source-cluster";

/// The DB cluster snapshot every `CopyDBClusterSnapshot` row copies.
const SOURCE_CLUSTER_SNAPSHOT: &str = "emu-kms-source-cluster-snap";

/// An Amazon Redshift namespace ARN, the kind of target `CreateIntegration$TargetArn` takes.
const REDSHIFT_TARGET_ARN: &str =
    "arn:aws:redshift-serverless:us-west-2:000000000000:namespace/zero-etl";

/// The engine family the custom-engine-version rows create a CEV of, and the name form that
/// engine documents for one (`CreateCustomDBEngineVersion$EngineVersion`:
/// *major*.*minor*.*customized_string* for the Microsoft SQL Server engines).
const CEV_ENGINE: &str = "sqlserver-dev-ee";

/// The sentence a CREATE answers `KMSKeyNotAccessibleFault` with, spelled out here so the
/// wording is pinned by the test rather than by the code under test.
fn key_not_accessible(key_id: &str) -> String {
    format!(
        "The specified KMS key [{key_id}] does not exist, is not enabled or you do not have \
         permissions to access it."
    )
}

/// The same sentence on a COPY, whose named key is the key the COPY is encrypted under —
/// the TARGET key, beside the source snapshot's own.
fn target_key_not_accessible(key_id: &str) -> String {
    format!(
        "The target KMS key [{key_id}] does not exist, is not enabled or you do not have \
         permissions to access it."
    )
}

/// The ARN form of a key this emulator's own KMS holds.
fn key_arn(key_id: &str) -> String {
    format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{key_id}")
}

/// Assert the status, the wire error `<Code>`, the exact `<Message>` and the `Sender` class
/// of one refusal.
fn expect_refusal(case: &str, response: (u16, String), status: u16, code: &str, message: &str) {
    let body = expect_error(case, response, status, code);
    let answered = text(&body, "Message");
    assert_eq!(
        answered, message,
        "case {case:?}: expected the message {message:?}, got {answered:?} in {body}"
    );
    let kind = text(&body, "Type");
    assert_eq!(
        kind, "Sender",
        "case {case:?}: expected a Sender fault, got {kind:?} in {body}"
    );
}

/// The fixture these tables run against: the router, the sources the rows read, and the two
/// keys KMS really holds — one enabled, one disabled.
struct Fixture {
    router: MultiServiceRouter,
    enabled_key: String,
    disabled_key: String,
}

/// One key this emulator's own KMS holds, reported by its key id.
async fn created_key(router: &MultiServiceRouter, description: &str) -> String {
    kms(router)
        .create_key()
        .description(description)
        .send()
        .await
        .expect("the emulated KMS creates the key")
        .key_metadata()
        .map(|metadata| metadata.key_id().to_string())
        .expect("the created key reports its id")
}

/// Stand the fixture up: the sources every row resolves against, so no refusal below can
/// come from a source that does not exist, and the two keys KMS holds.
async fn fixture() -> Fixture {
    let (router, _engine) = emulator();
    available_instance(&router, SOURCE_INSTANCE, "mysql").await;
    expect_ok(
        "snapshot the source instance",
        call(
            &router,
            "CreateDBSnapshot",
            &[
                ("DBInstanceIdentifier", SOURCE_INSTANCE),
                ("DBSnapshotIdentifier", SOURCE_SNAPSHOT),
            ],
        )
        .await,
    );
    available_cluster(&router, SOURCE_CLUSTER, "aurora-postgresql").await;
    expect_ok(
        "snapshot the source cluster",
        call(
            &router,
            "CreateDBClusterSnapshot",
            &[
                ("DBClusterIdentifier", SOURCE_CLUSTER),
                ("DBClusterSnapshotIdentifier", SOURCE_CLUSTER_SNAPSHOT),
            ],
        )
        .await,
    );
    // Both snapshots are answered in a status the clock owes `available`, and a copy reads
    // one that holds data: the fixture waits that span out once, for both.
    wait_for_settle();

    let enabled_key = created_key(&router, "the encryption key this account holds").await;
    let disabled_key = created_key(&router, "the encryption key this account disabled").await;
    kms(&router)
        .disable_key()
        .key_id(&disabled_key)
        .send()
        .await
        .expect("the emulated KMS disables the key");
    Fixture {
        router,
        enabled_key,
        disabled_key,
    }
}

/// One key a row names: how the request spells it, and the identifier itself.
struct KeyRow {
    case: String,
    key_id: String,
}

/// Every key no resource can be encrypted under: the three forms the member accepts naming
/// no key of this account, and a key KMS HOLDS but has disabled, in both the forms that name
/// one key.
fn inaccessible_keys(fixture: &Fixture) -> Vec<KeyRow> {
    vec![
        KeyRow {
            case: "a key ARN KMS holds nothing under".to_string(),
            key_id: key_arn("11111111-2222-3333-4444-555555555555"),
        },
        KeyRow {
            case: "a bare key id KMS holds nothing under".to_string(),
            key_id: "22222222-3333-4444-5555-666666666666".to_string(),
        },
        KeyRow {
            case: "an alias this account holds no key under".to_string(),
            key_id: "alias/emu-absent-encryption-key".to_string(),
        },
        KeyRow {
            case: "a key KMS holds disabled, by its key id".to_string(),
            key_id: fixture.disabled_key.clone(),
        },
        KeyRow {
            case: "the same disabled key, by its ARN".to_string(),
            key_id: key_arn(&fixture.disabled_key),
        },
    ]
}

/// The two forms that name the one key KMS holds ENABLED, for the control suite.
fn accessible_keys(fixture: &Fixture) -> Vec<KeyRow> {
    vec![
        KeyRow {
            case: "an enabled key, by its key id".to_string(),
            key_id: fixture.enabled_key.clone(),
        },
        KeyRow {
            case: "the same enabled key, by its ARN".to_string(),
            key_id: key_arn(&fixture.enabled_key),
        },
    ]
}

/// One operation of the table: what it is called, the member that names its encryption key,
/// the resource the call would create, and the rest of its request.
struct OperationRow {
    case: &'static str,
    action: &'static str,
    /// `KmsKeyId` on the instance and snapshot paths, `KMSKeyId` on the custom engine
    /// version and the integration — the two spellings the model uses.
    key_member: &'static str,
    /// Whether the key this operation names is the TARGET key of a copy, which Amazon RDS
    /// words its refusal about, rather than the one key a create specifies.
    target_key: bool,
    /// The name the call would create its resource under, and the describe it would join:
    /// what the "nothing was created" assertion reads back.
    created: Created,
    params: Vec<(&'static str, String)>,
}

/// The resource one row would create: the describe that reports it, the member that names it
/// there, and the name this row asked for.
struct Created {
    describe: &'static str,
    member: &'static str,
    name: String,
}

/// Every operation of this service that encrypts what it creates under a caller-named key
/// and models `KMSKeyNotAccessibleFault`, with the rest of the request it needs to be served.
///
/// `tag` distinguishes the resources one pass over the table would create from another's, so
/// no row is ever refused for a name a previous row already took.
fn operation_rows(tag: &str) -> Vec<OperationRow> {
    let instance = format!("emu-kms-inst-{tag}");
    let replica = format!("emu-kms-replica-{tag}");
    let snapshot_copy = format!("emu-kms-snap-copy-{tag}");
    let cluster_snapshot_copy = format!("emu-kms-cluster-snap-copy-{tag}");
    let cev_version = format!("15.00.4385.2.kms_cev{tag}");
    let integration = format!("emu-kms-integration-{tag}");
    vec![
        OperationRow {
            case: "CreateDBInstance with storage encryption turned on",
            action: "CreateDBInstance",
            key_member: "KmsKeyId",
            target_key: false,
            created: Created {
                describe: "DescribeDBInstances",
                member: "DBInstanceIdentifier",
                name: instance.clone(),
            },
            params: vec![
                ("DBInstanceIdentifier", instance),
                ("Engine", "mysql".to_string()),
                ("DBInstanceClass", "db.t3.micro".to_string()),
                ("AllocatedStorage", "20".to_string()),
                ("StorageEncrypted", "true".to_string()),
            ],
        },
        OperationRow {
            case: "CreateDBInstanceReadReplica of an encrypted replica",
            action: "CreateDBInstanceReadReplica",
            key_member: "KmsKeyId",
            target_key: false,
            created: Created {
                describe: "DescribeDBInstances",
                member: "DBInstanceIdentifier",
                name: replica.clone(),
            },
            params: vec![
                ("DBInstanceIdentifier", replica),
                ("SourceDBInstanceIdentifier", SOURCE_INSTANCE.to_string()),
            ],
        },
        OperationRow {
            case: "CopyDBSnapshot under a key of the caller's",
            action: "CopyDBSnapshot",
            key_member: "KmsKeyId",
            target_key: true,
            created: Created {
                describe: "DescribeDBSnapshots",
                member: "DBSnapshotIdentifier",
                name: snapshot_copy.clone(),
            },
            params: vec![
                ("SourceDBSnapshotIdentifier", SOURCE_SNAPSHOT.to_string()),
                ("TargetDBSnapshotIdentifier", snapshot_copy),
            ],
        },
        OperationRow {
            case: "CopyDBClusterSnapshot under a key of the caller's",
            action: "CopyDBClusterSnapshot",
            key_member: "KmsKeyId",
            target_key: true,
            created: Created {
                describe: "DescribeDBClusterSnapshots",
                member: "DBClusterSnapshotIdentifier",
                name: cluster_snapshot_copy.clone(),
            },
            params: vec![
                (
                    "SourceDBClusterSnapshotIdentifier",
                    SOURCE_CLUSTER_SNAPSHOT.to_string(),
                ),
                (
                    "TargetDBClusterSnapshotIdentifier",
                    cluster_snapshot_copy,
                ),
            ],
        },
        OperationRow {
            case: "CreateCustomDBEngineVersion, whose media the key encrypts",
            action: "CreateCustomDBEngineVersion",
            key_member: "KMSKeyId",
            target_key: false,
            created: Created {
                describe: "DescribeDBEngineVersions",
                member: "EngineVersion",
                name: cev_version.clone(),
            },
            params: vec![
                ("Engine", CEV_ENGINE.to_string()),
                ("EngineVersion", cev_version),
            ],
        },
        OperationRow {
            case: "CreateIntegration, whose data the key encrypts",
            action: "CreateIntegration",
            key_member: "KMSKeyId",
            target_key: false,
            created: Created {
                describe: "DescribeIntegrations",
                member: "IntegrationName",
                name: integration.clone(),
            },
            params: vec![
                ("IntegrationName", integration),
                (
                    "SourceArn",
                    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{SOURCE_CLUSTER}"),
                ),
                ("TargetArn", REDSHIFT_TARGET_ARN.to_string()),
            ],
        },
    ]
}

/// Whether `name` is reported by the describe that would hold the resource a row asked for.
async fn is_recorded(router: &MultiServiceRouter, created: &Created) -> bool {
    let no_params: [(&str, &str); 0] = [];
    let body = expect_ok(created.describe, call(router, created.describe, &no_params).await);
    texts(&body, created.member).iter().any(|held| held == &created.name)
}

#[tokio::test]
async fn a_kms_key_that_cannot_encrypt_refuses_every_create_that_names_it() {
    let fixture = fixture().await;
    let mut refused: Vec<Created> = Vec::new();

    for (key_index, key_row) in inaccessible_keys(&fixture).into_iter().enumerate() {
        for row in operation_rows(&format!("r{key_index}")) {
            let case = format!("{} with {}", row.case, key_row.case);
            let mut params = row.params.clone();
            params.push((row.key_member, key_row.key_id.clone()));
            let expected = if row.target_key {
                target_key_not_accessible(&key_row.key_id)
            } else {
                key_not_accessible(&key_row.key_id)
            };
            expect_refusal(
                &case,
                call(&fixture.router, row.action, &params).await,
                400,
                "KMSKeyNotAccessibleFault",
                &expected,
            );
            refused.push(row.created);
        }
    }

    // A refused create leaves nothing behind: the refusal is a decision about the request,
    // answered before the resource is minted, so none of the names the rows asked for is in
    // the report that would hold it.
    for created in &refused {
        assert!(
            !is_recorded(&fixture.router, created).await,
            "case {:?}: a refused create records nothing, but {} reports it",
            created.name,
            created.describe
        );
    }
}

#[tokio::test]
async fn a_kms_key_this_account_holds_enabled_refuses_none_of_them() {
    let fixture = fixture().await;
    for (key_index, key_row) in accessible_keys(&fixture).into_iter().enumerate() {
        for row in operation_rows(&format!("ok{key_index}")) {
            let case = format!("{} with {}", row.case, key_row.case);
            let mut params = row.params.clone();
            params.push((row.key_member, key_row.key_id.clone()));
            let (status, body) = call(&fixture.router, row.action, &params).await;
            if status == 200 {
                continue;
            }
            // Whatever else a row may still be refused for, the key is not it: this one is
            // a key KMS resolves and reports enabled.
            let code = text(&body, "Code");
            assert_ne!(
                code, "KMSKeyNotAccessibleFault",
                "case {case:?}: an enabled key is accessible, got {code:?} in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_resource_encrypted_under_an_enabled_key_reports_that_key_when_read_back() {
    let fixture = fixture().await;
    let named = key_arn(&fixture.enabled_key);

    // The create the issue's own call makes: an encrypted DB instance under a key KMS holds.
    let identifier = "emu-kms-encrypted-instance";
    let created = expect_ok(
        "create an encrypted DB instance under an enabled key",
        call(
            &fixture.router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.t3.micro"),
                ("AllocatedStorage", "20"),
                ("StorageEncrypted", "true"),
                ("KmsKeyId", named.as_str()),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&created, "StorageEncrypted"),
        "true",
        "the created instance is encrypted: {created}"
    );
    assert_eq!(
        text(&created, "KmsKeyId"),
        named,
        "the created instance reports the key it was encrypted under: {created}"
    );

    // Read back through the describe: the key is a fact of the stored instance, not of the
    // response the create happened to build.
    let described = expect_ok(
        "read the encrypted instance back",
        call(
            &fixture.router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier)],
        )
        .await,
    );
    assert_eq!(
        text(&described, "KmsKeyId"),
        named,
        "the stored instance reports the key: {described}"
    );
    assert_eq!(
        text(&described, "StorageEncrypted"),
        "true",
        "the stored instance is encrypted: {described}"
    );

    // And the copy path the issue names too: a snapshot copy under the same key.
    let copy = "emu-kms-encrypted-copy";
    let copied = expect_ok(
        "copy a snapshot under an enabled key",
        call(
            &fixture.router,
            "CopyDBSnapshot",
            &[
                ("SourceDBSnapshotIdentifier", SOURCE_SNAPSHOT),
                ("TargetDBSnapshotIdentifier", copy),
                ("KmsKeyId", named.as_str()),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&copied, "Encrypted"),
        "true",
        "a copy under a named key is encrypted: {copied}"
    );
    assert_eq!(
        optional_text(&copied, "KmsKeyId").as_deref(),
        Some(named.as_str()),
        "the copy reports the key it was encrypted under: {copied}"
    );
}
