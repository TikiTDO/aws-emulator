// SPDX-License-Identifier: CC0-1.0

//! The emulator the suites beside this file drive, and the request helpers they share.
//!
//! Application Auto Scaling does not stand alone: a scalable target is a capacity dimension
//! of a resource that lives in ANOTHER service, a target-tracking policy's alarms live in
//! CloudWatch, and executing a scaling policy moves the capacity of the resource in the
//! service that owns it. So the emulator built here holds those services over ONE engine —
//! exactly what `core/router` assembles — and every call goes through the production
//! `MultiServiceRouter::dispatch`, so what the suites assert is what a signing AWS client
//! would see.
#![allow(dead_code)]

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every test's SigV4 credential scope names; every minted ARN is scoped to it.
pub const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
pub const ACCOUNT: &str = "000000000000";

/// One emulator over one shared engine.
pub struct Emulator {
    router: MultiServiceRouter,
    /// The shared engine, kept so a suite can seed the store of a service no client call can
    /// write — see [`Emulator::seed_metric_data`].
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// Application Auto Scaling, DynamoDB and CloudWatch on one engine, with each name in
    /// `tables` created as a PROVISIONED DynamoDB table of one read and one write unit.
    ///
    /// A scalable target is a capacity dimension OF A RESOURCE, so registering one resolves
    /// that resource in the service that owns it: a suite whose rows register a `dynamodb`
    /// target names the tables those rows need here, rather than relying on a list another
    /// suite maintains.
    pub async fn with_tables(tables: &[&str]) -> Self {
        let emulator = Self::empty();
        for table in tables {
            emulator.create_provisioned_table(table, 1, 1).await;
        }
        emulator
    }

    /// The same emulator with no resource created yet, for a suite whose rows create the
    /// resources they need (or deliberately do not).
    pub fn empty() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suites drive unsigned requests; the `Authorization` header is still what
        // routes each request and names its region, as a signed client's would.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        // Registering a scalable target creates that namespace's service-linked role in IAM,
        // so a suite that reads the role back needs IAM mounted on the same engine — which is
        // what `core/router` assembles too.
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Write datapoints straight into the CloudWatch metric store, the way the AWS service
    /// that OWNS the metric writes them.
    ///
    /// A client cannot publish into the AWS-reserved namespace space: `PutMetricData` refuses
    /// a `Namespace` beginning with `AWS/` ("To avoid conflicts with Amazon Web Services
    /// service namespaces, you should not specify a namespace that begins with AWS/"), so a
    /// row whose load metric is a service-authored series (`AWS/ECS`) seeds it here rather
    /// than over the wire. A customer-namespace series is published over the wire instead, by
    /// the handler, exactly as a client publishes it.
    pub fn seed_metric_data(
        &self,
        namespace: &str,
        metric_name: &str,
        dimensions: &[(&str, &str)],
        at_secs: f64,
        values: &[f64],
    ) {
        use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

        let key = cloudwatch::MetricKey {
            namespace: namespace.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: dimensions
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        };
        let data: Vec<cloudwatch::StoredDatum> = values
            .iter()
            .map(|value| cloudwatch::StoredDatum {
                timestamp_secs: at_secs,
                sample_count: 1.0,
                sum: *value,
                minimum: *value,
                maximum: *value,
                unit: None,
                // The default `StorageResolution` of a published datum.
                storage_resolution: 60,
            })
            .collect();
        self.engine
            .state::<cloudwatch::ComAmazonawsCloudwatchGraniteserviceversion20100801State>()
            .lock()
            .append_datapoints(key, data)
            .expect("the metric store has room for the seeded series");
    }

    /// Call an Application Auto Scaling operation.
    pub async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Call an Application Auto Scaling operation against the endpoint of `region`.
    ///
    /// Application Auto Scaling is a REGIONAL service: the credential scope of a request is
    /// what names the Region it is addressed to, and each Region holds its own scalable
    /// targets. A suite that asserts that partitioning has to be able to call two of them.
    pub async fn call_in(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch_in(
            region,
            "application-autoscaling",
            "application/x-amz-json-1.1",
            &format!("AnyScaleFrontendService.{operation}"),
            body,
        )
        .await
    }

    /// Call a DynamoDB operation.
    pub async fn call_dynamodb(&self, operation: &str, body: Value) -> (u16, Value) {
        self.call_dynamodb_in(REGION, operation, body).await
    }

    /// Call a DynamoDB operation against the endpoint of `region`, whose table store is the
    /// one a scalable target of that Region resolves against.
    pub async fn call_dynamodb_in(
        &self,
        region: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        self.dispatch_in(
            region,
            "dynamodb",
            "application/x-amz-json-1.0",
            &format!("DynamoDB_20120810.{operation}"),
            body,
        )
        .await
    }

    /// Call a CloudWatch operation.
    pub async fn call_cloudwatch(&self, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch(
            "monitoring",
            "application/x-amz-json-1.0",
            &format!("GraniteServiceVersion20100801.{operation}"),
            body,
        )
        .await
    }

    /// Call an Amazon ECS operation.
    pub async fn call_ecs(&self, operation: &str, body: Value) -> (u16, Value) {
        self.dispatch(
            "ecs",
            "application/x-amz-json-1.1",
            &format!("AmazonEC2ContainerServiceV20141113.{operation}"),
            body,
        )
        .await
    }

    /// Create an ECS service of `desired_count` tasks in a cluster of its own, with ONE
    /// container instance registered into that cluster, and answer the `ResourceId` an
    /// `ecs:service:DesiredCount` scalable target names it by.
    ///
    /// The cluster is given capacity because that is what lets the ECS scheduler PLACE the
    /// tasks the service wants: a capacity movement this service performs is fulfilled when
    /// the owning service reports the new count as running, so a cluster with nowhere to run a
    /// task is an estate where no movement is ever fulfilled — which is what
    /// [`Emulator::given_ecs_service_without_capacity`] is for.
    pub async fn given_ecs_service(
        &self,
        cluster: &str,
        service: &str,
        desired_count: i64,
    ) -> String {
        let resource_id = self
            .given_ecs_service_without_capacity(cluster, service, desired_count)
            .await;
        self.given_container_instance(cluster).await;
        resource_id
    }

    /// The same ECS service in a cluster with NO container instance registered: the estate
    /// the EC2 launch type has nowhere to run a task on, so its `desiredCount` moves and its
    /// `runningCount` never reaches it.
    pub async fn given_ecs_service_without_capacity(
        &self,
        cluster: &str,
        service: &str,
        desired_count: i64,
    ) -> String {
        let (status, body) = self
            .call_ecs("CreateCluster", json!({"clusterName": cluster}))
            .await;
        assert_eq!(
            status, 200,
            "case {cluster:?}: expected CreateCluster to answer 200, got {status} {body:?}"
        );
        let family = format!("{service}-task");
        let (status, body) = self
            .call_ecs(
                "RegisterTaskDefinition",
                json!({
                    "family": family,
                    "containerDefinitions": [
                        {"name": "web", "image": "public.ecr.aws/nginx/nginx:latest", "memory": 128},
                    ],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {service:?}: expected RegisterTaskDefinition to answer 200, got {status} {body:?}"
        );
        let (status, body) = self
            .call_ecs(
                "CreateService",
                json!({
                    "cluster": cluster,
                    "serviceName": service,
                    "taskDefinition": family,
                    "launchType": "EC2",
                    "desiredCount": desired_count,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {service:?}: expected CreateService to answer 200, got {status} {body:?}"
        );
        format!("service/{cluster}/{service}")
    }

    /// Register ONE container instance into `cluster`, the way the ECS agent does: the EC2
    /// instance identity document it reads off its own instance metadata, beside the
    /// signature it presents with it.
    ///
    /// "The EC2 launch type runs your tasks on Amazon EC2 instances registered to your
    /// cluster" — so this is what gives a cluster somewhere to run a task.
    pub async fn given_container_instance(&self, cluster: &str) {
        let identity_document = json!({
            "instanceId": "i-0c0ffee0c0ffee0c0",
            "region": REGION,
            "accountId": ACCOUNT,
            "imageId": "ami-0abcdef1234567890",
            "instanceType": "m5.large",
            "availabilityZone": format!("{REGION}a"),
        })
        .to_string();
        let (status, body) = self
            .call_ecs(
                "RegisterContainerInstance",
                json!({
                    "cluster": cluster,
                    "instanceIdentityDocument": identity_document,
                    // What a signature IS of cannot be checked without the key that produced
                    // it; this is base64 text of the length a 2048-bit signature has.
                    "instanceIdentityDocumentSignature": "ZXhhbXBsZQ".repeat(35),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {cluster:?}: expected RegisterContainerInstance to answer 200, \
             got {status} {body:?}"
        );
    }

    /// Call an IAM operation, the way a client asks for one: an awsQuery POST whose
    /// form-encoded body names the action, answered as the XML text of the response.
    ///
    /// IAM speaks awsQuery rather than awsJson, so it does not go through [`Self::dispatch`].
    pub async fn call_iam(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(&form_encode(name));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-www-form-urlencoded")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/iam\
                     /aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 body");
        (status, text)
    }

    /// The provisioned read or write capacity DynamoDB reports for a table, or for one of its
    /// global secondary indexes, as `DescribeTable` answers it.
    ///
    /// This is the capacity a `dynamodb:*:*CapacityUnits` scalable target scales, read where a
    /// client reads it: in the service that owns the table.
    pub async fn provisioned_capacity(&self, resource_id: &str, reads: bool) -> i64 {
        let rest = resource_id.trim_start_matches("table/");
        let (table, index) = match rest.split_once("/index/") {
            Some((table, index)) => (table, Some(index)),
            None => (rest, None),
        };
        let (status, body) = self
            .call_dynamodb("DescribeTable", json!({"TableName": table}))
            .await;
        assert_eq!(
            status, 200,
            "case {resource_id:?}: expected DescribeTable to answer 200, got {status} {body:?}"
        );
        let throughput = match index {
            None => body["Table"]["ProvisionedThroughput"].clone(),
            Some(index) => body["Table"]["GlobalSecondaryIndexes"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|described| described["IndexName"] == json!(index))
                .map(|described| described["ProvisionedThroughput"].clone())
                .unwrap_or(Value::Null),
        };
        let member = if reads {
            "ReadCapacityUnits"
        } else {
            "WriteCapacityUnits"
        };
        throughput[member].as_i64().unwrap_or_else(|| {
            panic!("case {resource_id:?}: expected a {member}, got {throughput:?}")
        })
    }

    /// The `desiredCount` ECS reports for one service, as `DescribeServices` answers it.
    pub async fn ecs_desired_count(&self, cluster: &str, service: &str) -> i64 {
        let (status, body) = self
            .call_ecs(
                "DescribeServices",
                json!({"cluster": cluster, "services": [service]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {service:?}: expected DescribeServices to answer 200, got {status} {body:?}"
        );
        body["services"][0]["desiredCount"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {service:?}: expected a desiredCount, got {body:?}"))
    }

    /// The `runningCount` ECS reports for one service, as `DescribeServices` answers it: "the
    /// number of tasks in the cluster that are in the RUNNING state", which is the capacity
    /// actually in effect for `ecs:service:DesiredCount`.
    pub async fn ecs_running_count(&self, cluster: &str, service: &str) -> i64 {
        let (status, body) = self
            .call_ecs(
                "DescribeServices",
                json!({"cluster": cluster, "services": [service]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {service:?}: expected DescribeServices to answer 200, got {status} {body:?}"
        );
        body["services"][0]["runningCount"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {service:?}: expected a runningCount, got {body:?}"))
    }

    /// Create the CloudWatch alarm a step scaling policy is wired to: the policy's ARN is the
    /// action the alarm executes when it moves into `ALARM`, which is how a client attaches a
    /// step scaling policy to an alarm.
    pub async fn given_policy_alarm(&self, alarm: &PolicyAlarm<'_>) {
        let (status, body) = self
            .call_cloudwatch(
                "PutMetricAlarm",
                json!({
                    "AlarmName": alarm.name,
                    "Namespace": alarm.metric_namespace,
                    "MetricName": alarm.metric_name,
                    "Statistic": "Average",
                    "Period": 60,
                    "EvaluationPeriods": 1,
                    "Threshold": alarm.threshold,
                    "ComparisonOperator": alarm.comparison_operator,
                    "AlarmActions": [alarm.policy_arn],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected PutMetricAlarm to answer 200, got {status} {body:?}",
            alarm.name
        );
    }

    /// Drive one alarm into `state_value` with `SetAlarmState`, the way a client breaches an
    /// alarm without publishing a datapoint for it.
    pub async fn set_alarm_state(&self, alarm_name: &str, state_value: &str) {
        let (status, body) = self
            .call_cloudwatch(
                "SetAlarmState",
                json!({
                    "AlarmName": alarm_name,
                    "StateValue": state_value,
                    "StateReason": "driven by the test suite",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {alarm_name:?}: expected SetAlarmState to answer 200, got {status} {body:?}"
        );
    }

    /// Dispatch one request, returning the HTTP status and the parsed response.
    async fn dispatch(
        &self,
        signing_name: &str,
        content_type: &str,
        target: &str,
        body: Value,
    ) -> (u16, Value) {
        self.dispatch_in(REGION, signing_name, content_type, target, body)
            .await
    }

    /// Dispatch one request whose credential scope names `region`.
    async fn dispatch_in(
        &self,
        region: &str,
        signing_name: &str,
        content_type: &str,
        target: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", content_type)
            .header("x-amz-target", target)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{region}/{signing_name}\
                     /aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Create a PROVISIONED DynamoDB table with `read` and `write` capacity units.
    pub async fn create_provisioned_table(&self, name: &str, read: i64, write: i64) {
        self.create_table(
            name,
            json!({"BillingMode": "PROVISIONED",
                   "ProvisionedThroughput": {"ReadCapacityUnits": read, "WriteCapacityUnits": write}}),
        )
        .await;
    }

    /// Create a PAY_PER_REQUEST (on-demand) DynamoDB table, which has no provisioned
    /// capacity at all.
    pub async fn create_on_demand_table(&self, name: &str) {
        self.create_table(name, json!({"BillingMode": "PAY_PER_REQUEST"}))
            .await;
    }

    /// Create a PROVISIONED table carrying one global secondary index on `index_key`.
    pub async fn create_table_with_index(
        &self,
        name: &str,
        index: &str,
        index_key: &str,
        read: i64,
        write: i64,
    ) {
        self.create_table(
            name,
            json!({
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": read, "WriteCapacityUnits": write},
                "AttributeDefinitions": [
                    {"AttributeName": "pk", "AttributeType": "S"},
                    {"AttributeName": index_key, "AttributeType": "S"},
                ],
                "GlobalSecondaryIndexes": [{
                    "IndexName": index,
                    "KeySchema": [{"AttributeName": index_key, "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {
                        "ReadCapacityUnits": read, "WriteCapacityUnits": write,
                    },
                }],
            }),
        )
        .await;
    }

    /// Create a table from `overrides` merged over the one-attribute default.
    async fn create_table(&self, name: &str, overrides: Value) {
        let mut request = json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        });
        for (member, value) in overrides.as_object().into_iter().flatten() {
            request[member] = value.clone();
        }
        let (status, body) = self.call_dynamodb("CreateTable", request).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateTable to answer 200, got {status} {body:?}"
        );
    }

    /// Register a scalable target on `dimension`, creating the resource it names where this
    /// emulator owns the service that resource lives in, and answer the three identity
    /// members of the target so a caller can address it.
    ///
    /// The resource id is the form that dimension's resource type documents
    /// ([`resource_id_for`]), so one call works for every modelled dimension.
    pub async fn given_target(&self, dimension: &str, min: i64, max: i64) -> Value {
        let namespace = dimension.split(':').next().unwrap_or_default();
        let resource_id = resource_id_for(dimension);
        self.given_resource(namespace, &resource_id, min).await;
        let target = json!({
            "ServiceNamespace": namespace,
            "ResourceId": resource_id,
            "ScalableDimension": dimension,
        });
        let mut registration = target.clone();
        registration["MinCapacity"] = json!(min);
        registration["MaxCapacity"] = json!(max);
        let (status, body) = self.call("RegisterScalableTarget", registration).await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
        );
        target
    }

    /// Create the resource a `(namespace, resource id)` pair names, where this emulator owns
    /// the service that resource lives in, at `capacity` units of the property a scalable
    /// target would scale.
    ///
    /// A scalable target is a capacity dimension OF A RESOURCE, so a registration resolves
    /// that resource in the service that owns it: a `dynamodb` target needs its table (and,
    /// for an index dimension, that index) and an `ecs` target needs its service. Idempotent,
    /// so a suite that registers several targets on one resource calls it once per target;
    /// a namespace whose service this emulator holds no store for needs nothing created.
    pub async fn given_resource(&self, namespace: &str, resource_id: &str, capacity: i64) {
        // A provisioned resource cannot be created with no capacity at all, and a window
        // whose floor is zero is still a window this resource sits inside of.
        let capacity = capacity.max(1);
        match namespace {
            "dynamodb" => {
                let rest = resource_id.trim_start_matches("table/");
                match rest.split_once("/index/") {
                    Some((table, index)) => {
                        if !self.table_exists(table).await {
                            self.create_table_with_index(table, index, "status", capacity, capacity)
                                .await;
                        }
                    }
                    None => {
                        if !self.table_exists(rest).await {
                            self.create_provisioned_table(rest, capacity, capacity).await;
                        }
                    }
                }
            }
            "ecs" => {
                if let Some((cluster, service)) = resource_id
                    .strip_prefix("service/")
                    .and_then(|rest| rest.split_once('/'))
                {
                    if !self.ecs_service_exists(cluster, service).await {
                        self.given_ecs_service(cluster, service, capacity).await;
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether the DynamoDB store already holds a table by that name.
    async fn table_exists(&self, name: &str) -> bool {
        let (status, _) = self
            .call_dynamodb("DescribeTable", json!({"TableName": name}))
            .await;
        status == 200
    }

    /// Whether the ECS store already holds that service in that cluster.
    pub async fn ecs_service_exists(&self, cluster: &str, service: &str) -> bool {
        let (status, body) = self
            .call_ecs(
                "DescribeServices",
                json!({"cluster": cluster, "services": [service]}),
            )
            .await;
        status == 200 && body["services"].as_array().is_some_and(|found| !found.is_empty())
    }

    /// The `ScalableTargetARN` this service minted for `target`, read back over the wire the
    /// way a client that kept the registration's answer holds it.
    ///
    /// `target` is the three identity members [`Emulator::given_target`] answers, and the ARN
    /// is the one reported for exactly that identity — not for whichever target the namespace
    /// happens to list first.
    pub async fn target_arn(&self, target: &Value) -> String {
        let (status, body) = self
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": target["ServiceNamespace"]}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {target:?}: expected DescribeScalableTargets to answer 200, got {status} \
             {body:?}"
        );
        let reported = body["ScalableTargets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|reported| {
                reported["ResourceId"] == target["ResourceId"]
                    && reported["ScalableDimension"] == target["ScalableDimension"]
            });
        let reported = reported
            .unwrap_or_else(|| panic!("case {target:?}: expected the target to be reported"));
        reported["ScalableTargetARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {target:?}: expected a ScalableTargetARN"))
            .to_string()
    }

    /// Every alarm CloudWatch holds under the `TargetTracking` prefix, by alarm name.
    pub async fn target_tracking_alarms(&self) -> Vec<Value> {
        let (status, body) = self
            .call_cloudwatch(
                "DescribeAlarms",
                json!({"AlarmNamePrefix": ALARM_NAME_PREFIX, "MaxRecords": 100}),
            )
            .await;
        assert_eq!(
            status, 200,
            "expected DescribeAlarms to answer 200, got {status} {body:?}"
        );
        let mut alarms = body["MetricAlarms"].as_array().cloned().unwrap_or_default();
        alarms.sort_by_key(|alarm| alarm["AlarmName"].as_str().unwrap_or_default().to_string());
        alarms
    }
}

/// The prefix Application Auto Scaling gives every alarm it mints, which is what lets a
/// client find them with `DescribeAlarms`.
pub const ALARM_NAME_PREFIX: &str = "TargetTracking";

/// One CloudWatch alarm a step scaling policy is wired to
/// ([`Emulator::given_policy_alarm`]).
pub struct PolicyAlarm<'a> {
    /// The alarm's `AlarmName`.
    pub name: &'a str,
    /// The `PolicyARN` the alarm executes as its action.
    pub policy_arn: &'a str,
    /// The metric namespace the alarm watches.
    pub metric_namespace: &'a str,
    /// The metric it watches inside that namespace.
    pub metric_name: &'a str,
    /// The `Threshold` a step adjustment's interval is measured from.
    pub threshold: f64,
    /// The comparison that puts the alarm in `ALARM`, which is what names the DIRECTION a
    /// breach of it is in.
    pub comparison_operator: &'a str,
}

/// The resource-type suffix a namespace's service-linked role name carries.
///
/// Source: "Service-linked roles for Application Auto Scaling" in the Application Auto
/// Scaling User Guide, which publishes one role per supported service, each named
/// `AWSServiceRoleForApplicationAutoScaling_<suffix>`. Spelled out here rather than read
/// from the emulator's own table, so an assertion is against the PUBLISHED name.
pub fn service_linked_role_suffix(namespace: &str) -> &'static str {
    match namespace {
        "appstream" => "AppStreamFleet",
        "cassandra" => "CassandraTable",
        "comprehend" => "ComprehendEndpoint",
        "custom-resource" => "CustomResource",
        "dynamodb" => "DynamoDBTable",
        "ec2" => "EC2SpotFleetRequest",
        "ecs" => "ECSService",
        "elasticache" => "ElastiCacheRG",
        "elasticmapreduce" => "EMRInstanceGroup",
        "kafka" => "KafkaCluster",
        "lambda" => "LambdaConcurrency",
        "neptune" => "NeptuneCluster",
        "rds" => "RDSCluster",
        "sagemaker" => "SageMakerEndpoint",
        "workspaces" => "WorkSpacesPool",
        other => panic!("case {other:?}: no service-linked role is published for it"),
    }
}

/// A `ResourceId` of the form the resource type in `dimension` documents.
///
/// One row per resource type the model's own `ResourceId` documentation gives an example
/// for (`smithy_query(query="doc",shape="com.amazonaws.applicationautoscaling#RegisterScalableTargetRequest",member="ResourceId")`),
/// so a suite that walks the `ScalableDimension` enum can name a resource for every value
/// of it instead of carrying its own table.
pub fn resource_id_for(dimension: &str) -> String {
    match dimension {
        "appstream:fleet:DesiredCapacity" => "fleet/sample-fleet".to_string(),
        "cassandra:table:ReadCapacityUnits" | "cassandra:table:WriteCapacityUnits" => {
            "keyspace/events/table/clicks".to_string()
        }
        "comprehend:document-classifier-endpoint:DesiredInferenceUnits" => format!(
            "arn:aws:comprehend:{REGION}:{ACCOUNT}:document-classifier-endpoint/classifier"
        ),
        "comprehend:entity-recognizer-endpoint:DesiredInferenceUnits" => format!(
            "arn:aws:comprehend:{REGION}:{ACCOUNT}:entity-recognizer-endpoint/recognizer"
        ),
        "custom-resource:ResourceType:Property" => "custom-resource/my-resource".to_string(),
        "dynamodb:table:ReadCapacityUnits" | "dynamodb:table:WriteCapacityUnits" => {
            "table/catalog".to_string()
        }
        "dynamodb:index:ReadCapacityUnits" | "dynamodb:index:WriteCapacityUnits" => {
            "table/catalog-indexed/index/by-status".to_string()
        }
        "ec2:spot-fleet-request:TargetCapacity" => {
            "spot-fleet-request/sfr-73fbd2ce-aa30-494c-8788-1cee4EXAMPLE".to_string()
        }
        "ecs:service:DesiredCount" => "service/prod-cluster/web-app".to_string(),
        "elasticache:cache-cluster:Nodes" => "cache-cluster/sessions".to_string(),
        "elasticache:replication-group:NodeGroups"
        | "elasticache:replication-group:Replicas" => "replication-group/sessions".to_string(),
        "elasticmapreduce:instancegroup:InstanceCount" => {
            "instancegroup/j-2EEZNYKUA1NTV/ig-1791Y4E1L8YI0".to_string()
        }
        "kafka:broker-storage:VolumeSize" => format!(
            "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/events/abcd1234-1234-abcd-5678-cdef0123ab01-2"
        ),
        "lambda:function:ProvisionedConcurrency" => "function:ingest:green".to_string(),
        "neptune:cluster:ReadReplicaCount" => "cluster:graphdb".to_string(),
        "rds:cluster:ReadReplicaCount" => "cluster:orders-db".to_string(),
        "sagemaker:inference-component:DesiredCopyCount" => {
            "inference-component/ranker".to_string()
        }
        "sagemaker:variant:DesiredInstanceCount"
        | "sagemaker:variant:DesiredProvisionedConcurrency" => {
            "endpoint/rank/variant/blue".to_string()
        }
        "workspaces:workspacespool:DesiredUserSessions" => {
            "workspacespool/wspool-9f8e7d".to_string()
        }
        other => panic!("case {other:?}: this harness names no resource for it"),
    }
}

/// Which of a policy's alarms an alarm name is, read out of the grammar the service mints
/// one by: `TargetTracking-<resource id>-<kind>-<id>`, whose trailing id is an 8-4-4-4-12
/// identifier — so the kind is the segment before the LAST five dash-separated groups.
pub fn alarm_kind(alarm: &Value) -> String {
    let name = alarm["AlarmName"].as_str().unwrap_or_default();
    let mut segments = name.rsplitn(7, '-');
    // The five groups of the trailing identifier.
    for _ in 0..5 {
        segments.next();
    }
    segments.next().unwrap_or_default().to_string()
}

/// The kinds of the alarms in `alarms`, sorted, as `DescribeAlarms` reported them.
pub fn alarm_kinds(alarms: &[Value]) -> Vec<String> {
    let mut kinds: Vec<String> = alarms.iter().map(alarm_kind).collect();
    kinds.sort();
    kinds
}

/// An `at(yyyy-mm-ddThh:mm:ss)` schedule for the instant `due_in` from now, and that
/// instant.
///
/// A suite that watches an occurrence come due needs the expression and the instant it
/// names together: nothing sleeps in the service, so the occurrence is materialised by the
/// first request after that instant passes, and the caller waits for it itself. Shared here
/// rather than copied per suite so both read the same wall clock the same way.
pub fn at_schedule_in(due_in: std::time::Duration) -> (String, std::time::SystemTime) {
    let at = std::time::SystemTime::now() + due_in;
    let seconds = at
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("an instant after the epoch")
        .as_secs();
    let schedule = format!(
        "at({})",
        chrono::DateTime::from_timestamp(
            i64::try_from(seconds).expect("a representable instant"),
            0
        )
        .expect("a representable instant")
        .format("%Y-%m-%dT%H:%M:%S")
    );
    (schedule, at)
}

/// Percent-encode one `application/x-www-form-urlencoded` component, leaving only RFC 3986's
/// unreserved characters literal, which is how a client encodes an awsQuery parameter.
fn form_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            other => encoded.push_str(&format!("%{other:02X}")),
        }
    }
    encoded
}

/// The `__type` discriminator an awsJson error body carries, without its namespace.
pub fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .map(|name| name.rsplit('#').next().unwrap_or(name).to_string())
        .unwrap_or_default()
}

/// The `Message` an error body carries, whichever casing the protocol rendered it in.
pub fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// How many hexadecimal digits a scalable target's own identifier carries.
const TARGET_ID_HEX_DIGITS: usize = 32;

/// The identifier of the scalable target `scalable_target_arn` names, as a refusal about one
/// of its sub-resources states it: the hexadecimal digits the ARN ends with, grouped
/// `8-4-4-4-12`.
///
/// Derived HERE from the ARN a registration handed back, so a suite asserts the refusal names
/// the target the caller holds an ARN for rather than a string this file spells out.
pub fn scalable_target_id(scalable_target_arn: &str) -> String {
    let digits: Vec<char> = scalable_target_arn.chars().collect();
    assert!(
        digits.len() >= TARGET_ID_HEX_DIGITS,
        "case {scalable_target_arn:?}: expected an ARN ending in {TARGET_ID_HEX_DIGITS} \
         hexadecimal digits"
    );
    let digits = &digits[digits.len() - TARGET_ID_HEX_DIGITS..];
    let group = |range: std::ops::Range<usize>| digits[range].iter().collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        group(0..8),
        group(8..12),
        group(12..16),
        group(16..20),
        group(20..TARGET_ID_HEX_DIGITS),
    )
}
