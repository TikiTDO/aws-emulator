// SPDX-License-Identifier: CC0-1.0

//! A stack's Amazon RDS Proxy resources in the service that OWNS them, end to end.
//!
//! RULE: a resource CloudFormation reports as created exists in the service that owns its
//! type, under the identifier the stack reports as its `PhysicalResourceId` —
//! "CloudFormation then makes the underlying service calls to provision your resources" —
//! so a proxy a stack reports created is a proxy Amazon RDS holds, in the status that
//! service reports a usable one in, with the one target group every proxy has and the
//! databases the template declared registered in it; and deleting the stack deletes the
//! proxy again.
//!
//! The types here are `AWS::RDS::DBProxy` (whose documented `Ref` return value is the
//! `DBProxyName`) and the `AWS::RDS::DBProxyTargetGroup` that names the databases the
//! proxy's one target group routes to, whose `Ref` returns the `TargetGroupArn` Amazon RDS
//! minted for that group. `crate::state::provisioner` is the layer under test.
//!
//! Every case drives the real request path for BOTH services through one router — the
//! CloudFormation calls over the awsQuery wire (the harness `stack_lifecycle.rs` owns) and
//! the Amazon RDS calls as a client's SDK makes them — so what is asserted is what a client
//! that deploys an RDS Proxy by template and then calls `DescribeDBProxyTargetGroups`
//! observes.

use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

use crate::rds_stack_resources::physical_id;
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, rds, router, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts,
};

/// The status Amazon RDS reports a usable proxy, and its one target group, in: "A status of
/// `available` means the proxy is ready to handle requests" (`DBProxy$Status`).
const AVAILABLE: &str = "available";

/// The only target group a proxy has: "Currently, this name must be `default`"
/// (`RegisterDBProxyTargets$TargetGroupName`).
const DEFAULT_TARGET_GROUP: &str = "default";

/// `TargetType`'s `RDS_INSTANCE` and `TargetHealth$State`'s `AVAILABLE`: what a registered
/// DB instance whose database the proxy can reach reports.
const RDS_INSTANCE: &str = "RDS_INSTANCE";
const TARGET_AVAILABLE: &str = "AVAILABLE";

/// The IAM role every row's proxy reads its secret with, in the form
/// `CreateDBProxy$RoleArn` takes.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/cfn-probe-rds-proxy";

/// One connection pool setting a row's target group must report, read off
/// `DescribeDBProxyTargetGroups`.
struct PoolSetting {
    member: &'static str,
    value: &'static str,
}

/// One row of the proxy-provisioning table.
struct ProxyCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    /// The logical id of the `AWS::RDS::DBProxy` the row declares.
    proxy: &'a str,
    /// The logical id of the `AWS::RDS::DBProxyTargetGroup` beside it.
    target_group: &'a str,
    /// The name the stack must report for the proxy: `Some` where the template NAMES it,
    /// `None` where CloudFormation generates the name.
    declared_name: Option<&'a str>,
    /// `EngineFamily` as declared, which Amazon RDS must report for the proxy.
    engine_family: &'a str,
    /// `RequireTLS` as declared, likewise.
    require_tls: &'a str,
    /// The DB instance identifiers the target group declares, which must be registered as
    /// `RDS_INSTANCE` targets reaching `AVAILABLE` health.
    registered: &'a [&'a str],
    /// The connection pool settings the target group declares, which the group must report.
    pool: &'a [PoolSetting],
}

/// `DescribeDBProxies` on one proxy name, as a client calls it.
async fn describe_db_proxy(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    rds(router, "DescribeDBProxies", &[("DBProxyName", name)]).await
}

/// `DescribeDBProxyTargetGroups` on one proxy name.
async fn describe_target_groups(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    rds(
        router,
        "DescribeDBProxyTargetGroups",
        &[("DBProxyName", name)],
    )
    .await
}

/// `DescribeDBProxyTargets` on one proxy name.
async fn describe_targets(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    rds(router, "DescribeDBProxyTargets", &[("DBProxyName", name)]).await
}

/// How often this suite re-reads a proxy's targets while they are still being put in
/// force, and how many times. The product bounds the wait, so a health that never settles
/// fails the test instead of hanging.
const TARGET_HEALTH_CHECK_INTERVAL: ::std::time::Duration = ::std::time::Duration::from_millis(25);
const TARGET_HEALTH_CHECKS: usize = 40;

/// `DescribeDBProxyTargets` on one proxy name, once every target it reports has finished
/// being put in force.
///
/// Amazon RDS answers a registration with the target `REGISTERING` and settles its health
/// on `AVAILABLE` on its own — `TargetHealth$State` publishes that lifecycle — so a client
/// reading a deployment's targets waits for the health it needs, which is what this does.
async fn describe_settled_targets(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    for _ in 0..TARGET_HEALTH_CHECKS {
        let (status_code, targets) = describe_targets(router, name).await;
        if status_code != 200
            || xml_texts(&targets, "State")
                .iter()
                .all(|state| state == TARGET_AVAILABLE)
        {
            return (status_code, targets);
        }
        tokio::time::sleep(TARGET_HEALTH_CHECK_INTERVAL).await;
    }
    describe_targets(router, name).await
}

/// The `OutputValue` the stack reports for `key`, read as the pair of lists the response
/// carries (every output of every template here resolves, so the pairing cannot slip).
fn output_value(xml: &str, key: &str) -> String {
    let keys = xml_texts(xml, "OutputKey");
    let values = xml_texts(xml, "OutputValue");
    keys.iter()
        .zip(values.iter())
        .find(|(output_key, _)| output_key.as_str() == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// A `DBProxyName` of the form that member publishes: `@length(min: 1, max: 63)` with
/// pattern `^[a-zA-Z](?:-?[a-zA-Z0-9]+)*$` (`smithy_query(query="operation",
/// operation="CreateDBProxy")`) — a letter first, then letters, digits and single hyphens,
/// ending on an alphanumeric.
fn is_db_proxy_name(value: &str) -> bool {
    (1..=63).contains(&value.chars().count())
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && value.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && !value.ends_with('-')
        && !value.contains("--")
}

#[tokio::test]
async fn a_proxy_a_stack_declares_exists_in_rds_once_the_stack_reports_it_created() {
    // The rows differ along what the rule generalises over: every value of `EngineFamily`
    // (`smithy_query(query="shape", shape="EngineFamily")`: `MYSQL | POSTGRESQL |
    // SQLSERVER`), whether the template NAMES the proxy or leaves CloudFormation to
    // generate the name, the template spelling (YAML with short-form intrinsics and JSON),
    // whether TLS is required, HOW MANY databases the target group registers (one, two and
    // none at all), and whether the group states connection pool settings of its own.
    let cases: &[ProxyCase<'_>] = &[
        ProxyCase {
            case: "a named MySQL proxy routing to one instance, in YAML",
            stack_name: "rds-proxy-mysql",
            template: concat!(
                "Resources:\n",
                "  Database:\n",
                "    Type: AWS::RDS::DBInstance\n",
                "    Properties:\n",
                "      DBInstanceIdentifier: cfn-probe-proxy-mysql\n",
                "      Engine: mysql\n",
                "      DBInstanceClass: db.t3.micro\n",
                "  Proxy:\n",
                "    Type: AWS::RDS::DBProxy\n",
                "    Properties:\n",
                "      DBProxyName: cfn-probe-mysql-proxy\n",
                "      EngineFamily: MYSQL\n",
                "      RequireTLS: false\n",
                "      RoleArn: arn:aws:iam::000000000000:role/cfn-probe-rds-proxy\n",
                "      VpcSubnetIds:\n",
                "        - subnet-0123456789abcdef0\n",
                "        - subnet-0fedcba987654321f\n",
                "      VpcSecurityGroupIds:\n",
                "        - sg-0123456789abcdef0\n",
                "      Auth:\n",
                "        - AuthScheme: SECRETS\n",
                "          IAMAuth: DISABLED\n",
                "          SecretArn: arn:aws:secretsmanager:us-east-1:000000000000:secret:p\n",
                "  Targets:\n",
                "    Type: AWS::RDS::DBProxyTargetGroup\n",
                "    Properties:\n",
                "      DBProxyName: !Ref Proxy\n",
                "      TargetGroupName: default\n",
                "      DBInstanceIdentifiers:\n",
                "        - !Ref Database\n",
                "Outputs:\n",
                "  ProxyName:\n",
                "    Value: !Ref Proxy\n",
                "  ProxyArn:\n",
                "    Value: !GetAtt Proxy.DBProxyArn\n",
                "  ProxyEndpoint:\n",
                "    Value: !GetAtt Proxy.Endpoint\n",
                "  TargetGroupArn:\n",
                "    Value: !Ref Targets\n",
            ),
            proxy: "Proxy",
            target_group: "Targets",
            declared_name: Some("cfn-probe-mysql-proxy"),
            engine_family: "MYSQL",
            require_tls: "false",
            registered: &["cfn-probe-proxy-mysql"],
            pool: &[],
        },
        ProxyCase {
            case: "a proxy CloudFormation names, routing to two instances with connection \
                   pool settings of its own, in JSON",
            stack_name: "rdsproxypostgres",
            template: r#"{"Resources":{
                   "First":{"Type":"AWS::RDS::DBInstance","Properties":{
                     "DBInstanceIdentifier":"cfn-probe-proxy-pg-one",
                     "Engine":"postgres","DBInstanceClass":"db.t3.small"}},
                   "Second":{"Type":"AWS::RDS::DBInstance","Properties":{
                     "DBInstanceIdentifier":"cfn-probe-proxy-pg-two",
                     "Engine":"postgres","DBInstanceClass":"db.t3.small"}},
                   "Proxy":{"Type":"AWS::RDS::DBProxy","Properties":{
                     "EngineFamily":"POSTGRESQL","RequireTLS":true,
                     "IdleClientTimeout":900,"DebugLogging":false,
                     "RoleArn":"arn:aws:iam::000000000000:role/cfn-probe-rds-proxy",
                     "VpcSubnetIds":["subnet-0123456789abcdef0"],
                     "Auth":[{"AuthScheme":"SECRETS","IAMAuth":"DISABLED",
                       "SecretArn":"arn:aws:secretsmanager:us-east-1:000000000000:secret:pg"}],
                     "Tags":[{"Key":"env","Value":"probe"}]}},
                   "Targets":{"Type":"AWS::RDS::DBProxyTargetGroup","Properties":{
                     "DBProxyName":{"Ref":"Proxy"},
                     "DBInstanceIdentifiers":[{"Ref":"First"},{"Ref":"Second"}],
                     "ConnectionPoolConfigurationInfo":{"MaxConnectionsPercent":80,
                       "MaxIdleConnectionsPercent":20,"ConnectionBorrowTimeout":90,
                       "InitQuery":"SET statement_timeout = 1000"}}}},
                 "Outputs":{"ProxyName":{"Value":{"Ref":"Proxy"}},
                   "ProxyArn":{"Value":{"Fn::GetAtt":["Proxy","DBProxyArn"]}},
                   "ProxyEndpoint":{"Value":{"Fn::GetAtt":["Proxy","Endpoint"]}},
                   "TargetGroupArn":{"Value":{"Ref":"Targets"}}}}"#,
            proxy: "Proxy",
            target_group: "Targets",
            declared_name: None,
            engine_family: "POSTGRESQL",
            require_tls: "true",
            registered: &["cfn-probe-proxy-pg-one", "cfn-probe-proxy-pg-two"],
            pool: &[
                PoolSetting {
                    member: "MaxConnectionsPercent",
                    value: "80",
                },
                PoolSetting {
                    member: "MaxIdleConnectionsPercent",
                    value: "20",
                },
                PoolSetting {
                    member: "ConnectionBorrowTimeout",
                    value: "90",
                },
                PoolSetting {
                    member: "InitQuery",
                    value: "SET statement_timeout = 1000",
                },
            ],
        },
        ProxyCase {
            case: "a SQL Server proxy whose target group registers no database at all, in \
                   YAML",
            stack_name: "rds-proxy-sqlserver",
            template: concat!(
                "Resources:\n",
                "  Proxy:\n",
                "    Type: AWS::RDS::DBProxy\n",
                "    Properties:\n",
                "      DBProxyName: cfn-probe-sqlserver-proxy\n",
                "      EngineFamily: SQLSERVER\n",
                "      RequireTLS: true\n",
                "      RoleArn: arn:aws:iam::000000000000:role/cfn-probe-rds-proxy\n",
                "      VpcSubnetIds:\n",
                "        - subnet-0123456789abcdef0\n",
                "  Targets:\n",
                "    Type: AWS::RDS::DBProxyTargetGroup\n",
                "    Properties:\n",
                "      DBProxyName: !Ref Proxy\n",
                "      TargetGroupName: default\n",
                "Outputs:\n",
                "  ProxyName:\n",
                "    Value: !Ref Proxy\n",
                "  ProxyArn:\n",
                "    Value: !GetAtt Proxy.DBProxyArn\n",
                "  ProxyEndpoint:\n",
                "    Value: !GetAtt Proxy.Endpoint\n",
                "  TargetGroupArn:\n",
                "    Value: !Ref Targets\n",
            ),
            proxy: "Proxy",
            target_group: "Targets",
            declared_name: Some("cfn-probe-sqlserver-proxy"),
            engine_family: "SQLSERVER",
            require_tls: "true",
            registered: &[],
            pool: &[],
        },
    ];

    for case in cases {
        let router = router();
        let (status_code, created) =
            create_stack(&router, case.stack_name, case.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: CreateStack must answer 200, got {status_code} {created}",
            case.case
        );
        wait_for_the_transition().await;

        let (status_code, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeStacks must answer 200, got {status_code} {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
            case.case
        );

        // The name the stack reports for the proxy — the same value its `Ref` output
        // publishes, and one Amazon RDS accepts as a `DBProxyName`.
        let name = physical_id(&router, case.stack_name, case.proxy).await;
        let published = output_value(&described, "ProxyName");
        assert_eq!(
            published, name,
            "case {:?}: the ProxyName output must publish the physical id of {}, got \
             {published:?} against {name:?}",
            case.case, case.proxy
        );
        if let Some(declared) = case.declared_name {
            assert_eq!(
                name, declared,
                "case {:?}: the template named the proxy, so that name is its identifier",
                case.case
            );
        }
        assert!(
            is_db_proxy_name(&name),
            "case {:?}: {name:?} must be a name Amazon RDS accepts",
            case.case
        );

        // The whole of the rule: Amazon RDS answers for that name, with the configuration
        // the template declared, and the proxy has reached the status a client waits for.
        let (status_code, proxy) = describe_db_proxy(&router, &name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeDBProxies({name}) must answer 200, got {status_code} \
             {proxy}",
            case.case
        );
        for (element, expected) in [
            ("DBProxyName", name.as_str()),
            ("Status", AVAILABLE),
            ("EngineFamily", case.engine_family),
            ("RequireTLS", case.require_tls),
            ("RoleArn", ROLE_ARN),
        ] {
            let actual = xml_text(&proxy, element);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {:?}: RDS must report {element} as {expected:?}, got {actual:?} in \
                 {proxy}",
                case.case
            );
        }

        // The two views of the resource AGREE: the ARN and the endpoint the stack publishes
        // are the ones Amazon RDS reports for that proxy, not values CloudFormation
        // computed for itself.
        for (output, element) in [("ProxyArn", "DBProxyArn"), ("ProxyEndpoint", "Endpoint")] {
            let stack_value = output_value(&described, output);
            let rds_value = xml_text(&proxy, element).unwrap_or_default();
            assert!(
                !rds_value.is_empty(),
                "case {:?}: RDS must report a {element} for the proxy, got {proxy}",
                case.case
            );
            assert_eq!(
                stack_value, rds_value,
                "case {:?}: the {output} output must be the {element} Amazon RDS reports, \
                 got {stack_value:?} against {rds_value:?}",
                case.case
            );
        }

        // The proxy's one target group is there, ready, and is the resource the stack's own
        // `AWS::RDS::DBProxyTargetGroup` reports — under the ARN Amazon RDS minted for it.
        let (status_code, groups) = describe_target_groups(&router, &name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeDBProxyTargetGroups({name}) must answer 200, got \
             {status_code} {groups}",
            case.case
        );
        for (element, expected) in [
            ("DBProxyName", name.as_str()),
            ("TargetGroupName", DEFAULT_TARGET_GROUP),
            ("Status", AVAILABLE),
        ] {
            let actual = xml_text(&groups, element);
            assert_eq!(
                actual.as_deref(),
                Some(expected),
                "case {:?}: the target group must report {element} as {expected:?}, got \
                 {actual:?} in {groups}",
                case.case
            );
        }
        let group_arn = xml_text(&groups, "TargetGroupArn").unwrap_or_default();
        let group_id = physical_id(&router, case.stack_name, case.target_group).await;
        assert_eq!(
            group_id, group_arn,
            "case {:?}: the target group resource's physical id must be the TargetGroupArn \
             Amazon RDS reports, got {group_id:?} against {group_arn:?}",
            case.case
        );
        assert_eq!(
            output_value(&described, "TargetGroupArn"),
            group_arn,
            "case {:?}: the TargetGroupArn output must publish that same ARN, got {described}",
            case.case
        );
        for setting in case.pool {
            let actual = xml_text(&groups, setting.member);
            assert_eq!(
                actual.as_deref(),
                Some(setting.value),
                "case {:?}: the group must report {} as {:?}, got {actual:?} in {groups}",
                case.case,
                setting.member,
                setting.value
            );
        }

        // Every database the target group declared is registered in it, as the kind of
        // target it is and with the health of one whose database the proxy can reach.
        let (status_code, targets) = describe_settled_targets(&router, &name).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: DescribeDBProxyTargets({name}) must answer 200, got {status_code} \
             {targets}",
            case.case
        );
        let reported = xml_blocks(&targets, "member");
        assert_eq!(
            reported.len(),
            case.registered.len(),
            "case {:?}: expected {} registered target(s), got {} in {targets}",
            case.case,
            case.registered.len(),
            reported.len()
        );
        for identifier in case.registered {
            let suffix = format!(":db:{identifier}");
            let target = reported
                .iter()
                .find(|entry| {
                    xml_text(entry, "TargetArn").is_some_and(|arn| arn.ends_with(&suffix))
                })
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected {identifier} among the proxy's targets, got \
                         {targets}",
                        case.case
                    )
                });
            for (element, expected) in [
                ("Type", RDS_INSTANCE),
                ("State", TARGET_AVAILABLE),
            ] {
                let actual = xml_text(target, element);
                assert_eq!(
                    actual.as_deref(),
                    Some(expected),
                    "case {:?}: target {identifier} must report {element} as {expected:?}, \
                     got {actual:?} in {target}",
                    case.case
                );
            }
        }
    }
}

#[tokio::test]
async fn a_target_group_registers_a_database_the_stack_did_not_create() {
    let router = router();
    // The database exists BEFORE the stack and is named literally by the declaration,
    // which is the other way round from the rows above (where the same stack creates it):
    // the rule is about the identifier the declaration resolves to, not about who created
    // the database behind it.
    let (status_code, created) = rds(
        &router,
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier", "emu-standing-proxy-db"),
            ("Engine", "mysql"),
            ("DBInstanceClass", "db.t3.micro"),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "CreateDBInstance must answer 200, got {status_code} {created}"
    );
    // The instance comes up before the stack is deployed, exactly as a client that waits
    // for it leaves it.
    wait_for_the_transition().await;

    let template = concat!(
        "Resources:\n",
        "  Proxy:\n",
        "    Type: AWS::RDS::DBProxy\n",
        "    Properties:\n",
        "      DBProxyName: emu-standing-proxy\n",
        "      EngineFamily: MYSQL\n",
        "      RoleArn: arn:aws:iam::000000000000:role/cfn-probe-rds-proxy\n",
        "      VpcSubnetIds:\n",
        "        - subnet-0123456789abcdef0\n",
        "  Targets:\n",
        "    Type: AWS::RDS::DBProxyTargetGroup\n",
        "    Properties:\n",
        "      DBProxyName: !Ref Proxy\n",
        "      TargetGroupName: default\n",
        "      DBInstanceIdentifiers:\n",
        "        - emu-standing-proxy-db\n",
    );
    let (status_code, body) = create_stack(&router, "rds-proxy-standing", template, &[]).await;
    assert_eq!(
        status_code, 200,
        "CreateStack must answer 200, got {status_code} {body}"
    );
    wait_for_the_transition().await;
    let (status_code, described) = describe_stack(&router, "rds-proxy-standing").await;
    assert_eq!(
        status_code, 200,
        "DescribeStacks must answer 200, got {status_code} {described}"
    );
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "the stack must reach CREATE_COMPLETE, got {described}"
    );

    let (status_code, groups) = describe_target_groups(&router, "emu-standing-proxy").await;
    assert_eq!(
        status_code, 200,
        "DescribeDBProxyTargetGroups must answer 200, got {status_code} {groups}"
    );
    assert_eq!(
        xml_text(&groups, "Status").as_deref(),
        Some(AVAILABLE),
        "the proxy's target group must be ready, got {groups}"
    );

    let (status_code, targets) = describe_settled_targets(&router, "emu-standing-proxy").await;
    assert_eq!(
        status_code, 200,
        "DescribeDBProxyTargets must answer 200, got {status_code} {targets}"
    );
    for (element, expected) in [
        ("Type", RDS_INSTANCE),
        ("State", TARGET_AVAILABLE),
    ] {
        let actual = xml_text(&targets, element);
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "the standing database must be registered reporting {element} {expected:?}, got \
             {actual:?} in {targets}"
        );
    }
    assert!(
        xml_text(&targets, "TargetArn")
            .is_some_and(|arn| arn.ends_with(":db:emu-standing-proxy-db")),
        "the target must address the database the declaration named, got {targets}"
    );
}

#[tokio::test]
async fn deleting_a_stack_deletes_the_proxy_and_the_registrations_it_created() {
    let router = router();
    // The instance the target group registers is declared by the same stack, so the
    // deletion walks the dependency order: the registrations go first, then the proxy, then
    // the database behind it.
    let template = concat!(
        "Resources:\n",
        "  Database:\n",
        "    Type: AWS::RDS::DBInstance\n",
        "    Properties:\n",
        "      DBInstanceIdentifier: cfn-probe-proxy-deleted\n",
        "      Engine: mysql\n",
        "      DBInstanceClass: db.t3.micro\n",
        "  Proxy:\n",
        "    Type: AWS::RDS::DBProxy\n",
        "    Properties:\n",
        "      DBProxyName: cfn-probe-deleted-proxy\n",
        "      EngineFamily: MYSQL\n",
        "      RoleArn: arn:aws:iam::000000000000:role/cfn-probe-rds-proxy\n",
        "      VpcSubnetIds:\n",
        "        - subnet-0123456789abcdef0\n",
        "  Targets:\n",
        "    Type: AWS::RDS::DBProxyTargetGroup\n",
        "    Properties:\n",
        "      DBProxyName: !Ref Proxy\n",
        "      DBInstanceIdentifiers:\n",
        "        - !Ref Database\n",
    );
    create_stack(&router, "rds-proxy-deleted", template, &[]).await;
    wait_for_the_transition().await;
    let (status_code, _) = describe_db_proxy(&router, "cfn-probe-deleted-proxy").await;
    assert_eq!(
        status_code, 200,
        "the stack's proxy must exist before the delete"
    );

    let (status_code, deleted) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "rds-proxy-deleted")],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "DeleteStack must answer 200, got {status_code} {deleted}"
    );
    // Twice: the stack's own terminal status and the removal Amazon RDS owes a resource it
    // put in `deleting` are two clocks, and both have to be past due.
    wait_for_the_transition().await;
    wait_for_the_transition().await;

    let (status_code, reported) = describe_db_proxy(&router, "cfn-probe-deleted-proxy").await;
    assert_eq!(
        status_code, 404,
        "deleting the stack must delete the proxy it created, got {status_code} {reported}"
    );
    assert_eq!(
        xml_text(&reported, "Code").as_deref(),
        Some("DBProxyNotFoundFault"),
        "the proxy must be gone under RDS's own not-found code, got {reported}"
    );
    // Every later RDS proxy read of the name is that same refusal — the report the stack's
    // own resources were read through included.
    let (status_code, reported) =
        describe_target_groups(&router, "cfn-probe-deleted-proxy").await;
    assert_eq!(
        status_code, 404,
        "the deleted proxy has no target group to report, got {status_code} {reported}"
    );
}
