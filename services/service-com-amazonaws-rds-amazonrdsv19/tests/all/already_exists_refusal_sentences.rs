// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SENTENCE each conflict, reserved-name and in-use refusal of
//! this service carries.
//!
//! The rule under test: a refusal answers the sentence Amazon RDS words for that fault
//! class — its fixed text exactly, plus only the variable parts the service itself puts in
//! it, taken from the request — rather than one house-style template built from the
//! resource type and the identifier the request carried. Concretely, over the fault classes
//! this suite spans:
//!
//! * `DBSecurityGroupAlreadyExistsFault` (400, `awsQueryError` code
//!   `DBSecurityGroupAlreadyExists` —
//!   `smithy_query(query="operation", operation="CreateDBSecurityGroup")`) is a sentence
//!   that NAMES the requested group, unquoted, INSIDE it, in the folded spelling that
//!   member documents it is stored under.
//! * `GlobalClusterAlreadyExistsFault` (400) is a sentence that NAMES the requested global
//!   cluster identifier, unquoted, inside it, likewise folded.
//! * `DBInstanceAlreadyExistsFault` and `DBClusterAlreadyExistsFault` (both 400) are FIXED
//!   sentences with NO variable part: neither the identifier the request carried nor the
//!   one the region holds appears, and every operation that answers the fault — the create
//!   that names a taken identifier, and the modification that renames onto one — answers
//!   the same sentence.
//! * `InvalidOptionGroupStateFault` (400) for a group a DB instance is using is a sentence
//!   that names the requested group SINGLE-QUOTED and reports nothing of this service's
//!   state: the instance holding the group decides the refusal but is not disclosed by it.
//! * `InvalidParameterValue` (400) for a DB security group create that asks for the name
//!   the service keeps for itself is a sentence about what cannot be named, carrying that
//!   reserved name and nothing else of the request.
//!
//! Each table spans what the rule generalises over: the shape of the identifier the request
//! carried (plain, hyphen-bearing, digit-bearing, a single letter, one at the published
//! length bound, the punctuation the member admits), the case it was sent in against the
//! case it is stored in, the region it was created in, the engine the resource runs, and —
//! for the two fixed sentences — which operation answered (create or rename). The recorded
//! duplicate create is one row of each.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each
//! assertion reads the XML the emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, text, wait_for_settle, REGION,
};

/// A second region, so a conflict in a region other than the default is a row of each
/// table and the sentence is shown to be the same there.
const OTHER_REGION: &str = "eu-central-1";

/// The longest `DBSecurityGroupName` the member admits: "Must be 1 to 255 letters,
/// numbers, or hyphens" (`CreateDBSecurityGroup$DBSecurityGroupName`,
/// `smithy_query(query="doc", shape="CreateDBSecurityGroupMessage",
/// member="DBSecurityGroupName")`).
const MAX_DB_SECURITY_GROUP_NAME_LEN: usize = 255;

/// The longest `GlobalClusterIdentifier` a global database may carry: the "1 to 63 letters,
/// numbers, or hyphens" the API publishes for the identifier of a global database
/// (`smithy_query(query="doc", shape="ModifyGlobalClusterMessage",
/// member="NewGlobalClusterIdentifier")`, the member that writes that rule down), which is
/// narrower than the `length 1..255` the modelled shape carries.
const MAX_GLOBAL_CLUSTER_IDENTIFIER_LEN: usize = 63;

/// The FIXED sentence a `DBInstanceAlreadyExistsFault` carries, whichever operation
/// answered it.
const DB_INSTANCE_ALREADY_EXISTS: &str = "DB instance already exists";

/// The FIXED sentence a `DBClusterAlreadyExistsFault` carries, whichever operation
/// answered it.
const DB_CLUSTER_ALREADY_EXISTS: &str = "DB Cluster already exists";

/// The sentence the DB security group conflict carries for the group `stored_as`.
fn security_group_sentence(stored_as: &str) -> String {
    format!("A security group named {stored_as} already exists")
}

/// The sentence the global cluster conflict carries for the identifier `stored_as`.
fn global_cluster_sentence(stored_as: &str) -> String {
    format!("Global cluster {stored_as} already exists")
}

/// The sentence the in-use option group refusal carries for the requested `name`.
fn option_group_in_use_sentence(name: &str) -> String {
    format!("The option group '{name}' cannot be deleted because it is in use.")
}

/// The sentence a DB security group create that asks for the reserved name is refused with.
const RESERVED_DB_SECURITY_GROUP_SENTENCE: &str = "You cannot name a DB security group default";

/// A name of exactly `len` characters that satisfies the identifier rule both families
/// publish (first character a letter, then letters, numbers or single hyphens).
fn name_at_length(len: usize) -> String {
    let mut name = String::from("e");
    name.extend(std::iter::repeat_n('a', len - 1));
    assert_eq!(
        name.chars().count(),
        len,
        "the row at the published bound must be {len} characters, got {}",
        name.chars().count()
    );
    name
}

/// `CreateDBSecurityGroup` in `region`.
async fn create_security_group(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateDBSecurityGroup",
        &[
            ("DBSecurityGroupName", name),
            ("DBSecurityGroupDescription", "conflict-sentence-group"),
        ],
    )
    .await
}

/// `CreateGlobalCluster` in `region`, on `engine`.
async fn create_global_cluster(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    engine: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateGlobalCluster",
        &[
            ("GlobalClusterIdentifier", identifier),
            ("Engine", engine),
        ],
    )
    .await
}

/// `CreateDBInstance` in `region`, on `engine` and `class`.
async fn create_instance(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    engine: &str,
    class: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier", identifier),
            ("Engine", engine),
            ("DBInstanceClass", class),
        ],
    )
    .await
}

/// The four settings a Multi-AZ DB cluster is required to be created with
/// (`AllocatedStorage`, `DBClusterInstanceClass`, `Iops` and `StorageType`, each documented
/// "This setting is required to create a Multi-AZ DB cluster"), with an `Iops` inside the
/// documented ratio of the allocated storage.
const MULTI_AZ_SETTINGS: &[(&str, &str)] = &[
    ("AllocatedStorage", "400"),
    ("DBClusterInstanceClass", "db.m5d.large"),
    ("Iops", "3000"),
    ("StorageType", "gp3"),
];

/// `CreateDBCluster` in `region`, on `engine`, carrying `extra` beyond the two members
/// every cluster create needs.
async fn create_cluster(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![
        ("DBClusterIdentifier", identifier),
        ("Engine", engine),
    ];
    params.extend_from_slice(extra);
    call_in(router, region, "CreateDBCluster", &params).await
}

/// One duplicate-create row of a family whose sentence NAMES the requested resource.
struct NamedConflict {
    case: &'static str,
    region: &'static str,
    /// The spelling the first create sent.
    created_as: String,
    /// The spelling the second create sends, which can differ in case.
    requested_as: String,
    /// The spelling the sentence has to carry: the folded one the resource is stored
    /// under.
    stored_as: String,
}

impl NamedConflict {
    /// A row whose two creates send the same spelling.
    fn same(case: &'static str, region: &'static str, name: &str) -> Self {
        Self {
            case,
            region,
            created_as: name.to_string(),
            requested_as: name.to_string(),
            stored_as: name.to_ascii_lowercase(),
        }
    }

    /// A row whose second create sends `requested_as` for a resource created as
    /// `created_as`.
    fn recased(
        case: &'static str,
        region: &'static str,
        created_as: &str,
        requested_as: &str,
    ) -> Self {
        Self {
            case,
            region,
            created_as: created_as.to_string(),
            requested_as: requested_as.to_string(),
            stored_as: created_as.to_ascii_lowercase(),
        }
    }
}

#[tokio::test]
async fn a_duplicate_db_security_group_name_is_named_inside_the_sentence() {
    let router = emulator();

    let table = [
        NamedConflict::same("a plain hyphen-bearing name", REGION, "emu-dup-sg"),
        NamedConflict::same("a name carrying digits", REGION, "emu-dup-sg-2026"),
        NamedConflict::same("a single letter", REGION, "e"),
        NamedConflict::same(
            "a name at the published length bound",
            REGION,
            &name_at_length(MAX_DB_SECURITY_GROUP_NAME_LEN),
        ),
        NamedConflict::recased(
            "a name created in mixed case and requested in another",
            REGION,
            "Emu-Dup-SG-Mixed",
            "EMU-DUP-sg-MIXED",
        ),
        NamedConflict::same("a group another region holds", OTHER_REGION, "emu-dup-sg"),
    ];

    for row in &table {
        expect_ok(
            row.case,
            create_security_group(&router, row.region, &row.created_as).await,
        );
        let refused = expect_error(
            row.case,
            create_security_group(&router, row.region, &row.requested_as).await,
            400,
            "DBSecurityGroupAlreadyExists",
        );
        let message = text(&refused, "Message");
        assert_eq!(
            message,
            security_group_sentence(&row.stored_as),
            "case {:?}: expected the sentence naming the group, got {message:?}",
            row.case
        );

        // The refused create changed nothing: the group the first create persisted still
        // reads back through the report, with the description that create gave it.
        let described = expect_ok(
            row.case,
            call_in(
                &router,
                row.region,
                "DescribeDBSecurityGroups",
                &[("DBSecurityGroupName", row.stored_as.as_str())],
            )
            .await,
        );
        assert_eq!(
            (
                text(&described, "DBSecurityGroupName"),
                text(&described, "DBSecurityGroupDescription"),
            ),
            (
                row.stored_as.clone(),
                "conflict-sentence-group".to_string()
            ),
            "case {:?}: expected the refused create to leave the group as it was, got \
             {described}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_duplicate_global_cluster_identifier_is_named_inside_the_sentence() {
    // The two engines a global cluster may run (`CreateGlobalCluster$Engine`), so the
    // sentence is shown not to depend on the engine.
    let table = [
        (
            NamedConflict::same("a plain hyphen-bearing identifier", REGION, "emu-dup-gc"),
            "aurora-mysql",
        ),
        (
            NamedConflict::same(
                // An Amazon RDS identifier is letters, digits and hyphens; the `.`, `_` and
                // `:` the model's own `@pattern` would admit are refused by the rule the API
                // publishes for an identifier, so the longest punctuation an identifier can
                // carry is the hyphen.
                "an identifier carrying the punctuation an identifier admits",
                REGION,
                "emu-dup-gc-2026-a",
            ),
            "aurora-postgresql",
        ),
        (
            NamedConflict::same("a single letter", REGION, "e"),
            "aurora-mysql",
        ),
        (
            NamedConflict::same(
                "an identifier at the published length bound",
                REGION,
                &name_at_length(MAX_GLOBAL_CLUSTER_IDENTIFIER_LEN),
            ),
            "aurora-postgresql",
        ),
        (
            NamedConflict::recased(
                "an identifier created in mixed case and requested in another",
                REGION,
                "Emu-Dup-GC-Mixed",
                "EMU-DUP-gc-MIXED",
            ),
            "aurora-mysql",
        ),
        (
            // A global cluster is account-scoped, so one created against another region's
            // request is still a conflict for a request signed for this one.
            NamedConflict::same(
                "an identifier created through another region's request",
                OTHER_REGION,
                "emu-dup-gc-elsewhere",
            ),
            "aurora-mysql",
        ),
    ];

    for (row, engine) in &table {
        // A global cluster is account-scoped and the account's allowance is small, so each
        // row gets its own store rather than being refused for the quota instead of for
        // the conflict under test.
        let router = emulator();
        expect_ok(
            row.case,
            create_global_cluster(&router, row.region, &row.created_as, engine).await,
        );
        let refused = expect_error(
            row.case,
            create_global_cluster(&router, row.region, &row.requested_as, engine).await,
            400,
            "GlobalClusterAlreadyExistsFault",
        );
        let message = text(&refused, "Message");
        assert_eq!(
            message,
            global_cluster_sentence(&row.stored_as),
            "case {:?}: expected the sentence naming the global cluster, got {message:?}",
            row.case
        );

        // The refused create changed nothing: the record the first create persisted still
        // reads back, on the engine that create named.
        let described = expect_ok(
            row.case,
            call_in(
                &router,
                row.region,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", row.stored_as.as_str())],
            )
            .await,
        );
        assert_eq!(
            (
                text(&described, "GlobalClusterIdentifier"),
                text(&described, "Engine"),
            ),
            (row.stored_as.clone(), (*engine).to_string()),
            "case {:?}: expected the refused create to leave the global cluster as it was, \
             got {described}",
            row.case
        );
    }
}

#[tokio::test]
async fn every_db_instance_conflict_answers_one_fixed_sentence() {
    let router = emulator();

    // `(case, the identifier the instance is created under, the engine, the class)`. The
    // rows differ along the identifier's shape and case, the engine and the instance class,
    // none of which the sentence may depend on.
    let table: Vec<(&str, String, &str, &str)> = vec![
        (
            "a plain hyphen-bearing identifier",
            "emu-dup-db".to_string(),
            "postgres",
            "db.t3.micro",
        ),
        (
            "an identifier carrying digits",
            "emu-dup-db-2026".to_string(),
            "mysql",
            "db.m5.large",
        ),
        ("a single letter", "e".to_string(), "mariadb", "db.t3.small"),
        (
            "an identifier at the length bound the member publishes",
            // "Must contain from 1 to 63 letters, numbers, or hyphens"
            // (`CreateDBInstance$DBInstanceIdentifier`).
            name_at_length(63),
            "postgres",
            "db.t3.micro",
        ),
        (
            "an identifier created in mixed case",
            "Emu-Dup-DB-Mixed".to_string(),
            "mysql",
            "db.t3.micro",
        ),
    ];

    for (case, identifier, engine, class) in &table {
        expect_ok(
            case,
            create_instance(&router, REGION, identifier, engine, class).await,
        );
        // Sent in the spelling the create used, and in the other case: the sentence is the
        // same fixed text either way, because it carries nothing of the request.
        for requested in [identifier.clone(), identifier.to_ascii_uppercase()] {
            let refused = expect_error(
                case,
                create_instance(&router, REGION, &requested, engine, class).await,
                400,
                "DBInstanceAlreadyExists",
            );
            let message = text(&refused, "Message");
            assert_eq!(
                message, DB_INSTANCE_ALREADY_EXISTS,
                "case {case:?}: expected the fixed sentence for {requested:?}, got {message:?}"
            );
        }
    }

    // The RENAME onto a taken identifier answers the same fault, so it answers the same
    // sentence: one fault class, one wording, whichever operation reached it.
    wait_for_settle();
    let renamed = expect_error(
        "a rename onto an identifier the region holds",
        call(
            &router,
            "ModifyDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-dup-db"),
                ("NewDBInstanceIdentifier", "emu-dup-db-2026"),
            ],
        )
        .await,
        400,
        "DBInstanceAlreadyExists",
    );
    let message = text(&renamed, "Message");
    assert_eq!(
        message, DB_INSTANCE_ALREADY_EXISTS,
        "case \"a rename onto an identifier the region holds\": expected the fixed sentence, \
         got {message:?}"
    );

    // The refused calls left every instance where it was.
    for (_, identifier, _, _) in &table {
        let described = expect_ok(
            "the instance after the refusals",
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier.as_str())],
            )
            .await,
        );
        assert_eq!(
            text(&described, "DBInstanceIdentifier"),
            identifier.to_ascii_lowercase(),
            "expected the refused calls to leave the instance as it was, got {described}"
        );
    }
}

#[tokio::test]
async fn every_db_cluster_conflict_answers_one_fixed_sentence() {
    let router = emulator();

    // `(case, the identifier the cluster is created under, the engine)`. The rows differ
    // along the identifier's shape and case and along the engine — including a multi-AZ DB
    // cluster engine beside the Aurora ones — none of which the sentence may depend on.
    let table: Vec<(&str, String, &str)> = vec![
        (
            "a plain hyphen-bearing identifier",
            "emu-dup-cluster".to_string(),
            "aurora-mysql",
        ),
        (
            "an identifier carrying digits",
            "emu-dup-cluster-2026".to_string(),
            "aurora-postgresql",
        ),
        ("a single letter", "e".to_string(), "aurora-mysql"),
        (
            "a multi-AZ DB cluster engine",
            "emu-dup-cluster-mysql".to_string(),
            "mysql",
        ),
        (
            "a multi-AZ DB cluster engine, another identifier shape",
            "emu-dup-cluster-postgres-2".to_string(),
            "postgres",
        ),
        (
            "an identifier created in mixed case",
            "Emu-Dup-Cluster-Mixed".to_string(),
            "aurora-postgresql",
        ),
    ];

    for (case, identifier, engine) in &table {
        // A Multi-AZ DB cluster engine needs the four settings its members document as
        // required; an Aurora engine takes none of them.
        let extra: &[(&str, &str)] = if *engine == "aurora-mysql" || *engine == "aurora-postgresql"
        {
            &[]
        } else {
            MULTI_AZ_SETTINGS
        };
        expect_ok(
            case,
            create_cluster(&router, REGION, identifier, engine, extra).await,
        );
        for requested in [identifier.clone(), identifier.to_ascii_uppercase()] {
            let refused = expect_error(
                case,
                create_cluster(&router, REGION, &requested, engine, extra).await,
                400,
                "DBClusterAlreadyExistsFault",
            );
            let message = text(&refused, "Message");
            assert_eq!(
                message, DB_CLUSTER_ALREADY_EXISTS,
                "case {case:?}: expected the fixed sentence for {requested:?}, got {message:?}"
            );
        }
    }

    // The rename onto a taken identifier answers the same fault class, so it answers the
    // same sentence.
    wait_for_settle();
    let renamed = expect_error(
        "a rename onto an identifier the region holds",
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", "emu-dup-cluster"),
                ("NewDBClusterIdentifier", "emu-dup-cluster-2026"),
            ],
        )
        .await,
        400,
        "DBClusterAlreadyExistsFault",
    );
    let message = text(&renamed, "Message");
    assert_eq!(
        message, DB_CLUSTER_ALREADY_EXISTS,
        "case \"a rename onto an identifier the region holds\": expected the fixed sentence, \
         got {message:?}"
    );

    // The refused calls left every cluster where it was.
    for (_, identifier, engine) in &table {
        let described = expect_ok(
            "the cluster after the refusals",
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", identifier.as_str())],
            )
            .await,
        );
        assert_eq!(
            (
                text(&described, "DBClusterIdentifier"),
                text(&described, "Engine"),
            ),
            (identifier.to_ascii_lowercase(), (*engine).to_string()),
            "expected the refused calls to leave the cluster as it was, got {described}"
        );
    }
}

#[tokio::test]
async fn an_option_group_in_use_names_only_the_group_the_request_asked_to_delete() {
    let router = emulator();

    // `(case, the group's name, its engine, its major engine version, the instance using
    // it)`. The rows differ along the group's name shape, the engine and major-version
    // form, and the instance identifier — which the sentence must NOT carry.
    let table: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "a mysql group a plainly named instance uses",
            "emu-inuse-og",
            "mysql",
            "8.0",
            "emu-inuse-db",
        ),
        (
            "a mariadb group, hyphen-bearing name",
            "emu-inuse-og-mariadb",
            "mariadb",
            "10.11",
            "emu-inuse-db-mariadb",
        ),
        (
            "an oracle group, two-digit major version",
            "emu-inuse-og-oracle",
            "oracle-se2",
            "19",
            "emu-inuse-db-oracle",
        ),
        (
            "a sqlserver group, four-part major version",
            "emu-inuse-og-sqlserver",
            "sqlserver-ex",
            "15.00",
            "emu-inuse-db-sqlserver",
        ),
    ];

    for &(case, group, engine, major_version, holder) in &table {
        expect_ok(
            case,
            call(
                &router,
                "CreateOptionGroup",
                &[
                    ("OptionGroupName", group),
                    ("EngineName", engine),
                    ("MajorEngineVersion", major_version),
                    ("OptionGroupDescription", "conflict-sentence-options"),
                ],
            )
            .await,
        );
        expect_ok(
            case,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", holder),
                    ("Engine", engine),
                    ("DBInstanceClass", "db.m5.large"),
                    ("OptionGroupName", group),
                ],
            )
            .await,
        );

        let refused = expect_error(
            case,
            call(&router, "DeleteOptionGroup", &[("OptionGroupName", group)]).await,
            400,
            "InvalidOptionGroupStateFault",
        );
        let message = text(&refused, "Message");
        assert_eq!(
            message,
            option_group_in_use_sentence(group),
            "case {case:?}: expected the in-use sentence quoting the group, got {message:?}"
        );
        assert!(
            !message.contains(holder),
            "case {case:?}: the sentence must not disclose the instance using the group, got \
             {message:?}"
        );

        // The refused delete left the group exactly where it was.
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeOptionGroups",
                &[("OptionGroupName", group)],
            )
            .await,
        );
        assert_eq!(
            text(&described, "OptionGroupName"),
            group,
            "case {case:?}: expected the refused delete to change nothing, got {described}"
        );
    }
}

#[tokio::test]
async fn the_reserved_db_security_group_name_is_refused_with_one_sentence_whatever_its_case() {
    let router = emulator();

    // The member states the constraint as 'Must not be "Default"' and documents that the
    // value is stored as a lowercase string, so every spelling of the reserved name folds
    // to it and is refused with the one sentence — in either region, and whether or not
    // the region already holds groups of its own.
    expect_ok(
        "a group of the region's own",
        create_security_group(&router, REGION, "emu-reserved-neighbour").await,
    );

    for region in [REGION, OTHER_REGION] {
        for spelling in ["default", "Default", "DEFAULT", "DeFaUlT"] {
            let case = format!("the reserved name sent as {spelling:?} in {region}");
            let refused = expect_error(
                &case,
                create_security_group(&router, region, spelling).await,
                400,
                "InvalidParameterValue",
            );
            let message = text(&refused, "Message");
            assert_eq!(
                message, RESERVED_DB_SECURITY_GROUP_SENTENCE,
                "case {case:?}: expected the reserved-name sentence, got {message:?}"
            );
        }
    }

    // The refusals stored nothing, and the group the region does hold still reads back.
    expect_error(
        "the reserved name after the refusals",
        call(
            &router,
            "DescribeDBSecurityGroups",
            &[("DBSecurityGroupName", "default")],
        )
        .await,
        404,
        "DBSecurityGroupNotFound",
    );
    let described = expect_ok(
        "the region's own group after the refusals",
        call(
            &router,
            "DescribeDBSecurityGroups",
            &[("DBSecurityGroupName", "emu-reserved-neighbour")],
        )
        .await,
    );
    assert_eq!(
        text(&described, "DBSecurityGroupName"),
        "emu-reserved-neighbour",
        "expected the refusals to leave the region's own group as it was, got {described}"
    );
}
