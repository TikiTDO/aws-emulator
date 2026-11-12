// SPDX-License-Identifier: CC0-1.0

//! How CloudWatchOmni SHAPES the values in a success response.
//!
//! Three rules, each asserted over a table of resources rather than one recorded
//! exchange, and each checked on the rpcv2Cbor wire as well as through the handlers:
//!
//! * **ARNs.** A resource's ARN names the resource type and the id the backend minted
//!   for it, in this service's ARN namespace, the signed region and the emulator
//!   account. The resource path addresses the resource itself: a resource whose own id
//!   addresses it carries that id alone — no parent segment — and a view, which is
//!   addressed by a name the account may release and take again, carries its name
//!   followed by the id minted for it.
//! * **Timestamps.** Every modelled `Timestamp` member carries a whole-second epoch
//!   instant, serialized on the wire as an RFC 8949 tag(1) epoch date-time item.
//! * **Empty aggregates.** A resource's own aggregate members — its tag map, its
//!   principal's attribute conditions, its identity-provider configuration — are
//!   reported as the EMPTY aggregate when the resource holds nothing in them, never
//!   dropped from the response.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row
//! of the table that broke.

use std::collections::HashMap;
use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use aws_smithy_types::DateTime;
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_access_grant::CreateAccessGrant;
use omni::ops::create_access_profile::CreateAccessProfile;
use omni::ops::create_alert::CreateAlert;
use omni::ops::create_domain::CreateDomain;
use omni::ops::create_omni_dashboard::CreateOmniDashboard;
use omni::ops::create_space::CreateSpace;
use omni::ops::create_view::CreateView;
use omni::ops::get_domain::GetDomain;
use omni::ops::get_omni_dashboard::GetOmniDashboard;
use omni::ops::types::{
    AccessGrantPrincipal, AccessGrantPrincipalAttribute, AlertCondition, AlertEvaluation,
    AlertRuleQuery, CreateAccessGrantInput, CreateAccessProfileInput, CreateAccessProfileOutput,
    CreateAlertInput, CreateDomainInput, CreateOmniDashboardInput, CreateOmniDashboardOutput,
    CreateSpaceInput, CreateViewInput, CreateViewOutput, GetAccessGrantInput, GetAccessGrantOutput,
    GetDomainInput, GetDomainOutput, GetOmniDashboardInput, IdentityCenterConfiguration,
    IdentityProviderConfiguration, ResourceScope, Rule, ScopedActions, TelemetryRule,
    CREATE_ACCESS_PROFILE_INPUT_SCHEMA, CREATE_OMNI_DASHBOARD_INPUT_SCHEMA,
    CREATE_VIEW_INPUT_SCHEMA, GET_ACCESS_GRANT_INPUT_SCHEMA, GET_DOMAIN_INPUT_SCHEMA,
};
use omni::state::{
    ACCESS_GRANT_ARN_RESOURCE_TYPE, ACCESS_PROFILE_ARN_RESOURCE_TYPE, ACCESS_PROFILE_RESOURCE_TYPE,
    ALERT_ARN_RESOURCE_TYPE, ASSUME_ACCESS_PROFILE_ACTION, DOMAIN_ENDPOINT_HOST,
    OMNI_DASHBOARD_ARN_RESOURCE_TYPE, VIEW_ARN_RESOURCE_TYPE,
};

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN below is expected to carry it.
const REGION: &str = "us-west-2";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// The smallest dashboard body the service admits: a definition laying out no panel.
const EMPTY_PANELS_BODY: &str = r#"{"panels":[]}"#;

/// A `definition` the SQL parser reads as one query, which `CreateView` requires.
const VIEW_DEFINITION: &str = "SELECT service, count(*) FROM logs GROUP BY service";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key) and the region the handlers record.
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/cloudwatch/aws4_request, \
                 SignedHeaders=host, Signature=emulator"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Omni {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below all hold the SAME
        // engine, so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and the body.
    async fn over_the_wire<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor");
        for (name, value) in &self.parts.headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// The ARN segments of a resource: everything after `arn:aws:<service>:<region>:<account>:`,
/// split on `/`.
///
/// The prefix is asserted too, so a row proves the whole ARN rather than its tail.
fn resource_path(case: &str, arn: &str) -> Vec<String> {
    let prefix = format!(
        "arn:aws:cloudwatch:{REGION}:{}:",
        engine::emulator_account_id()
    );
    let resource = arn.strip_prefix(&prefix).unwrap_or_else(|| {
        panic!("case {case:?}: expected an ARN under {prefix:?}, got {arn:?}")
    });
    assert!(
        !resource.is_empty(),
        "case {case:?}: the ARN names no resource, got {arn:?}"
    );
    resource.split('/').map(str::to_string).collect()
}

/// Whether `value` is a canonical lowercase hyphenated v4 UUID — the form this backend
/// mints an id in when the model documents no prefix for it.
fn is_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        })
}

/// The bytes an RFC 8949 tag(1) epoch date-time item carries for `at`: the tag head
/// (major type 6, value 1) followed by a double-precision epoch-seconds value.
///
/// Source: RFC 8949 §3.4.2 (tag number 1, "Epoch-Based Date/Time"), which the
/// `rpcv2Cbor` protocol requires for a `Timestamp` member.
fn tagged_epoch_bytes(at: DateTime) -> Vec<u8> {
    let mut bytes = vec![0xc1, 0xfb];
    bytes.extend_from_slice(&at.as_secs_f64().to_be_bytes());
    bytes
}

/// Require that `body` carries `at` as a tag(1) epoch date-time item.
fn assert_tagged_timestamp(case: &str, body: &[u8], at: DateTime) {
    let expected = tagged_epoch_bytes(at);
    assert!(
        body.windows(expected.len()).any(|window| window == expected),
        "case {case:?}: the response does not carry {at:?} as a tag(1) epoch date-time item \
         ({expected:02x?}); body {body:02x?}"
    );
}

/// Require that every instant in `instants` is a whole-second epoch value.
fn assert_whole_seconds(case: &str, instants: &[(&str, DateTime)]) {
    for (member, at) in instants {
        assert_eq!(
            at.subsec_nanos(),
            0,
            "case {case:?}: {member} must be a whole-second epoch instant, got {at:?}"
        );
        assert!(
            at.secs() > 0,
            "case {case:?}: {member} must be a real instant, got {at:?}"
        );
    }
}

/// A domain, a space of it, and an access profile of that space — the parents the rest
/// of the resources need.
struct Parents {
    domain_id: String,
    space_id: String,
    profile_id: String,
    profile_arn: String,
}

impl Omni {
    async fn parents(&self, case: &str, name: &str) -> Parents {
        let domain_id = CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: format!("omni-{name}"),
                    identity_providers: vec!["IAM".to_string()],
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain
            .domain_id;
        let space_id = CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id: domain_id.clone(),
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space
            .space_id;
        let profile = CreateAccessProfile::new(Arc::clone(&self.engine))
            .handle(
                CreateAccessProfileInput {
                    space_id: space_id.clone(),
                    name: format!("{name} profile"),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: CreateAccessProfile failed: {error:?}")
            })
            .access_profile;
        Parents {
            domain_id,
            space_id,
            profile_id: profile.profile_id,
            profile_arn: profile.arn,
        }
    }
}

/// The rule: a resource's ARN carries its resource type and the id addressing the
/// resource, and nothing else — a parent's id is reported by the member that names it
/// (`spaceId`, `domainArn`), never folded into the child's ARN.
///
/// The rows span every resource kind this service mints an ARN for, so the rule is
/// asserted on the ones the recording cited AND on the ones it did not.
#[tokio::test]
async fn every_resource_arn_names_its_resource_type_and_its_own_id() {
    let omni = Omni::new();
    let parents = omni.parents("arns", "arn-table").await;

    // Rows: (case, the ARN, the resource path it must carry).
    let mut rows: Vec<(String, String, Vec<String>)> = Vec::new();

    let domain = GetDomain::new(Arc::clone(&omni.engine))
        .handle(
            GetDomainInput {
                domain_id: parents.domain_id.clone(),
            },
            &omni.ctx(),
        )
        .await
        .expect("GetDomain answers for the domain just created")
        .domain;
    rows.push((
        "a domain".to_string(),
        domain.domain_arn.clone(),
        vec!["domain".to_string(), parents.domain_id.clone()],
    ));

    rows.push((
        "an access profile".to_string(),
        parents.profile_arn.clone(),
        vec![
            ACCESS_PROFILE_ARN_RESOURCE_TYPE.to_string(),
            parents.profile_id.clone(),
        ],
    ));

    let grant = CreateAccessGrant::new(Arc::clone(&omni.engine))
        .handle(
            CreateAccessGrantInput {
                domain_id: parents.domain_id.clone(),
                space_id: parents.space_id.clone(),
                name: "arn-grant".to_string(),
                permission: "READ".to_string(),
                principal: AccessGrantPrincipal {
                    principal_type: "IAM_ROLE".to_string(),
                    principal_id: Some("arn:aws:iam::000000000000:role/Reader".to_string()),
                    principal_attributes: None,
                },
                ..Default::default()
            },
            &omni.ctx(),
        )
        .await
        .expect("CreateAccessGrant in an existing space")
        .access_grant;
    rows.push((
        "an access grant".to_string(),
        grant.grant_arn.clone(),
        vec![
            ACCESS_GRANT_ARN_RESOURCE_TYPE.to_string(),
            grant.grant_id.clone(),
        ],
    ));

    let dashboard = CreateOmniDashboard::new(Arc::clone(&omni.engine))
        .handle(
            CreateOmniDashboardInput {
                space_id: parents.space_id.clone(),
                name: "arn-dashboard".to_string(),
                body: EMPTY_PANELS_BODY.to_string(),
                ..Default::default()
            },
            &omni.ctx(),
        )
        .await
        .expect("CreateOmniDashboard in an existing space")
        .omni_dashboard;
    rows.push((
        "a dashboard".to_string(),
        dashboard.arn.clone(),
        vec![
            OMNI_DASHBOARD_ARN_RESOURCE_TYPE.to_string(),
            dashboard.dashboard_id.clone(),
        ],
    ));

    let alert = {
        // An alert may only name a profile it is trusted to assume, so the trust grant
        // comes first: that is the grant the handler reads.
        CreateAccessGrant::new(Arc::clone(&omni.engine))
            .handle(
                CreateAccessGrantInput {
                    domain_id: parents.domain_id.clone(),
                    space_id: parents.space_id.clone(),
                    name: "arn-alert-trust".to_string(),
                    permission: "CUSTOM".to_string(),
                    principal: AccessGrantPrincipal {
                        principal_type: "ALERT".to_string(),
                        principal_id: Some("arn-alert".to_string()),
                        principal_attributes: None,
                    },
                    scoped_actions: Some(vec![ScopedActions {
                        actions: vec![ASSUME_ACCESS_PROFILE_ACTION.to_string()],
                        resources: Some(vec![ResourceScope {
                            resource_type: ACCESS_PROFILE_RESOURCE_TYPE.to_string(),
                            resource_arns: Some(vec![parents.profile_arn.clone()]),
                            tags: None,
                            signal_types: None,
                            row_scope_groups: None,
                        }]),
                        context_conditions: None,
                    }]),
                    ..Default::default()
                },
                &omni.ctx(),
            )
            .await
            .expect("the alert's trust grant");
        CreateAlert::new(Arc::clone(&omni.engine))
        .handle(
            CreateAlertInput {
                space_id: parents.space_id.clone(),
                profile_id: parents.profile_id.clone(),
                name: "arn-alert".to_string(),
                rule: Rule::telemetryRule(TelemetryRule {
                    query: Some(AlertRuleQuery {
                        language: "SQL".to_string(),
                        expression: "SELECT count(*) FROM logs".to_string(),
                    }),
                    condition: Some(AlertCondition {
                        threshold_mode: Some("COUNT_OF_RESULTS".to_string()),
                        comparator: Some("GT".to_string()),
                        critical_threshold: Some(10.0),
                        ..Default::default()
                    }),
                    evaluation: Some(AlertEvaluation {
                        interval_seconds: 60,
                        ..Default::default()
                    }),
                    no_data: None,
                }),
                description: None,
                notifications_enabled: None,
                notification_rules: None,
                tags: None,
                client_token: None,
            },
            &omni.ctx(),
        )
        .await
        .expect("CreateAlert for an assumable profile")
        .alert
    };
    let alert_id = alert
        .alert_id
        .clone()
        .expect("a created alert reports the id minted for it");
    rows.push((
        "an alert".to_string(),
        alert.alert_arn.clone(),
        vec![ALERT_ARN_RESOURCE_TYPE.to_string(), alert_id],
    ));

    for (case, arn, expected) in &rows {
        assert_eq!(
            &resource_path(case, arn),
            expected,
            "case {case:?}: the ARN {arn:?} does not name {expected:?}"
        );
        assert!(
            !arn.contains(&parents.space_id),
            "case {case:?}: the ARN {arn:?} must not fold in the parent space id"
        );
    }

    // A view is addressed by a name rather than by a minted id, so BOTH appear: the
    // name, then the id minted for that view.
    let mut view_ids: Vec<String> = Vec::new();
    for name in ["view.arn_a", "view.arn-b", "view.0"] {
        let created = CreateView::new(Arc::clone(&omni.engine))
            .handle(
                CreateViewInput {
                    name: name.to_string(),
                    definition: VIEW_DEFINITION.to_string(),
                    ..Default::default()
                },
                &omni.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: CreateView failed: {error:?}"))
            .arn;
        let path = resource_path(name, &created);
        assert_eq!(
            path.len(),
            3,
            "case {name:?}: a view ARN carries its type, its name and its id, got {created:?}"
        );
        assert_eq!(
            (path[0].as_str(), path[1].as_str()),
            (VIEW_ARN_RESOURCE_TYPE, name),
            "case {name:?}: the ARN {created:?} does not name this view"
        );
        assert!(
            is_uuid(&path[2]),
            "case {name:?}: the ARN's view id {:?} is not an id this backend mints",
            path[2]
        );
        assert!(
            !view_ids.contains(&path[2]),
            "case {name:?}: the view id {:?} repeats an earlier view's",
            path[2]
        );
        view_ids.push(path[2].clone());
    }
}

/// The rule: every modelled `Timestamp` member carries a whole-second epoch instant,
/// and the rpcv2Cbor wire form of one is an RFC 8949 tag(1) epoch date-time item.
///
/// The rows span the resources whose timestamps a caller reads back, and each is checked
/// through the handler AND in the bytes the wire call answered with.
#[tokio::test]
async fn every_timestamp_is_a_whole_second_tagged_epoch_instant() {
    let omni = Omni::new();
    let parents = omni.parents("timestamps", "ts-table").await;
    let codec = CborCodec::default();

    // Row 1: a create's own response.
    let (status, body) = omni
        .over_the_wire(
            "CreateAccessProfile",
            CREATE_ACCESS_PROFILE_INPUT_SCHEMA,
            &CreateAccessProfileInput {
                space_id: parents.space_id.clone(),
                name: "ts profile".to_string(),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(status, 200, "CreateAccessProfile over the wire answered {status}");
    let profile = {
        let mut deserializer = codec.create_deserializer(&body);
        CreateAccessProfileOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .access_profile
    };
    assert_whole_seconds(
        "CreateAccessProfile",
        &[
            ("createdAt", profile.created_at),
            ("updatedAt", profile.updated_at),
        ],
    );
    assert_tagged_timestamp("CreateAccessProfile", &body, profile.created_at);

    // Row 2: a reporting operation's response for a resource created earlier.
    let (status, body) = omni
        .over_the_wire(
            "GetDomain",
            GET_DOMAIN_INPUT_SCHEMA,
            &GetDomainInput {
                domain_id: parents.domain_id.clone(),
            },
        )
        .await;
    assert_eq!(status, 200, "GetDomain over the wire answered {status}");
    let domain = {
        let mut deserializer = codec.create_deserializer(&body);
        GetDomainOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .domain
    };
    assert_whole_seconds(
        "GetDomain",
        &[
            ("createdAt", domain.created_at),
            ("updatedAt", domain.updated_at),
        ],
    );
    assert_tagged_timestamp("GetDomain", &body, domain.updated_at);

    // Row 3: a dashboard, whose create reports both instants.
    let (status, body) = omni
        .over_the_wire(
            "CreateOmniDashboard",
            CREATE_OMNI_DASHBOARD_INPUT_SCHEMA,
            &CreateOmniDashboardInput {
                space_id: parents.space_id.clone(),
                name: "ts-dashboard".to_string(),
                body: EMPTY_PANELS_BODY.to_string(),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(status, 200, "CreateOmniDashboard over the wire answered {status}");
    let dashboard = {
        let mut deserializer = codec.create_deserializer(&body);
        CreateOmniDashboardOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .omni_dashboard
    };
    assert_whole_seconds(
        "CreateOmniDashboard",
        &[
            ("createdAt", dashboard.created_at),
            ("updatedAt", dashboard.updated_at),
        ],
    );
    assert_tagged_timestamp("CreateOmniDashboard", &body, dashboard.created_at);

    // Row 4: a view, whose timestamps are top-level members of the response.
    let (status, body) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &CreateViewInput {
                name: "view.timestamps".to_string(),
                definition: VIEW_DEFINITION.to_string(),
                ..Default::default()
            },
        )
        .await;
    assert_eq!(status, 200, "CreateView over the wire answered {status}");
    let view = {
        let mut deserializer = codec.create_deserializer(&body);
        CreateViewOutput::deserialize(&mut deserializer).expect("the output deserializes")
    };
    assert_whole_seconds(
        "CreateView",
        &[
            ("createdAt", view.created_at),
            ("updatedAt", view.updated_at),
        ],
    );
    assert_tagged_timestamp("CreateView", &body, view.created_at);
}

/// The rule: a resource's own aggregate members are reported as the EMPTY aggregate
/// when the resource holds nothing in them.
///
/// Rows span the three aggregate kinds the service reports — a map (`tags`), a list
/// (`principalAttributes`) and a structure (`identityProviderConfiguration`) — each with
/// nothing, one entry and many, and each read back over the wire so an OMITTED member is
/// distinguishable from an empty one (an omitted member deserializes to `None`).
/// One `principalAttributes` row: the case, the principal type it names, and the
/// attribute conditions the request carries (absent, one, several).
type AttributeRow = (&'static str, &'static str, Option<Vec<(&'static str, &'static str)>>);

#[tokio::test]
async fn a_resources_own_aggregate_members_are_reported_empty_rather_than_omitted() {
    let omni = Omni::new();
    let parents = omni.parents("aggregates", "agg-table").await;
    let codec = CborCodec::default();

    // tags: a map the dashboard always has.
    let tag_rows: Vec<(&str, Option<HashMap<String, String>>)> = vec![
        ("no tags member at all", None),
        ("an empty tag map", Some(HashMap::new())),
        (
            "one tag",
            Some(HashMap::from([("team".to_string(), "sre".to_string())])),
        ),
        (
            "the most tags the model admits",
            Some(
                (0..50)
                    .map(|index| (format!("key{index}"), format!("value{index}")))
                    .collect(),
            ),
        ),
    ];
    for (index, (case, tags)) in tag_rows.into_iter().enumerate() {
        let (status, body) = omni
            .over_the_wire(
                "CreateOmniDashboard",
                CREATE_OMNI_DASHBOARD_INPUT_SCHEMA,
                &CreateOmniDashboardInput {
                    space_id: parents.space_id.clone(),
                    name: format!("agg-dashboard-{index}"),
                    body: EMPTY_PANELS_BODY.to_string(),
                    tags: tags.clone(),
                    ..Default::default()
                },
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: CreateOmniDashboard answered {status}");
        let created = {
            let mut deserializer = codec.create_deserializer(&body);
            CreateOmniDashboardOutput::deserialize(&mut deserializer)
                .expect("the output deserializes")
                .omni_dashboard
        };
        assert_eq!(
            created.tags.as_ref(),
            Some(&tags.clone().unwrap_or_default()),
            "case {case:?}: expected the dashboard's tag map, got {:?}",
            created.tags
        );

        // And the reader reports the same map, from the store.
        let read = GetOmniDashboard::new(Arc::clone(&omni.engine))
            .handle(
                GetOmniDashboardInput {
                    space_id: parents.space_id.clone(),
                    dashboard_id: created.dashboard_id.clone(),
                },
                &omni.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetOmniDashboard failed: {error:?}"))
            .omni_dashboard;
        assert_eq!(
            read.tags, created.tags,
            "case {case:?}: the stored dashboard reports {:?}, the create reported {:?}",
            read.tags, created.tags
        );
    }

    // principalAttributes: a list every grant principal has. `principalAttributes` is
    // "Supported only for IDC_USER principals", so the rows that carry one name that
    // principal type.
    let attribute_rows: Vec<AttributeRow> = vec![
        ("a role principal, no attributes member", "IAM_ROLE", None),
        (
            "an IDC user, one attribute condition",
            "IDC_USER",
            Some(vec![("department", "sre")]),
        ),
        (
            "an IDC user, several attribute conditions",
            "IDC_USER",
            Some(vec![("department", "sre"), ("region", "emea")]),
        ),
    ];
    for (index, (case, principal_type, attributes)) in attribute_rows.into_iter().enumerate() {
        let principal_attributes = attributes.as_ref().map(|pairs| {
            pairs
                .iter()
                .map(|(key, value)| AccessGrantPrincipalAttribute {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                })
                .collect::<Vec<_>>()
        });
        let created = CreateAccessGrant::new(Arc::clone(&omni.engine))
            .handle(
                CreateAccessGrantInput {
                    domain_id: parents.domain_id.clone(),
                    space_id: parents.space_id.clone(),
                    name: format!("agg-grant-{index}"),
                    permission: "READ".to_string(),
                    principal: AccessGrantPrincipal {
                        principal_type: principal_type.to_string(),
                        principal_id: Some(format!("principal-{index}")),
                        principal_attributes: principal_attributes.clone(),
                    },
                    ..Default::default()
                },
                &omni.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateAccessGrant failed: {error:?}"))
            .access_grant;
        let reported: Vec<(String, String)> = created
            .principal
            .principal_attributes
            .as_ref()
            .unwrap_or_else(|| {
                panic!("case {case:?}: principalAttributes must be present, got None")
            })
            .iter()
            .map(|attribute| (attribute.key.clone(), attribute.value.clone()))
            .collect();
        let expected: Vec<(String, String)> = attributes
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the principal's attribute conditions {expected:?}, got {reported:?}"
        );

        // Over the wire, where an omitted member would deserialize to `None`.
        let (status, body) = omni
            .over_the_wire(
                "GetAccessGrant",
                GET_ACCESS_GRANT_INPUT_SCHEMA,
                &GetAccessGrantInput {
                    grant_id: created.grant_id.clone(),
                },
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: GetAccessGrant answered {status}");
        let fetched = {
            let mut deserializer = codec.create_deserializer(&body);
            GetAccessGrantOutput::deserialize(&mut deserializer)
                .expect("the output deserializes")
                .access_grant
        };
        assert_eq!(
            fetched
                .principal
                .principal_attributes
                .as_ref()
                .map(Vec::len),
            Some(expected.len()),
            "case {case:?}: the wire read reports principalAttributes {:?}",
            fetched.principal.principal_attributes
        );
        assert_eq!(
            fetched.grant_arn, created.grant_arn,
            "case {case:?}: the wire read reports {:?} for the created {:?}",
            fetched.grant_arn, created.grant_arn
        );
    }

    // identityProviderConfiguration: a structure every domain has, and the endpoint URLs
    // a domain publishes, which carry the global application host rather than a region.
    let domain_rows: Vec<(&str, Vec<&str>, Option<IdentityProviderConfiguration>)> = vec![
        ("an IAM-only domain, no configuration member", vec!["IAM"], None),
        (
            "an IAM-only domain, an empty configuration",
            vec!["IAM"],
            Some(IdentityProviderConfiguration::default()),
        ),
        (
            "an Identity Center domain",
            vec!["IAM", "IDC"],
            Some(IdentityProviderConfiguration {
                identity_center_configuration: Some(IdentityCenterConfiguration {
                    identity_center_instance_arn: Some(
                        "arn:aws:sso:::instance/ssoins-1234567890abcdef".to_string(),
                    ),
                }),
            }),
        ),
    ];
    for (index, (case, providers, configuration)) in domain_rows.into_iter().enumerate() {
        let name = format!("omni-agg-domain-{index}");
        let created = CreateDomain::new(Arc::clone(&omni.engine))
            .handle(
                CreateDomainInput {
                    name: name.clone(),
                    identity_providers: providers.iter().map(|p| (*p).to_string()).collect(),
                    identity_provider_configuration: configuration.clone(),
                    ..Default::default()
                },
                &omni.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain;

        let (status, body) = omni
            .over_the_wire(
                "GetDomain",
                GET_DOMAIN_INPUT_SCHEMA,
                &GetDomainInput {
                    domain_id: created.domain_id.clone(),
                },
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: GetDomain answered {status}");
        let fetched = {
            let mut deserializer = codec.create_deserializer(&body);
            GetDomainOutput::deserialize(&mut deserializer)
                .expect("the output deserializes")
                .domain
        };
        let reported = fetched
            .identity_provider_configuration
            .as_ref()
            .unwrap_or_else(|| {
                panic!("case {case:?}: identityProviderConfiguration must be present, got None")
            });
        assert_eq!(
            reported
                .identity_center_configuration
                .as_ref()
                .and_then(|center| center.identity_center_instance_arn.clone()),
            configuration
                .and_then(|given| given.identity_center_configuration)
                .and_then(|center| center.identity_center_instance_arn),
            "case {case:?}: the reported configuration is {reported:?}"
        );

        // The endpoint URLs a domain publishes are global: the domain id and the name
        // under the application host, with no region anywhere.
        assert_eq!(
            fetched.domain_endpoint_url,
            format!(
                "https://{}.cloudwatch-omni.{DOMAIN_ENDPOINT_HOST}",
                fetched.domain_id
            ),
            "case {case:?}: domainEndpointUrl is {:?}",
            fetched.domain_endpoint_url
        );
        assert_eq!(
            fetched.custom_endpoint_urls.as_deref(),
            Some(
                [format!("https://{name}.cloudwatch-omni.{DOMAIN_ENDPOINT_HOST}")].as_slice()
            ),
            "case {case:?}: customEndpointUrls are {:?}",
            fetched.custom_endpoint_urls
        );
        assert!(
            !fetched.domain_endpoint_url.contains(REGION)
                && !fetched
                    .custom_endpoint_urls
                    .iter()
                    .flatten()
                    .any(|url| url.contains(REGION)),
            "case {case:?}: a global endpoint URL must carry no region, got {:?} and {:?}",
            fetched.domain_endpoint_url,
            fetched.custom_endpoint_urls
        );
    }
}
