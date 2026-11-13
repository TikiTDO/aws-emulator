// SPDX-License-Identifier: CC0-1.0

//! The lineage GROUP every lineage entity of an account belongs to, end to end over the
//! wire.
//!
//! THE RULE: SageMaker ML Lineage Tracking gives an account one lineage group per Region
//! with no call creating it, and reports its ARN as the `LineageGroupArn` of each entity in
//! it. That group carries SageMaker's own FIXED name, the same in every Region — what makes
//! two Regions' groups two groups is the Region the ARN names, not the name — and the group
//! is readable under that name through `DescribeLineageGroup` wherever an entity reports it.
//!
//! The rows differ along the entity KIND that reports the member (an action, an artifact, a
//! context and a trial component — every lineage entity whose read models
//! `LineageGroupArn`) and along the Region the caller signed for, including the unsigned
//! fallback.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The Region an unsigned request is answered for.
const DEFAULT_REGION: &str = "us-east-1";

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `target` with a SigV4 credential scope naming `region`, or with no scope at all
    /// when `region` is `None`.
    async fn call_in(&self, region: Option<&str>, target: &str, body: &Value) -> (u16, Value) {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"));
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            );
        }
        let request = builder
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `target` for `region` and require a 200, returning the parsed output.
    async fn ok_in(&self, case: &str, region: Option<&str>, target: &str, body: &Value) -> Value {
        let (status, parsed) = self.call_in(region, target, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200 from {target}, got {status} with {parsed}"
        );
        parsed
    }
}

/// One row: the lineage entity kind, the create that mints one and the read that reports
/// its `LineageGroupArn`.
struct Kind {
    case: &'static str,
    create: &'static str,
    read: &'static str,
    /// The create body, given the label the row names its entity with.
    body: fn(&str) -> Value,
    /// The read body, given that same label.
    request: fn(&str) -> Value,
}

fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            case: "an action",
            create: "CreateAction",
            read: "DescribeAction",
            body: |label| {
                json!({
                    "ActionName": label,
                    "Source": { "SourceUri": format!("s3://bucket/{label}") },
                    "ActionType": "ModelDeployment",
                })
            },
            request: |label| json!({ "ActionName": label }),
        },
        Kind {
            case: "an artifact",
            create: "CreateArtifact",
            read: "DescribeArtifact",
            body: |label| {
                json!({
                    "ArtifactName": label,
                    "Source": { "SourceUri": format!("s3://bucket/{label}") },
                    "ArtifactType": "DataSet",
                })
            },
            // An artifact is addressed by the ARN its create minted, which the caller
            // substitutes for this placeholder.
            request: |arn| json!({ "ArtifactArn": arn }),
        },
        Kind {
            case: "a context",
            create: "CreateContext",
            read: "DescribeContext",
            body: |label| {
                json!({
                    "ContextName": label,
                    "Source": { "SourceUri": format!("s3://bucket/{label}"),
                                "SourceType": "Endpoint" },
                    "ContextType": "Endpoint",
                })
            },
            request: |label| json!({ "ContextName": label }),
        },
        Kind {
            case: "a trial component",
            create: "CreateTrialComponent",
            read: "DescribeTrialComponent",
            body: |label| json!({ "TrialComponentName": label }),
            request: |label| json!({ "TrialComponentName": label }),
        },
    ]
}

#[tokio::test]
async fn every_lineage_entity_reports_its_regions_default_lineage_group() {
    let emulator = Emulator::new();

    // The Regions a caller may sign for, plus the unsigned fallback: the group's NAME is
    // the same for all of them, while its ARN names the Region that answered.
    let regions: [(Option<&str>, &str); 4] = [
        (Some("us-east-1"), "us-east-1"),
        (Some("us-west-2"), "us-west-2"),
        (Some("eu-central-1"), "eu-central-1"),
        (None, DEFAULT_REGION),
    ];

    let mut names: BTreeSet<String> = BTreeSet::new();
    for kind in kinds() {
        for (signed, region) in regions {
            let case = format!("{} in {region} (signed={})", kind.case, signed.is_some());
            let label = format!(
                "{}-{}-{}",
                kind.create.to_lowercase(),
                region.replace('-', ""),
                if signed.is_some() { "signed" } else { "unsigned" }
            );
            let created = emulator
                .ok_in(&case, signed, kind.create, &(kind.body)(&label))
                .await;
            // An artifact is read back by the ARN its create minted; every other entity by
            // the name the create named it with.
            let identifier = if kind.create == "CreateArtifact" {
                created
                    .get("ArtifactArn")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: expected an ArtifactArn in {created}"))
                    .to_string()
            } else {
                label.clone()
            };
            let described = emulator
                .ok_in(&case, signed, kind.read, &(kind.request)(&identifier))
                .await;

            let reported = described
                .get("LineageGroupArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a LineageGroupArn in {described}")
                })
                .to_string();
            let (prefix, name) = reported.rsplit_once('/').unwrap_or((reported.as_str(), ""));
            assert_eq!(
                prefix,
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group"),
                "case {case}: the ARN names the Region that answered, got {reported:?}"
            );
            names.insert(name.to_string());

            // The group is readable under the name the ARN carries, in that Region, and
            // reports the very ARN the entity did: the member is a reference to a group
            // this service holds rather than a string it formatted.
            let group = emulator
                .ok_in(
                    &case,
                    signed,
                    "DescribeLineageGroup",
                    &json!({ "LineageGroupName": name }),
                )
                .await;
            assert_eq!(
                group.get("LineageGroupArn").and_then(Value::as_str),
                Some(reported.as_str()),
                "case {case}: the group's own read reports the ARN the entity carries, got \
                 {group}"
            );
            assert_eq!(
                group.get("LineageGroupName").and_then(Value::as_str),
                Some(name),
                "case {case}: the group's own read reports the name its ARN carries, got \
                 {group}"
            );
        }
    }

    // One name for every entity kind and every Region — the group is SageMaker's own, so
    // nothing of the account or of the Region is spelled into it — and that name satisfies
    // `ExperimentEntityName` (`^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`), so a reader that takes
    // a name can address it.
    let reported: Vec<&String> = names.iter().collect();
    assert_eq!(
        reported.len(),
        1,
        "case every kind in every Region: the default lineage group has ONE name, got \
         {reported:?}"
    );
    let name = reported[0];
    assert!(
        (1..=120).contains(&name.chars().count())
            && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
            && name.chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric())
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "case the default group's name: expected a name ExperimentEntityName admits, got \
         {name:?}"
    );
    assert!(
        !name.contains(ACCOUNT) && !name.contains("us-east-1"),
        "case the default group's name: the name carries neither the account nor a Region, \
         got {name:?}"
    );
}
