// SPDX-License-Identifier: CC0-1.0

//! The requests this front door REFUSES because a published fact says they cannot be
//! served, and the refusal each one carries.
//!
//! Five rules, each asserted over a TABLE spanning what the rule generalises over rather
//! than over the one request any recording carried:
//!
//! * a `DBParameterGroupFamily` has to be a family an engine version of this front door
//!   belongs to — the families `DescribeDBEngineVersions` publishes, which the table READS
//!   from that report instead of naming them here, and the family a CUSTOM engine version of
//!   the account names — and a value outside them is refused with `InvalidParameterValue`
//!   naming the family;
//! * a parameter group carries only the parameters its family publishes WHERE that family's
//!   published vocabulary is the whole of it (the service-managed Amazon DocumentDB and
//!   Amazon Neptune lists): the table spans four engine vocabularies, both levels of a
//!   family's vocabulary, a name outside a complete one (refused, on the modify and on the
//!   reset), a name outside a SAMPLED one (carried) and a family that publishes no vocabulary
//!   at all;
//! * a parameter the engine PROTECTS cannot be set, and a STATIC parameter cannot be applied
//!   immediately: both tables are the published catalog ITSELF, so every protected parameter
//!   of every family in them is exercised;
//! * an event subscription's delivery topic has to be a topic Amazon SNS holds — the table
//!   spans a topic that service minted, one it never did, and one it deleted, on the create
//!   and on the modify — with Amazon SNS mounted beside Amazon RDS on ONE engine and driven
//!   through its own API, so every topic in an assertion is a topic that service really
//!   holds;
//! * a `GlobalClusterIdentifier` has to satisfy the identifier rule the API publishes for a
//!   global database, over every clause of it and both members that name one — and the
//!   global database a create mints is read back through `DescribeGlobalClusters`.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the assertions
//! read the XML the emulator itself answered with.

use std::sync::Arc;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    published_cluster_parameters, published_db_parameters, ClusterParameterSpec,
    APPLY_METHOD_IMMEDIATE, APPLY_METHOD_PENDING_REBOOT, APPLY_TYPE_DYNAMIC,
    MAX_GLOBAL_CLUSTER_IDENTIFIER_LEN,
};
use service_com_amazonaws_rds_amazonrdsv19::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

use crate::cluster_parameter_groups::create_group as create_cluster_group;
use crate::cluster_parameter_groups::modify as modify_cluster_group;
use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts, REGION,
};
use crate::db_parameters::{create_group as create_db_group, modify_in as modify_db_group};

/// The Query API code Amazon RDS answers a value its own member does not admit with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The Query API code it answers two request values that cannot hold together with.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

// ---------------------------------------------------------------------------
// Rule 1: which `DBParameterGroupFamily` values exist
// ---------------------------------------------------------------------------

/// The families this front door publishes an engine version of, as
/// `DescribeDBEngineVersions` reports them — asked, never assumed, so each admitted row
/// below is a family the catalogue really carries.
async fn published_families(router: &MultiServiceRouter) -> Vec<String> {
    let reported = expect_ok(
        "the published engine versions",
        call(
            router,
            "DescribeDBEngineVersions",
            &[("MaxRecords", "100"), ("IncludeAll", "true")],
        )
        .await,
    );
    let mut families: Vec<String> = Vec::new();
    for family in texts(&reported, "DBParameterGroupFamily") {
        if !families.contains(&family) {
            families.push(family);
        }
    }
    assert!(
        families.len() > 1,
        "the catalogue must publish several families, got {families:?}"
    );
    families
}

/// A `DBParameterGroupFamily` is one of the families an engine version of this front door
/// belongs to; a value naming none is refused, and the refused group is not stored.
///
/// The admitted rows are READ from `DescribeDBEngineVersions`; the refused ones are values
/// whose engine this front door serves but whose version no engine version carries
/// (`aurora-postgresql99` is the one a recording used), a value naming no engine at all, and
/// a bare engine name, which is not a family for any engine whose families carry a version.
#[tokio::test]
async fn only_a_family_an_engine_version_publishes_is_admitted() {
    let router = emulator();
    let families = published_families(&router).await;

    // (case, family) — every one refused.
    let refused: [(&str, &str); 6] = [
        ("a version no engine version of this family carries", "aurora-postgresql99"),
        ("another engine's unpublished version", "mysql9.9"),
        ("a sibling control plane's unpublished version", "docdb9.9"),
        ("an engine with no version at all", "postgres"),
        ("a value naming no engine", "not-a-family"),
        ("a family of an engine this door does not serve", "cassandra3.11"),
    ];

    for (index, family) in families.iter().enumerate() {
        let case = format!("the published family {family}");
        // The same published family is admitted in the case the caller spells it in: AWS
        // publishes each family lowercase, and a differently-cased value names the one
        // family it can only have meant.
        let spelling = if index % 2 == 0 {
            family.clone()
        } else {
            family.to_ascii_uppercase()
        };
        for (action, name_member, group_member) in [
            (
                "CreateDBClusterParameterGroup",
                "DBClusterParameterGroupName",
                "DBClusterParameterGroup",
            ),
            ("CreateDBParameterGroup", "DBParameterGroupName", "DBParameterGroup"),
        ] {
            let name = format!("published-{index}-{}", group_member.to_ascii_lowercase());
            let created = expect_ok(
                &case,
                call(
                    &router,
                    action,
                    &[
                        (name_member, name.as_str()),
                        ("DBParameterGroupFamily", spelling.as_str()),
                        ("Description", "a group under test"),
                    ],
                )
                .await,
            );
            assert_eq!(
                text(&created, "DBParameterGroupFamily"),
                spelling,
                "case {case:?}: the family the request named must be the one stored, got \
                 {created}"
            );
        }
    }

    for (case, family) in refused {
        for (action, name_member, describe, not_found) in [
            (
                "CreateDBClusterParameterGroup",
                "DBClusterParameterGroupName",
                "DescribeDBClusterParameterGroups",
                "DBParameterGroupNotFound",
            ),
            (
                "CreateDBParameterGroup",
                "DBParameterGroupName",
                "DescribeDBParameterGroups",
                "DBParameterGroupNotFound",
            ),
        ] {
            let name = format!("refused-{}", family.replace(['.', ' '], "-"));
            let answered = expect_error(
                case,
                call(
                    &router,
                    action,
                    &[
                        (name_member, name.as_str()),
                        ("DBParameterGroupFamily", family),
                        ("Description", "a group under test"),
                    ],
                )
                .await,
                400,
                INVALID_PARAMETER_VALUE,
            );
            let message = text(&answered, "Message");
            assert_eq!(
                message,
                format!("ParameterGroupFamily {family} is not a valid parameter group family"),
                "case {case:?}: expected the published refusal wording, got {message:?}"
            );
            // The refusal stored nothing: the name is not there to describe.
            expect_error(
                case,
                call(&router, describe, &[(name_member, name.as_str())]).await,
                404,
                not_found,
            );
        }
    }
}

/// A family only a CUSTOM engine version of this account names is a family that exists: the
/// account minted that engine version, this front door reports it, and a DB instance created
/// on it takes the default parameter groups of that family — so a parameter group of it is
/// admitted, while the same family is refused in a region the account minted no such version
/// in.
#[tokio::test]
async fn a_family_only_a_custom_engine_version_names_is_admitted() {
    let router = emulator();
    // A CEV whose MAJOR version the engine-version catalogue publishes no row for, so the
    // family RDS writes for it (`<engine>-<major version>`) is one only this account's own
    // engine version names.
    let engine = "sqlserver-se";
    let engine_version = "17.00.1000.1.probe_cev";
    let family = "sqlserver-se-17";
    let group = "cev-family-pg";

    // Before the CEV exists, the family names nothing.
    let refused = expect_error(
        "the family before the custom engine version exists",
        call(
            &router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", group),
                ("DBParameterGroupFamily", family),
                ("Description", "a group under test"),
            ],
        )
        .await,
        400,
        INVALID_PARAMETER_VALUE,
    );
    assert_eq!(
        text(&refused, "Message"),
        format!("ParameterGroupFamily {family} is not a valid parameter group family"),
        "expected the published refusal wording, got {refused}"
    );

    let minted = expect_ok(
        "the custom engine version",
        call(
            &router,
            "CreateCustomDBEngineVersion",
            &[
                ("Engine", engine),
                ("EngineVersion", engine_version),
                ("KMSKeyId", "11111111-2222-3333-4444-555555555555"),
            ],
        )
        .await,
    );
    // The family the group is created of is the one the CEV itself reports, never a value
    // this test derives.
    assert_eq!(
        text(&minted, "DBParameterGroupFamily"),
        family,
        "the custom engine version must report the family RDS writes for it, got {minted}"
    );
    // ...and now a group of the family that version names is created and read back.
    let created = expect_ok(
        "a group of the custom engine version's family",
        call(
            &router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", group),
                ("DBParameterGroupFamily", family),
                ("Description", "a group under test"),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&created, "DBParameterGroupFamily"),
        family,
        "the family the request named must be the one stored, got {created}"
    );
    let read_back = expect_ok(
        "the stored group",
        call(
            &router,
            "DescribeDBParameterGroups",
            &[("DBParameterGroupName", group)],
        )
        .await,
    );
    assert_eq!(
        text(&read_back, "DBParameterGroupFamily"),
        family,
        "the stored group must read back with that family, got {read_back}"
    );
    // The custom engine version is the ACCOUNT's, in the region it was created in, so another
    // region's request still names no such family.
    let elsewhere = expect_error(
        "the family in a region holding no such version",
        call_in(
            &router,
            "eu-central-1",
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", group),
                ("DBParameterGroupFamily", family),
                ("Description", "a group under test"),
            ],
        )
        .await,
        400,
        INVALID_PARAMETER_VALUE,
    );
    assert_eq!(
        text(&elsewhere, "Message"),
        format!("ParameterGroupFamily {family} is not a valid parameter group family"),
        "expected the published refusal wording, got {elsewhere}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: which parameters a group may carry
// ---------------------------------------------------------------------------

/// One row of the vocabulary table: the case, the family, the parameter, a value it admits,
/// and whether the group may carry that name at all.
type VocabularyRow = (&'static str, &'static str, &'static str, &'static str, bool);

/// A cluster parameter group carries only the parameters its family publishes, WHERE that
/// family's published vocabulary is the whole of it: Amazon DocumentDB and Amazon Neptune
/// publish a closed, service-managed parameter list per family and this emulator carries it,
/// so a name outside it names no parameter and is refused — on the modify that would SET it
/// and on the reset that would clear it alike. A family whose parameters are the open-source
/// ENGINE's own system variables (the MySQL- and PostgreSQL-compatible ones) publishes a
/// SAMPLE of AWS's list, and there every name is carried, because refusing against a sample
/// would reject parameters the engine really has; the names those families DO publish are
/// carried from either level of the family's vocabulary.
#[tokio::test]
async fn a_cluster_group_carries_only_parameters_its_family_publishes() {
    let router = emulator();

    let rows: [VocabularyRow; 11] = [
        // The family's own cluster-level parameters, across four engine vocabularies.
        ("an Aurora PostgreSQL cluster parameter", "aurora-postgresql16", "rds.logical_replication", "1", true),
        ("an Aurora MySQL cluster parameter", "aurora-mysql8.0", "binlog_format", "ROW", true),
        ("an Amazon DocumentDB parameter", "docdb5.0", "tls", "enabled", true),
        ("an Amazon Neptune cluster parameter", "neptune1.3", "neptune_streams", "1", true),
        // The ENGINE's own parameters, which an Aurora cluster parameter group carries too.
        ("the engine's own parameter, PostgreSQL", "aurora-postgresql16", "log_min_duration_statement", "250", true),
        ("the engine's own parameter, MySQL", "aurora-mysql8.0", "max_connections", "1000", true),
        // A name outside a family whose vocabulary IS complete: no such parameter.
        ("a name the DocumentDB vocabulary does not publish", "docdb5.0", "docdb_no_such_parameter", "1", false),
        ("a name the Neptune vocabulary does not publish", "neptune1.3", "neptune_no_such_parameter", "1", false),
        // A name outside a family whose published vocabulary is a SAMPLE: carried, since the
        // engine may well have it.
        ("an unpublished name of a sampled PostgreSQL vocabulary", "aurora-postgresql16", "autovacuum_naptime", "60", true),
        ("an unpublished name of a sampled MySQL vocabulary", "aurora-mysql8.0", "time_zone", "UTC", true),
        // A family this front door publishes no vocabulary for at either level.
        ("a family with no published vocabulary", "db2-se", "a_parameter_nothing_publishes", "1", true),
    ];

    for (index, (case, family, parameter, value, carried)) in rows.into_iter().enumerate() {
        let group = create_cluster_group(
            &router,
            case,
            &format!("vocabulary-cpg-{index}"),
            family,
        )
        .await;
        let answered = modify_cluster_group(&router, &group, parameter, value, None).await;
        if carried {
            expect_ok(case, answered);
            let reported = expect_ok(
                case,
                call(
                    &router,
                    "DescribeDBClusterParameters",
                    &[
                        ("DBClusterParameterGroupName", group.as_str()),
                        ("Source", "user"),
                    ],
                )
                .await,
            );
            assert_eq!(
                texts(&reported, "ParameterName"),
                vec![parameter.to_string()],
                "case {case:?}: the group must report the parameter it was given, got \
                 {reported}"
            );
            continue;
        }
        let refused = expect_error(case, answered, 400, INVALID_PARAMETER_VALUE);
        let message = text(&refused, "Message");
        assert_eq!(
            message,
            format!("Could not find parameter with name: {parameter}"),
            "case {case:?}: expected the published refusal wording, got {message:?}"
        );
        // A RESET naming it is refused the same way and by the same resolution: a parameter
        // the family publishes none of is not one to put back to its default either.
        let reset = expect_error(
            case,
            call(
                &router,
                "ResetDBClusterParameterGroup",
                &[
                    ("DBClusterParameterGroupName", group.as_str()),
                    ("Parameters.Parameter.1.ParameterName", parameter),
                ],
            )
            .await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        assert_eq!(
            text(&reset, "Message"),
            format!("Could not find parameter with name: {parameter}"),
            "case {case:?}: expected the published refusal wording, got {reset}"
        );
        // Nothing was stored, so the group's report does not carry the name either.
        let reported = expect_ok(
            case,
            call(
                &router,
                "DescribeDBClusterParameters",
                &[
                    ("DBClusterParameterGroupName", group.as_str()),
                    ("Source", "user"),
                ],
            )
            .await,
        );
        assert!(
            !texts(&reported, "ParameterName").contains(&parameter.to_string()),
            "case {case:?}: a refused parameter must not be in state, got {reported}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the parameters the engine protects, and the method a change may take
// ---------------------------------------------------------------------------

/// The families whose published catalogs the two tables below are driven by: one family per
/// engine vocabulary this front door publishes a DB-parameter catalog for, so every protected
/// parameter of each of them is exercised rather than the one a recording named.
const CATALOG_FAMILIES: [&str; 5] = [
    "mysql8.0",
    "postgres16",
    "oracle-ee-19",
    "sqlserver-ee-15.0",
    "neptune1.3",
];

/// A parameter the engine does not let a caller modify cannot be set: the request is refused
/// with `InvalidParameterValue` naming the parameter, and the group keeps reporting the
/// engine's own value for it rather than the one the request asked for.
///
/// The table is the published catalog ITSELF: every parameter of every family above whose
/// record says the engine protects it.
#[tokio::test]
async fn a_parameter_the_engine_protects_cannot_be_set() {
    let router = emulator();
    let mut protected = 0usize;

    for (index, family) in CATALOG_FAMILIES.into_iter().enumerate() {
        let catalog = published_db_parameters(family)
            .unwrap_or_else(|| panic!("case {family:?}: this family must publish a catalog"));
        let group =
            create_db_group(&router, family, &format!("protected-pg-{index}"), family).await;
        for spec in catalog.iter().filter(|spec| !spec.is_modifiable) {
            protected += 1;
            let case = format!("{family} {}", spec.name);
            let refused = expect_error(
                &case,
                modify_db_group(
                    &router,
                    REGION,
                    &group,
                    &[(spec.name, &admitted_value(spec), None)],
                )
                .await,
                400,
                INVALID_PARAMETER_VALUE,
            );
            let message = text(&refused, "Message");
            assert_eq!(
                message,
                format!("The parameter {} cannot be modified.", spec.name),
                "case {case:?}: expected the published refusal wording, got {message:?}"
            );
            // The group carries no value of its own for it: what a read reports is still the
            // engine's own default.
            let reported = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeDBParameters",
                    &[
                        ("DBParameterGroupName", group.as_str()),
                        ("Source", "user"),
                    ],
                )
                .await,
            );
            assert!(
                !texts(&reported, "ParameterName").contains(&spec.name.to_string()),
                "case {case:?}: a refused parameter must not be in state, got {reported}"
            );
        }
    }
    assert!(
        protected > 1,
        "the catalogs must publish several protected parameters, got {protected}"
    );
}

/// A STATIC parameter cannot be applied immediately — "changes to static parameters require a
/// reboot or maintenance window before the change can take effect" — so that one combination
/// is refused with `InvalidParameterCombination`, while `pending-reboot` and an absent method
/// are served. A DYNAMIC parameter takes either method.
///
/// The table is the published catalogs themselves, at both levels: the DB-parameter catalog of
/// every family above and the CLUSTER catalog of one family per cluster vocabulary.
#[tokio::test]
async fn a_static_parameter_cannot_be_applied_immediately() {
    let router = emulator();

    for (index, family) in CATALOG_FAMILIES.into_iter().enumerate() {
        let catalog = published_db_parameters(family)
            .unwrap_or_else(|| panic!("case {family:?}: this family must publish a catalog"));
        let group = create_db_group(&router, family, &format!("methods-pg-{index}"), family).await;
        for spec in catalog.iter().filter(|spec| spec.is_modifiable) {
            let apply_type = apply_type_in_family(family, spec);
            for method in [Some(APPLY_METHOD_IMMEDIATE), Some(APPLY_METHOD_PENDING_REBOOT), None] {
                let case =
                    format!("{family} {} ({apply_type}) asked for {method:?}", spec.name);
                let answered = modify_db_group(
                    &router,
                    REGION,
                    &group,
                    &[(spec.name, &admitted_value(spec), method)],
                )
                .await;
                assert_refusal_follows_the_apply_type(&case, apply_type, method, answered);
            }
        }
    }

    for (index, family) in ["aurora-mysql8.0", "aurora-postgresql16", "docdb5.0", "neptune1.3"]
        .into_iter()
        .enumerate()
    {
        let catalog = published_cluster_parameters(family)
            .unwrap_or_else(|| panic!("case {family:?}: this family must publish a catalog"));
        let group = create_cluster_group(
            &router,
            family,
            &format!("methods-cpg-{index}"),
            family,
        )
        .await;
        for spec in catalog.iter().filter(|spec| spec.is_modifiable) {
            let apply_type = apply_type_in_family(family, spec);
            for method in [Some(APPLY_METHOD_IMMEDIATE), Some(APPLY_METHOD_PENDING_REBOOT), None] {
                let case =
                    format!("{family} {} ({apply_type}) asked for {method:?}", spec.name);
                let answered =
                    modify_cluster_group(&router, &group, spec.name, &admitted_value(spec), method)
                        .await;
                assert_refusal_follows_the_apply_type(&case, apply_type, method, answered);
            }
        }
    }
}

/// The apply type `spec` carries FOR `family`: the catalog's own column, except for
/// `rds.force_ssl`, whose record moved with the engine — "beginning with RDS for PostgreSQL
/// 15, rds.force_ssl is a dynamic parameter" — so a PostgreSQL-compatible family at 15 or
/// later applies a change to it without a restart. Written out here rather than read from the
/// crate, so a drift in it fails this test.
fn apply_type_in_family(family: &str, spec: &ClusterParameterSpec) -> &'static str {
    let postgres = family
        .strip_prefix("postgres")
        .or_else(|| family.strip_prefix("aurora-postgresql"));
    let moved = postgres
        .filter(|_| spec.name == "rds.force_ssl")
        .and_then(|version| version.split('.').next())
        .and_then(|major| major.parse::<u32>().ok())
        .is_some_and(|major| major >= 15);
    if moved {
        APPLY_TYPE_DYNAMIC
    } else {
        spec.apply_type
    }
}

/// `immediate` on a STATIC parameter is the one combination refused; every other pair of
/// parameter and method is served.
fn assert_refusal_follows_the_apply_type(
    case: &str,
    apply_type: &str,
    method: Option<&str>,
    answered: (u16, String),
) {
    if method == Some(APPLY_METHOD_IMMEDIATE) && apply_type != APPLY_TYPE_DYNAMIC {
        let refused = expect_error(case, answered, 400, INVALID_PARAMETER_COMBINATION);
        let message = text(&refused, "Message");
        assert_eq!(
            message, "cannot use immediate apply method for static parameter",
            "case {case:?}: expected the published refusal wording, got {message:?}"
        );
        return;
    }
    expect_ok(case, answered);
}

/// A value `spec` admits, read from its own published record: the first of an enumeration,
/// the low bound of a range, and any string for a parameter the engine bounds by nothing.
fn admitted_value(spec: &ClusterParameterSpec) -> String {
    if spec.allowed_values.is_empty() {
        return "a-value-no-published-domain-bounds".to_string();
    }
    if spec.allowed_values.contains(',') {
        return spec
            .allowed_values
            .split(',')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
    }
    match spec.range() {
        Some((low, _)) if matches!(spec.data_type, "integer" | "float") => low.trim().to_string(),
        _ => spec.allowed_values.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the Amazon SNS topic an event subscription delivers to
// ---------------------------------------------------------------------------

/// Amazon RDS and Amazon SNS on ONE engine, the way the assembled router mounts them, with an
/// Amazon SNS client over the router's own in-process callback — the very seam the topic
/// resolution under test reaches that service through.
struct Account {
    router: MultiServiceRouter,
    sns: ::aws_sdk_sns::Client,
}

impl Account {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject of these tests; the requests still carry the SigV4
        // credential scope, which is what routes them and names the region.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        let config = ::aws_sdk_sns::Config::builder()
            .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
            .region(::aws_sdk_sns::config::Region::new(REGION.to_string()))
            .endpoint_url(format!("https://sns.{REGION}.amazonaws.com"))
            .credentials_provider(callback.aws_credentials_provider())
            .http_client(callback.http_client())
            .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
            .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled(),
            )
            .build();
        Self {
            router,
            sns: ::aws_sdk_sns::Client::from_conf(config),
        }
    }

    /// A topic Amazon SNS really holds, created through its own API, reported by its ARN.
    async fn create_topic(&self, name: &str) -> String {
        self.sns
            .create_topic()
            .name(name)
            .send()
            .await
            .expect("Amazon SNS must create the topic")
            .topic_arn
            .expect("a created topic reports its ARN")
    }

    /// Delete a topic Amazon SNS holds, so the ARN names no topic afterwards.
    async fn delete_topic(&self, arn: &str) {
        self.sns
            .delete_topic()
            .topic_arn(arn)
            .send()
            .await
            .expect("Amazon SNS must delete the topic");
    }
}

/// An event subscription's delivery topic has to be a topic Amazon SNS holds: a subscription
/// against one it does not is refused with `SNSTopicArnNotFound` (HTTP 404) naming the topic,
/// and nothing is stored for it — on the create and on a modify that would move a stored
/// subscription onto such a topic. A value that is not a topic ARN at all is the other
/// refusal, `SNSInvalidTopic`.
#[tokio::test]
async fn an_event_subscription_needs_a_topic_amazon_sns_holds() {
    let account = Account::new();
    let router = &account.router;
    // Two topics that EXIST, and one ARN that never named a topic. The absent ARN is written
    // the way Amazon SNS writes one, so the refusal is about the topic and not about the form.
    let held = account.create_topic("rds-events-held").await;
    let doomed = account.create_topic("rds-events-doomed").await;
    let absent = format!(
        "{}-never-created",
        held.trim_end_matches("rds-events-held").to_string() + "rds-events"
    );

    // A subscription against a topic Amazon SNS holds is created and read back.
    let created = expect_ok(
        "a topic Amazon SNS holds",
        call(
            router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "sub-held"),
                ("SnsTopicArn", held.as_str()),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&created, "SnsTopicArn"),
        held,
        "the subscription must report the topic it delivers to, got {created}"
    );
    let read_back = expect_ok(
        "the stored subscription",
        call(
            router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "sub-held")],
        )
        .await,
    );
    assert_eq!(
        text(&read_back, "SnsTopicArn"),
        held,
        "the stored subscription must read back with that topic, got {read_back}"
    );

    // (case, the topic ARN a create names, the error code, the HTTP status)
    let rows: Vec<(&str, String, &str, u16)> = vec![
        ("an ARN no topic was ever created under", absent.clone(), "SNSTopicArnNotFound", 404),
        ("a value that is not a topic ARN", "not-an-arn".to_string(), "SNSInvalidTopic", 400),
    ];
    for (index, (case, topic, code, status)) in rows.into_iter().enumerate() {
        let name = format!("sub-refused-{index}");
        let refused = expect_error(
            case,
            call(
                router,
                "CreateEventSubscription",
                &[
                    ("SubscriptionName", name.as_str()),
                    ("SnsTopicArn", topic.as_str()),
                ],
            )
            .await,
            status,
            code,
        );
        if code == "SNSTopicArnNotFound" {
            let message = text(&refused, "Message");
            assert_eq!(
                message,
                format!("Failed to create Subscription because of Topic {topic} Not Found."),
                "case {case:?}: expected the published refusal wording, got {message:?}"
            );
        }
        // The refusal stored nothing: the name is not there to describe.
        expect_error(
            case,
            call(
                router,
                "DescribeEventSubscriptions",
                &[("SubscriptionName", name.as_str())],
            )
            .await,
            404,
            "SubscriptionNotFound",
        );
    }

    // A topic that EXISTED and was then deleted is as absent as one that never existed: the
    // question is put to Amazon SNS on every call rather than answered from a cache.
    expect_ok(
        "a subscription against the topic that is about to be deleted",
        call(
            router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "sub-doomed"),
                ("SnsTopicArn", doomed.as_str()),
            ],
        )
        .await,
    );
    account.delete_topic(&doomed).await;
    let after_delete = expect_error(
        "a create against a topic Amazon SNS no longer holds",
        call(
            router,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "sub-after-delete"),
                ("SnsTopicArn", doomed.as_str()),
            ],
        )
        .await,
        404,
        "SNSTopicArnNotFound",
    );
    assert_eq!(
        text(&after_delete, "Message"),
        format!("Failed to create Subscription because of Topic {doomed} Not Found."),
        "expected the published refusal wording, got {after_delete}"
    );

    // A MODIFY onto a topic Amazon SNS does not hold is refused the same way, and the
    // subscription keeps the topic it already had.
    let refused = expect_error(
        "a modify onto a topic Amazon SNS does not hold",
        call(
            router,
            "ModifyEventSubscription",
            &[
                ("SubscriptionName", "sub-held"),
                ("SnsTopicArn", absent.as_str()),
            ],
        )
        .await,
        404,
        "SNSTopicArnNotFound",
    );
    assert_eq!(
        text(&refused, "Message"),
        format!("Failed to create Subscription because of Topic {absent} Not Found."),
        "expected the published refusal wording, got {refused}"
    );
    let unchanged = expect_ok(
        "the subscription after the refused modify",
        call(
            router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", "sub-held")],
        )
        .await,
    );
    assert_eq!(
        text(&unchanged, "SnsTopicArn"),
        held,
        "a refused modify must leave the subscription's topic alone, got {unchanged}"
    );
    // ...and a modify onto a topic it DOES hold is served.
    let moved = expect_ok(
        "a modify onto a topic Amazon SNS holds",
        call(
            router,
            "ModifyEventSubscription",
            &[
                ("SubscriptionName", "sub-held"),
                ("SnsTopicArn", held.as_str()),
            ],
        )
        .await,
    );
    assert_eq!(
        text(&moved, "SnsTopicArn"),
        held,
        "the modify must report the topic it delivers to, got {moved}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the identifier a global database may carry
// ---------------------------------------------------------------------------

/// The one sentence pair this service family refuses an identifier with, naming the member at
/// fault — written out here rather than read from the crate, so a drift in it fails this test.
fn not_a_valid_identifier(member: &str) -> String {
    format!(
        "The parameter {member} is not a valid identifier. Identifiers must begin with a \
         letter; must contain only ASCII letters, digits, and hyphens; and must not end with \
         a hyphen or contain two consecutive hyphens."
    )
}

/// A `GlobalClusterIdentifier` has to satisfy the identifier rule the API publishes for a
/// global database — "from 1 to 63 letters, numbers, or hyphens. The first character must be a
/// letter. Can't end with a hyphen or contain two consecutive hyphens" — whichever member
/// names one, and a value outside it is refused with `InvalidParameterValue` naming the member.
///
/// The table spans every clause of the rule, each side of its length bound, and the characters
/// the modelled `@pattern` would admit that an identifier may not carry. A value the model's
/// OWN constraint already refuses — one that does not begin with a letter, which its
/// `@pattern` requires too — is refused at the boundary before any handler runs, under the
/// same code; that row is marked, because the wording is then the framework's and not this
/// service's.
#[tokio::test]
async fn a_global_cluster_identifier_outside_the_published_rule_is_refused() {
    let router = emulator();
    let longest = format!("g{}", "a".repeat(MAX_GLOBAL_CLUSTER_IDENTIFIER_LEN - 1));
    let too_long = format!("g{}", "a".repeat(MAX_GLOBAL_CLUSTER_IDENTIFIER_LEN));

    // (case, identifier, admitted, the refusal is this service's own sentence)
    let rows: [(&str, &str, bool, bool); 11] = [
        ("letters, digits and hyphens", "emu-gc-1", true, false),
        ("a single letter", "g", true, false),
        ("the longest admitted identifier", &longest, true, false),
        ("one character past the bound", &too_long, false, true),
        ("mixed case, stored lowercase", "EMU-GC-Mixed", true, false),
        // The model's own `@pattern` requires a leading letter as well, so this one never
        // reaches the handler.
        ("a leading digit", "1emu-gc", false, false),
        ("an underscore", "emu_gc", false, true),
        ("a dot", "emu.gc", false, true),
        ("a colon", "emu:gc", false, true),
        ("a trailing hyphen", "emu-gc-", false, true),
        ("two consecutive hyphens", "emu--gc", false, true),
    ];

    for (case, identifier, admitted, own_sentence) in rows {
        let answered = call(
            &router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", identifier),
                ("Engine", "aurora-postgresql"),
            ],
        )
        .await;
        if admitted {
            let created = expect_ok(case, answered);
            // "This parameter is stored as a lowercase string", and the create's own answer
            // already reports the stored form.
            assert_eq!(
                text(&created, "GlobalClusterIdentifier"),
                identifier.to_ascii_lowercase(),
                "case {case:?}: expected the identifier stored lowercased, got {created}"
            );
            let read_back = expect_ok(
                case,
                call(
                    &router,
                    "DescribeGlobalClusters",
                    &[("GlobalClusterIdentifier", identifier)],
                )
                .await,
            );
            assert_eq!(
                text(&read_back, "GlobalClusterIdentifier"),
                identifier.to_ascii_lowercase(),
                "case {case:?}: the created global database must read back, got {read_back}"
            );
            continue;
        }
        let refused = expect_error(case, answered, 400, INVALID_PARAMETER_VALUE);
        if !own_sentence {
            // The model's own constraint refused it at the boundary, in the framework's
            // wording, and the same constraint refuses the value on every other member that
            // carries it — so there is no read of this identifier to make either.
            continue;
        }
        let message = text(&refused, "Message");
        assert_eq!(
            message,
            not_a_valid_identifier("GlobalClusterIdentifier"),
            "case {case:?}: expected the published refusal wording, got {message:?}"
        );
        // The refusal stored nothing.
        expect_error(
            case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", identifier)],
            )
            .await,
            404,
            "GlobalClusterNotFoundFault",
        );

        // The same rule holds for the member that RENAMES a global database onto an
        // identifier, so a value this create refuses cannot be reached by a rename either.
        let renamed = expect_error(
            case,
            call(
                &router,
                "ModifyGlobalCluster",
                &[
                    ("GlobalClusterIdentifier", "emu-gc-1"),
                    ("NewGlobalClusterIdentifier", identifier),
                ],
            )
            .await,
            400,
            INVALID_PARAMETER_VALUE,
        );
        assert_eq!(
            text(&renamed, "Message"),
            not_a_valid_identifier("NewGlobalClusterIdentifier"),
            "case {case:?}: expected the published refusal wording, got {renamed}"
        );
    }
}

/// A global database created AROUND an existing DB cluster adopts that cluster as its
/// primary, and reports the writer endpoint that primary is reached through — read back
/// unchanged by `DescribeGlobalClusters`, since the endpoint is the global database's own
/// name and not a value recomputed per response.
///
/// This is the create path the identifier rule above does not reach: it names a source DB
/// cluster rather than an engine, so the global database it mints holds a member.
#[tokio::test]
async fn a_global_database_created_around_a_cluster_reports_its_writer_endpoint() {
    let router = emulator();

    expect_ok(
        "the source DB cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-endpoint-source"),
                ("Engine", "aurora-postgresql"),
                ("MasterUsername", "primary"),
                ("MasterUserPassword", "a-master-password-1"),
            ],
        )
        .await,
    );
    let created = expect_ok(
        "a global database around an existing cluster",
        call(
            &router,
            "CreateGlobalCluster",
            &[
                ("GlobalClusterIdentifier", "emu-endpoint-primary"),
                ("SourceDBClusterIdentifier", "emu-endpoint-source"),
            ],
        )
        .await,
    );
    let members = subtrees(&created, "GlobalClusterMember");
    assert_eq!(
        members.len(),
        1,
        "the global database must hold the cluster it was created around, got {created}"
    );
    assert_eq!(
        optional_text(&members[0], "IsWriter").as_deref(),
        Some("true"),
        "the cluster it was created around is its primary, got {created}"
    );
    let endpoint = text(&created, "Endpoint");
    assert!(
        endpoint.starts_with("emu-endpoint-primary.global-")
            && endpoint.ends_with(".global.rds.amazonaws.com"),
        "expected the global database's own writer endpoint, got {endpoint:?} in {created}"
    );
    let read_back = expect_ok(
        "the global database with a primary",
        call(
            &router,
            "DescribeGlobalClusters",
            &[("GlobalClusterIdentifier", "emu-endpoint-primary")],
        )
        .await,
    );
    assert_eq!(
        text(&read_back, "Endpoint"),
        endpoint,
        "the writer endpoint must read back unchanged, got {read_back}"
    );
}
