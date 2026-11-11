// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack whose only resource is an `AWS::OpenSearchService::Domain` deploys:
//! it settles in `CREATE_COMPLETE`, its outputs carry the identifiers Amazon OpenSearch
//! Service minted, and the domain reads back through that service's own API with the
//! configuration the template declared.
//!
//! The rule under test: **a stack settles in `CREATE_COMPLETE` when every create the
//! template's resources ask of their owning services is ACCEPTED, and it reaches
//! `ROLLBACK_COMPLETE` only when one of those services refuses a create — in which case the
//! failed resource's event carries that service's own refusal as its
//! `ResourceStatusReason`.** So a domain configuration Amazon OpenSearch Service accepts from
//! a client directly must also deploy, whatever the template spells it with, and one it
//! refuses must fail the resource carrying the refusal that client would have read.
//!
//! The rows span what the rule generalises over rather than one recorded template: the
//! SPELLING of the body (YAML with the short-form intrinsic tags, and the same deployment as
//! JSON), where the domain's name comes from (a template `Parameter` resolved by `Ref`, a
//! literal, and a name CloudFormation generates because the template declares none), the
//! engine line and release, the shape of the cluster (a single burstable node, a Graviton pair
//! with dedicated masters and zone awareness, a node whose storage is the instance's own), the
//! storage form (`gp2`, `gp3` with provisioned IOPS and throughput, no `EBSOptions` at all),
//! the option containers declared beside it (encryption at rest with the service-owned key,
//! node-to-node encryption, `EnforceHTTPS`), the FORM of the access policy (an object whose
//! intrinsics resolve to the account root, the quoted text of a document, none at all), and
//! both directions of the rule — creates the owning service accepts, and one it refuses.
//!
//! The refused row's expected reason is not written here: it is read from the owning service by
//! asking it the SAME create directly, so the row cannot drift from the service it is asserted
//! against.
//!
//! The router, the two wire helpers, the domain read and the stack poller are the test crate's
//! own ([`crate::cloudformation_provisions_opensearch_domains`], [`crate::query_wire`]), not
//! another copy: that sibling suite deploys the same resource type, and what it holds to the
//! owning service's CATALOGUE (which instance type each release offers) this one holds to the
//! stack's own settling.

use std::collections::BTreeMap;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_opensearch_domains::{
    build_router, cloudformation, describe_domain, opensearch, resource_reasons, settle_stack,
    xml_text, MISSING_RESOURCE_STATUS,
};
use crate::query_wire::outputs_of;

/// The placeholder a row's template and direct request carry where the domain's name belongs.
const DOMAIN: &str = "@DOMAIN@";

/// `DomainName`'s published length bound, which a name CloudFormation GENERATES for a domain
/// has to satisfy like any other: "len 3..28, pattern `^[a-z][a-z0-9\-]+$`"
/// (`smithy_query(query="operation", operation="CreateDomain")`).
const DOMAIN_NAME_LENGTH: std::ops::RangeInclusive<usize> = 3..=28;

/// What the owning service must end up holding for a row whose create it accepts.
struct Held {
    /// The engine release the domain runs, as the template declared it.
    engine_version: &'static str,
    /// The instance type its data nodes run on.
    instance_type: &'static str,
    /// The volume the template declared, as `(VolumeType, VolumeSize)`, or `None` for a
    /// declaration carrying no `EBSOptions` at all.
    volume: Option<(&'static str, i64)>,
    /// Whether the template asked for encryption at rest and for node-to-node encryption.
    encrypted: bool,
    /// Whether it required HTTPS on the domain's endpoint.
    enforce_https: bool,
    /// Whether it declared an access policy.
    access_policy: bool,
}

/// What a row's deployment must do.
enum Expect {
    /// The stack settles in `CREATE_COMPLETE` and the domain reads back like this.
    Created(Held),
    /// The owning service refuses the create, so the stack rolls back carrying its refusal.
    /// The field is that SAME create as a direct request body, which is where the expected
    /// refusal is read from.
    Refused(&'static str),
}

/// One row: a deployment of a single-domain template, and what it must leave behind.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The domain the template names, inside `DomainName`'s own bounds, or `None` for a
    /// template that declares no name and leaves CloudFormation to generate one.
    domain: Option<&'static str>,
    /// The body the deployment sends, with [`DOMAIN`] where the domain's name belongs.
    template: &'static str,
    /// The `Parameters` the deployment passes, for a row whose template declares some.
    parameters: &'static [(&'static str, &'static str)],
    expect: Expect,
}

/// The recorded shape: a YAML body with the short-form intrinsic tags, the domain named by a
/// template parameter, the full option set, and a policy object delegating to the account
/// root.
const YAML_PARAMETERISED: &str = r#"AWSTemplateFormatVersion: '2010-09-09'
Description: The search domain of the indexing pipeline
Parameters:
  DomainName:
    Type: String
    Description: The domain to create
  EngineVersion:
    Type: String
    Default: OpenSearch_2.11
Resources:
  SearchDomain:
    Type: AWS::OpenSearchService::Domain
    Properties:
      DomainName: !Ref DomainName
      EngineVersion: !Ref EngineVersion
      ClusterConfig:
        InstanceType: t3.small.search
        InstanceCount: 1
        DedicatedMasterEnabled: false
        ZoneAwarenessEnabled: false
      EBSOptions:
        EBSEnabled: true
        VolumeType: gp2
        VolumeSize: 10
      NodeToNodeEncryptionOptions:
        Enabled: true
      EncryptionAtRestOptions:
        Enabled: true
      DomainEndpointOptions:
        EnforceHTTPS: true
      AccessPolicies:
        Version: '2012-10-17'
        Statement:
          - Effect: Allow
            Principal:
              AWS: !Sub 'arn:${AWS::Partition}:iam::${AWS::AccountId}:root'
            Action: 'es:*'
            Resource: !Sub 'arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/${DomainName}/*'
Outputs:
  DomainEndpoint:
    Value: !GetAtt SearchDomain.DomainEndpoint
  DomainArn:
    Value: !GetAtt SearchDomain.Arn
  DomainNameOut:
    Value: !Ref SearchDomain
"#;

/// The same deployment spelled as JSON, with the domain named literally: the spelling of the
/// body is not what decides whether a template deploys.
const JSON_LITERAL: &str = r#"{"AWSTemplateFormatVersion":"2010-09-09",
  "Resources":{"SearchDomain":{"Type":"AWS::OpenSearchService::Domain","Properties":{
    "DomainName":"@DOMAIN@","EngineVersion":"OpenSearch_2.11",
    "ClusterConfig":{"InstanceType":"t3.small.search","InstanceCount":1,
      "DedicatedMasterEnabled":false,"ZoneAwarenessEnabled":false},
    "EBSOptions":{"EBSEnabled":true,"VolumeType":"gp2","VolumeSize":10},
    "NodeToNodeEncryptionOptions":{"Enabled":true},
    "EncryptionAtRestOptions":{"Enabled":true},
    "DomainEndpointOptions":{"EnforceHTTPS":true},
    "AccessPolicies":{"Version":"2012-10-17","Statement":[
      {"Effect":"Allow",
       "Principal":{"AWS":{"Fn::Sub":"arn:${AWS::Partition}:iam::${AWS::AccountId}:root"}},
       "Action":"es:*",
       "Resource":{"Fn::Sub":"arn:${AWS::Partition}:es:${AWS::Region}:${AWS::AccountId}:domain/@DOMAIN@/*"}}]}}}},
  "Outputs":{
    "DomainEndpoint":{"Value":{"Fn::GetAtt":["SearchDomain","DomainEndpoint"]}},
    "DomainArn":{"Value":{"Fn::GetAtt":["SearchDomain","Arn"]}},
    "DomainNameOut":{"Value":{"Ref":"SearchDomain"}}}}"#;

/// A bigger cluster on a later release: a Graviton pair with dedicated masters and zone
/// awareness, `gp3` storage with provisioned IOPS and throughput, and the policy as the quoted
/// TEXT of a document.
const YAML_GRAVITON: &str = r#"Resources:
  SearchDomain:
    Type: AWS::OpenSearchService::Domain
    Properties:
      DomainName: '@DOMAIN@'
      EngineVersion: OpenSearch_2.19
      ClusterConfig:
        InstanceType: m6g.large.search
        InstanceCount: 2
        DedicatedMasterEnabled: true
        DedicatedMasterType: m6g.large.search
        DedicatedMasterCount: 3
        ZoneAwarenessEnabled: true
        ZoneAwarenessConfig:
          AvailabilityZoneCount: 2
      EBSOptions:
        EBSEnabled: true
        VolumeType: gp3
        VolumeSize: 100
        Iops: 3000
        Throughput: 125
      NodeToNodeEncryptionOptions:
        Enabled: true
      EncryptionAtRestOptions:
        Enabled: true
      DomainEndpointOptions:
        EnforceHTTPS: true
        TLSSecurityPolicy: Policy-Min-TLS-1-2-2019-07
      AccessPolicies: '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"*"},"Action":"es:ESHttpGet","Resource":"*"}]}'
Outputs:
  DomainEndpoint:
    Value: !GetAtt SearchDomain.DomainEndpoint
  DomainArn:
    Value: !GetAtt SearchDomain.Arn
  DomainNameOut:
    Value: !Ref SearchDomain
"#;

/// The smallest declaration of the class, on the other engine line: a node whose storage is
/// the instance's own, so no `EBSOptions`, no encryption containers and no access policy.
const YAML_INSTANCE_STORAGE: &str = r#"Resources:
  SearchDomain:
    Type: AWS::OpenSearchService::Domain
    Properties:
      DomainName: '@DOMAIN@'
      EngineVersion: Elasticsearch_7.10
      ClusterConfig:
        InstanceType: i3.large.search
        InstanceCount: 1
Outputs:
  DomainEndpoint:
    Value: !GetAtt SearchDomain.DomainEndpoint
  DomainArn:
    Value: !GetAtt SearchDomain.Arn
  DomainNameOut:
    Value: !Ref SearchDomain
"#;

/// A declaration carrying no `DomainName` at all, so the physical id — the name the domain is
/// created under — is the one CloudFormation generates for it.
const YAML_GENERATED_NAME: &str = r#"Resources:
  SearchDomain:
    Type: AWS::OpenSearchService::Domain
    Properties:
      EngineVersion: OpenSearch_2.11
      ClusterConfig:
        InstanceType: t3.small.search
        InstanceCount: 1
      EBSOptions:
        EBSEnabled: true
        VolumeType: gp2
        VolumeSize: 10
Outputs:
  DomainEndpoint:
    Value: !GetAtt SearchDomain.DomainEndpoint
  DomainArn:
    Value: !GetAtt SearchDomain.Arn
  DomainNameOut:
    Value: !Ref SearchDomain
"#;

/// The other direction of the rule: a volume below the floor the owning service publishes for
/// the instance type, which that service refuses.
const YAML_UNDERSIZED_VOLUME: &str = r#"Resources:
  SearchDomain:
    Type: AWS::OpenSearchService::Domain
    Properties:
      DomainName: '@DOMAIN@'
      EngineVersion: OpenSearch_2.11
      ClusterConfig:
        InstanceType: t3.small.search
        InstanceCount: 1
      EBSOptions:
        EBSEnabled: true
        VolumeType: gp2
        VolumeSize: 5
Outputs:
  DomainNameOut:
    Value: !Ref SearchDomain
"#;

/// That same create as a direct `CreateDomain` body, which is where the refusal the stack must
/// report is read from.
const UNDERSIZED_VOLUME_REQUEST: &str = r#"{"DomainName":"@DOMAIN@",
  "EngineVersion":"OpenSearch_2.11",
  "ClusterConfig":{"InstanceType":"t3.small.search","InstanceCount":1},
  "EBSOptions":{"EBSEnabled":true,"VolumeType":"gp2","VolumeSize":5}}"#;

const ROWS: &[Row] = &[
    Row {
        case: "a YAML body naming the domain through a parameter, with the full option set",
        stack: "sim-osd-yaml",
        domain: Some("sim-osd-yaml"),
        template: YAML_PARAMETERISED,
        parameters: &[
            ("DomainName", "sim-osd-yaml"),
            ("EngineVersion", "OpenSearch_2.11"),
        ],
        expect: Expect::Created(Held {
            engine_version: "OpenSearch_2.11",
            instance_type: "t3.small.search",
            volume: Some(("gp2", 10)),
            encrypted: true,
            enforce_https: true,
            access_policy: true,
        }),
    },
    Row {
        case: "the same deployment spelled as JSON, naming the domain literally",
        stack: "sim-osd-json",
        domain: Some("sim-osd-json"),
        template: JSON_LITERAL,
        parameters: &[],
        expect: Expect::Created(Held {
            engine_version: "OpenSearch_2.11",
            instance_type: "t3.small.search",
            volume: Some(("gp2", 10)),
            encrypted: true,
            enforce_https: true,
            access_policy: true,
        }),
    },
    Row {
        case: "a Graviton pair with dedicated masters, gp3 storage and a text policy",
        stack: "sim-osd-graviton",
        domain: Some("sim-osd-graviton"),
        template: YAML_GRAVITON,
        parameters: &[],
        expect: Expect::Created(Held {
            engine_version: "OpenSearch_2.19",
            instance_type: "m6g.large.search",
            volume: Some(("gp3", 100)),
            encrypted: true,
            enforce_https: true,
            access_policy: true,
        }),
    },
    Row {
        case: "a node on instance storage, declaring no EBS, encryption or policy at all",
        stack: "sim-osd-instance",
        domain: Some("sim-osd-instance"),
        template: YAML_INSTANCE_STORAGE,
        parameters: &[],
        expect: Expect::Created(Held {
            engine_version: "Elasticsearch_7.10",
            instance_type: "i3.large.search",
            volume: None,
            encrypted: false,
            enforce_https: false,
            access_policy: false,
        }),
    },
    Row {
        case: "a domain CloudFormation names, because the template declares no DomainName",
        stack: "sim-osd-generated",
        domain: None,
        template: YAML_GENERATED_NAME,
        parameters: &[],
        expect: Expect::Created(Held {
            engine_version: "OpenSearch_2.11",
            instance_type: "t3.small.search",
            volume: Some(("gp2", 10)),
            encrypted: false,
            enforce_https: false,
            access_policy: false,
        }),
    },
    Row {
        case: "a volume below the floor the owning service publishes",
        stack: "sim-osd-undersized",
        domain: Some("sim-osd-undersized"),
        template: YAML_UNDERSIZED_VOLUME,
        parameters: &[],
        expect: Expect::Refused(UNDERSIZED_VOLUME_REQUEST),
    },
];

/// The row's body with the domain's name in place of the placeholder it carries.
fn body_of(text: &str, domain: &str) -> String {
    text.replace(DOMAIN, domain)
}

/// The `Parameters.member.N.*` pairs for a row's declared parameters.
fn parameter_members(row: &Row) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for (index, (key, value)) in row.parameters.iter().enumerate() {
        let position = index + 1;
        pairs.push((
            format!("Parameters.member.{position}.ParameterKey"),
            (*key).to_string(),
        ));
        pairs.push((
            format!("Parameters.member.{position}.ParameterValue"),
            (*value).to_string(),
        ));
    }
    pairs
}

/// Deploy a row's template, and answer the body the stack was last described with once it
/// reached a terminal status.
async fn deploy(router: &MultiServiceRouter, row: &Row) -> String {
    let case = row.case;
    let template = body_of(row.template, row.domain.unwrap_or_default());
    let parameters = parameter_members(row);
    let mut members: Vec<(&str, &str)> =
        vec![("StackName", row.stack), ("TemplateBody", &template)];
    for (key, value) in &parameters {
        members.push((key.as_str(), value.as_str()));
    }
    let (status, body) = cloudformation(router, "CreateStack", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from CreateStack, got {status} with {body}"
    );
    settle_stack(router, case, row.stack).await
}

/// The account id an ARN names, which is its fifth colon-separated field.
fn account_of(arn: &str) -> &str {
    arn.split(':').nth(4).unwrap_or_default()
}

/// Whether `name` is a name the owning service's `DomainName` admits — the bound and the
/// pattern that member publishes, which a name CloudFormation generated has to satisfy too.
fn is_a_legal_domain_name(name: &str) -> bool {
    let mut characters = name.chars();
    DOMAIN_NAME_LENGTH.contains(&name.chars().count())
        && characters
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|rest| rest.is_ascii_lowercase() || rest.is_ascii_digit() || rest == '-')
}

/// Every create a row declares that the owning service accepts leaves the stack complete, its
/// outputs carrying the identifiers that service minted, and the domain holding what the
/// template declared.
#[tokio::test]
async fn a_single_domain_template_deploys_and_the_domain_reads_back() {
    let router = build_router();

    for row in ROWS {
        let Expect::Created(held) = &row.expect else {
            continue;
        };
        let case = row.case;
        let described = deploy(&router, row).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "CREATE_COMPLETE",
            "case {case:?}: a template the owning service accepts must deploy, got {described} \
             with reasons {:?}",
            resource_reasons(&router, row.stack).await
        );

        // Every output resolves, which is how a consumer of the stack reaches the domain at
        // all. The name is the stack's `Ref`: the one the template asked for, or the one
        // CloudFormation generated, which has to be a name the owning service admits.
        let outputs: BTreeMap<String, String> = outputs_of(&described);
        let name = outputs
            .get("DomainNameOut")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a DomainNameOut, got {outputs:?}"));
        match row.domain {
            Some(declared) => assert_eq!(
                name, declared,
                "case {case:?}: expected the declared name, got {outputs:?}"
            ),
            None => assert!(
                is_a_legal_domain_name(&name),
                "case {case:?}: a generated domain name must be one the owning service admits, \
                 got {name:?}"
            ),
        }

        // The domain the stack reports is the one that service holds, with the configuration
        // the template declared.
        let (status, reported) = describe_domain(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the created domain to read back, got {status} with \
             {reported}"
        );
        assert_eq!(
            reported.get("EngineVersion").and_then(Value::as_str),
            Some(held.engine_version),
            "case {case:?}: expected the declared release, got {reported}"
        );
        assert_eq!(
            reported
                .get("ClusterConfig")
                .and_then(|cluster| cluster.get("InstanceType"))
                .and_then(Value::as_str),
            Some(held.instance_type),
            "case {case:?}: expected the declared instance type, got {reported}"
        );
        let ebs = reported.get("EBSOptions");
        match held.volume {
            None => assert!(
                ebs.is_none_or(Value::is_null),
                "case {case:?}: a declaration carrying no EBSOptions must leave the domain with \
                 none, got {ebs:?}"
            ),
            Some((volume_type, size)) => {
                assert_eq!(
                    ebs.and_then(|options| options.get("VolumeType"))
                        .and_then(Value::as_str),
                    Some(volume_type),
                    "case {case:?}: expected the declared volume type, got {ebs:?}"
                );
                assert_eq!(
                    ebs.and_then(|options| options.get("VolumeSize"))
                        .and_then(Value::as_i64),
                    Some(size),
                    "case {case:?}: expected the declared volume size, got {ebs:?}"
                );
            }
        }
        for (member, declared) in [
            ("EncryptionAtRestOptions", held.encrypted),
            ("NodeToNodeEncryptionOptions", held.encrypted),
        ] {
            let enabled = reported
                .get(member)
                .and_then(|options| options.get("Enabled"))
                .and_then(Value::as_bool);
            // A declaration that turns encryption on is followed; one that declares none
            // leaves the domain with the value the SERVICE sets for it, which is off —
            // the owning service provisions a domain with a whole configuration.
            assert_eq!(
                enabled,
                Some(declared),
                "case {case:?}: expected {member} to follow the declaration, got {reported}"
            );
        }
        assert_eq!(
            reported
                .get("DomainEndpointOptions")
                .and_then(|options| options.get("EnforceHTTPS"))
                .and_then(Value::as_bool),
            Some(held.enforce_https),
            "case {case:?}: expected EnforceHTTPS to follow the declaration, got {reported}"
        );

        // The identifiers the outputs publish are the ones that service minted, never ones
        // the stack rebuilt.
        let arn = reported
            .get("ARN")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let endpoint = reported
            .get("Endpoint")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !arn.is_empty() && !endpoint.is_empty(),
            "case {case:?}: the owning service must report an ARN and an endpoint, got {reported}"
        );
        for (key, expected) in [("DomainArn", arn), ("DomainEndpoint", endpoint)] {
            assert_eq!(
                outputs.get(key).map(String::as_str),
                Some(expected),
                "case {case:?}: expected {key} to resolve to {expected:?}, got {outputs:?}"
            );
        }

        // An access policy the template declares reaches the member as the TEXT of that
        // document, with every intrinsic inside it resolved against the account the stack ran
        // in.
        let carried = reported.get("AccessPolicies");
        if !held.access_policy {
            // A declaration carrying no policy leaves the domain with the one the SERVICE
            // sets for a create that named none, which is the empty document.
            assert_eq!(
                carried.and_then(Value::as_str),
                Some(""),
                "case {case:?}: a domain declaring no access policy holds the empty document, \
                 got {carried:?}"
            );
            continue;
        }
        let text = carried.and_then(Value::as_str).unwrap_or_else(|| {
            panic!(
                "case {case:?}: the member is modelled a string, so the domain must carry its \
                 text, got {carried:?}"
            )
        });
        let document: Value = serde_json::from_str(text).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: expected the carried text to be the document's own JSON, got \
                 {text:?} ({error})"
            )
        });
        let principal = document
            .pointer("/Statement/0/Principal/AWS")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !principal.is_empty() && !principal.contains("${"),
            "case {case:?}: every intrinsic inside the document must be resolved, got {text:?}"
        );
        if principal != "*" {
            assert_eq!(
                principal,
                format!("arn:aws:iam::{}:root", account_of(arn)),
                "case {case:?}: expected the account root the template delegates to, got {text:?}"
            );
        }
    }
}

/// A create the owning service REFUSES fails the resource with that service's own refusal and
/// rolls the stack back, leaving no domain behind.
#[tokio::test]
async fn a_domain_the_owning_service_refuses_rolls_the_stack_back_with_its_reason() {
    let router = build_router();

    for row in ROWS {
        let Expect::Refused(direct_request) = &row.expect else {
            continue;
        };
        let case = row.case;
        let domain = row.domain.unwrap_or_default();
        let described = deploy(&router, row).await;
        assert_eq!(
            xml_text(&described, "StackStatus"),
            "ROLLBACK_COMPLETE",
            "case {case:?}: a resource the owning service refuses must roll the stack back, got \
             {described}"
        );

        // The refusal the resource reports is the one that service answers the SAME create
        // with when a client asks it directly, so the expected sentence is read from the
        // service rather than written here.
        let direct_name = format!("{domain}-direct");
        let request: Value = serde_json::from_str(&body_of(direct_request, &direct_name))
            .expect("the row's direct request is JSON");
        let (direct_status, direct_body) = opensearch(
            &router,
            "POST",
            "/2021-01-01/opensearch/domain",
            Some(request),
        )
        .await;
        assert!(
            (400..500).contains(&direct_status),
            "case {case:?}: the row's premise is that this create is refused, got \
             {direct_status} with {direct_body}"
        );
        let direct_message = direct_body
            .get("message")
            .or_else(|| direct_body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !direct_message.is_empty(),
            "case {case:?}: expected the service's refusal to carry a message, got {direct_body}"
        );
        let reasons = resource_reasons(&router, row.stack).await;
        assert!(
            reasons.iter().any(|reason| reason.contains(&direct_message)),
            "case {case:?}: expected a resource event carrying {direct_message:?}, got {reasons:?}"
        );

        // Nothing survives the rollback: the domain was never created.
        let (status, reported) = describe_domain(&router, domain).await;
        assert_eq!(
            status, MISSING_RESOURCE_STATUS,
            "case {case:?}: the refused domain must not exist, got {status} with {reported}"
        );
    }
}
