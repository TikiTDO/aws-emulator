// SPDX-License-Identifier: CC0-1.0

//! A bucket's event notifications as a stack declares them, read back through Amazon S3.
//!
//! RULE: a resource CloudFormation provisions carries, in the service that OWNS it, every
//! sub-resource configuration its declaration states — each applied by the one API call that
//! service publishes for it, since none of them is a member of the create — so a read of
//! that sub-resource straight after the stack reports the resource created answers with what
//! the template declared. For an `AWS::S3::Bucket`'s `NotificationConfiguration` that is a
//! `PutBucketNotificationConfiguration` of the declared destination configurations, so
//! `s3:GetBucketNotificationConfiguration` on the bucket the stack published reports one
//! configuration per declared `LambdaConfigurations`/`QueueConfigurations`/
//! `TopicConfigurations` entry, each carrying the destination the declaration resolved to,
//! the one `Event` it states as the `Events` list, and the `Filter.S3Key.Rules` it states as
//! the key `FilterRules`.
//!
//! The rows differ along what the rule generalises over: which DESTINATION kind the
//! configurations name (a Lambda function, an Amazon SQS queue, an Amazon SNS topic, and all
//! three in one bucket), HOW MANY one bucket declares (none, one, three), the EVENT each is
//! notified about (object created, object removed, several on one destination), the FILTER
//! each states (a suffix rule, a prefix rule, both in one configuration, none at all), where
//! the destination ARN COMES FROM (a literal the template spells, an `Fn::GetAtt` on a
//! function the same template declares, a `Ref` on a topic it declares), whether
//! EventBridge delivery is enabled, the template SPELLING (JSON and YAML, long- and
//! short-form intrinsics), and how many buckets one stack declares.
//!
//! Every expected value is computed from what the row declares — the ARN forms AWS publishes
//! for a function and a topic — never read back out of the notification answer under test.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, create_stack, describe_stack, router, wait_for_the_transition, xml_blocks,
    xml_text, xml_texts, ACCOUNT_ID, REGION,
};
use crate::stack_resource_provisioning::{output_value, s3};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// Where a declared configuration's destination ARN comes from, and so what it must read
/// back as.
///
/// The ARN forms are the ones AWS publishes for each service:
/// `arn:aws:lambda:<region>:<account>:function:<name>` for a function (the `Arn` attribute of
/// `AWS::Lambda::Function`) and `arn:aws:sns:<region>:<account>:<name>` for a topic (which is
/// what `AWS::SNS::Topic`'s `Ref` returns).
enum Destination<'a> {
    /// An ARN the template spells out as a literal.
    Literal(&'a str),
    /// A Lambda function the same template declares under this `FunctionName`, reached
    /// through `Fn::GetAtt`.
    Function(&'a str),
    /// An Amazon SNS topic the same template declares under this `TopicName`, reached
    /// through `Ref`.
    Topic(&'a str),
}

impl Destination<'_> {
    /// The ARN the declaration resolves to.
    fn arn(&self) -> String {
        match self {
            Self::Literal(arn) => (*arn).to_string(),
            Self::Function(name) => {
                format!("arn:aws:lambda:{REGION}:{ACCOUNT_ID}:function:{name}")
            }
            Self::Topic(name) => format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}"),
        }
    }
}

/// One destination configuration a row expects the bucket to carry.
struct Expected<'a> {
    destination: Destination<'a>,
    /// The events the configuration must be notified about, in the order it states them.
    events: &'a [&'a str],
    /// The `(Name, Value)` key-name filter rules it must carry, in the declared order; an
    /// empty slice is a configuration that must carry no `Filter` at all.
    rules: &'a [(&'a str, &'a str)],
}

/// One bucket of a row: the stack output that publishes its name, and the configurations
/// Amazon S3 must report for it.
struct Bucket<'a> {
    output_key: &'a str,
    lambda: &'a [Expected<'a>],
    queue: &'a [Expected<'a>],
    topic: &'a [Expected<'a>],
    /// Whether the bucket must report an `EventBridgeConfiguration`, whose PRESENCE is what
    /// says delivery to Amazon EventBridge is enabled.
    event_bridge: bool,
}

/// One row of the table.
struct NotificationCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    buckets: &'a [Bucket<'a>],
}

/// One destination configuration as Amazon S3 reported it.
#[derive(Debug, PartialEq, Eq)]
struct Installed {
    arn: String,
    events: Vec<String>,
    rules: Vec<(String, String)>,
}

/// Every configuration of one destination kind the answer carries, in document order.
///
/// `block` is the element the flattened list is rendered as and `destination` the element the
/// destination ARN arrives in — `CloudFunctionConfiguration`/`CloudFunction`,
/// `QueueConfiguration`/`Queue` and `TopicConfiguration`/`Topic`, which are the `xmlName`s
/// the model gives those members
/// (`smithy_query(query="shape",shape="NotificationConfiguration")` and its three
/// configuration shapes).
fn installed(xml: &str, block: &str, destination: &str) -> Vec<Installed> {
    xml_blocks(xml, block)
        .iter()
        .map(|entry| Installed {
            arn: xml_text(entry, destination).unwrap_or_default(),
            events: xml_texts(entry, "Event"),
            rules: xml_blocks(entry, "FilterRule")
                .iter()
                .map(|rule| {
                    (
                        xml_text(rule, "Name").unwrap_or_default(),
                        xml_text(rule, "Value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The same reading of what a row EXPECTS, so the two sides are compared as one value.
///
/// A row states its filter rules the way its TEMPLATE spells them — `FilterRuleName`'s own
/// values, `prefix` and `suffix` — and Amazon S3 keeps each rule under the canonical
/// capitalised name it reports to every reader, which is the one mapping applied here
/// (`service_com_amazonaws_s3_amazons3::state::canonical_filter_rule_name`).
fn expected(configurations: &[Expected<'_>]) -> Vec<Installed> {
    configurations
        .iter()
        .map(|configuration| Installed {
            arn: configuration.destination.arn(),
            events: configuration
                .events
                .iter()
                .map(|event| (*event).to_string())
                .collect(),
            rules: configuration
                .rules
                .iter()
                .map(|(name, value)| {
                    let reported =
                        service_com_amazonaws_s3_amazons3::state::canonical_filter_rule_name(name)
                            .map_or_else(|| (*name).to_string(), str::to_string);
                    (reported, (*value).to_string())
                })
                .collect(),
        })
        .collect()
}

/// `GET /<bucket>?notification` as a client makes it.
async fn bucket_notifications(router: &MultiServiceRouter, bucket: &str) -> (u16, String) {
    let answer = s3(router, "GET", &format!("/{bucket}?notification"), b"").await;
    (answer.status, answer.body)
}

/// The `Id` Amazon S3 assigned to every configuration of one destination kind.
///
/// CloudFormation sends none — an `Id` is not a member of this resource type's notification
/// configurations at all — so "if you don't provide one, Amazon S3 will assign an ID"
/// (`NotificationId`) is what every reported id comes from.
fn assigned_ids(xml: &str, block: &str) -> Vec<String> {
    xml_blocks(xml, block)
        .iter()
        .map(|entry| xml_text(entry, "Id").unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn a_bucket_a_stack_declares_carries_the_notification_configuration_it_declared() {
    let cases: &[NotificationCase<'_>] = &[
        NotificationCase {
            // The issue's own shape: three suffix-filtered configurations on one function,
            // on a bucket that DependsOn the permission granting S3 the invoke.
            case: "three suffix-filtered Lambda configurations, in JSON",
            stack_name: "rekognition-trigger",
            template: r#"{"Resources":{
                 "HandlerRole":{"Type":"AWS::IAM::Role","Properties":{
                    "AssumeRolePolicyDocument":{"Version":"2012-10-17","Statement":[{
                      "Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},
                      "Action":"sts:AssumeRole"}]}}},
                 "Handler":{"Type":"AWS::Lambda::Function","Properties":{
                    "FunctionName":"image-labeller","Runtime":"python3.12",
                    "Handler":"index.handler",
                    "Role":{"Fn::GetAtt":["HandlerRole","Arn"]},
                    "Code":{"ZipFile":"def handler(event, context):\n    return {}\n"}}},
                 "InvokePermission":{"Type":"AWS::Lambda::Permission","Properties":{
                    "Action":"lambda:InvokeFunction","Principal":"s3.amazonaws.com",
                    "FunctionName":{"Fn::GetAtt":["Handler","Arn"]}}},
                 "ImageBucket":{"Type":"AWS::S3::Bucket","DependsOn":["InvokePermission"],
                   "Properties":{"BucketName":"rekognition-trigger-images",
                    "NotificationConfiguration":{"LambdaConfigurations":[
                      {"Event":"s3:ObjectCreated:*","Function":{"Fn::GetAtt":["Handler","Arn"]},
                       "Filter":{"S3Key":{"Rules":[{"Name":"suffix","Value":".jpg"}]}}},
                      {"Event":"s3:ObjectCreated:*","Function":{"Fn::GetAtt":["Handler","Arn"]},
                       "Filter":{"S3Key":{"Rules":[{"Name":"suffix","Value":".jpeg"}]}}},
                      {"Event":"s3:ObjectCreated:*","Function":{"Fn::GetAtt":["Handler","Arn"]},
                       "Filter":{"S3Key":{"Rules":[{"Name":"suffix","Value":".png"}]}}}]}}}},
                 "Outputs":{"BucketName":{"Value":{"Ref":"ImageBucket"}}}}"#,
            buckets: &[Bucket {
                output_key: "BucketName",
                lambda: &[
                    Expected {
                        destination: Destination::Function("image-labeller"),
                        events: &["s3:ObjectCreated:*"],
                        rules: &[("suffix", ".jpg")],
                    },
                    Expected {
                        destination: Destination::Function("image-labeller"),
                        events: &["s3:ObjectCreated:*"],
                        rules: &[("suffix", ".jpeg")],
                    },
                    Expected {
                        destination: Destination::Function("image-labeller"),
                        events: &["s3:ObjectCreated:*"],
                        rules: &[("suffix", ".png")],
                    },
                ],
                queue: &[],
                topic: &[],
                event_bridge: false,
            }],
        },
        NotificationCase {
            // One configuration, a removal event, and a filter stating BOTH published rule
            // names — in YAML, with the long form of the intrinsic.
            case: "a prefix and a suffix rule on one removal configuration, in YAML",
            stack_name: "archive-sweeper",
            template: concat!(
                "Resources:\n",
                "  SweeperRole:\n",
                "    Type: AWS::IAM::Role\n",
                "    Properties:\n",
                "      AssumeRolePolicyDocument:\n",
                "        Version: '2012-10-17'\n",
                "        Statement:\n",
                "          - Effect: Allow\n",
                "            Principal:\n",
                "              Service: lambda.amazonaws.com\n",
                "            Action: sts:AssumeRole\n",
                "  Sweeper:\n",
                "    Type: AWS::Lambda::Function\n",
                "    Properties:\n",
                "      FunctionName: archive-sweeper-fn\n",
                "      Runtime: python3.12\n",
                "      Handler: index.handler\n",
                "      Role: !GetAtt SweeperRole.Arn\n",
                "      Code:\n",
                "        ZipFile: |\n",
                "          def handler(event, context):\n",
                "              return {}\n",
                "  SweeperPermission:\n",
                "    Type: AWS::Lambda::Permission\n",
                "    Properties:\n",
                "      Action: lambda:InvokeFunction\n",
                "      Principal: s3.amazonaws.com\n",
                "      FunctionName: !GetAtt Sweeper.Arn\n",
                "  ArchiveBucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    DependsOn: SweeperPermission\n",
                "    Properties:\n",
                "      BucketName: archive-sweeper-objects\n",
                "      NotificationConfiguration:\n",
                "        LambdaConfigurations:\n",
                "          - Event: s3:ObjectRemoved:Delete\n",
                "            Function:\n",
                "              Fn::GetAtt: [Sweeper, Arn]\n",
                "            Filter:\n",
                "              S3Key:\n",
                "                Rules:\n",
                "                  - Name: prefix\n",
                "                    Value: archive/\n",
                "                  - Name: suffix\n",
                "                    Value: .tar.gz\n",
                "Outputs:\n",
                "  BucketName:\n",
                "    Value: !Ref ArchiveBucket\n",
            ),
            buckets: &[Bucket {
                output_key: "BucketName",
                lambda: &[Expected {
                    destination: Destination::Function("archive-sweeper-fn"),
                    events: &["s3:ObjectRemoved:Delete"],
                    rules: &[("prefix", "archive/"), ("suffix", ".tar.gz")],
                }],
                queue: &[],
                topic: &[],
                event_bridge: false,
            }],
        },
        NotificationCase {
            // All three destination kinds on one bucket: a literal function ARN, a literal
            // queue ARN, and the topic ARN a `Ref` on a declared topic returns. The queue
            // states no `Filter` at all, so it must read back without one. The topic the
            // same template declares carries the policy that lets Amazon S3 publish to it,
            // which S3 validates before it records the configuration — and the bucket
            // `DependsOn` that policy, so it is in force by the time the bucket is created.
            case: "a function, a queue and a topic configuration on one bucket",
            stack_name: "fan-out-ingest",
            template: concat!(
                "Resources:\n",
                "  Notices:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: fan-out-ingest-notices\n",
                "  NoticesPolicy:\n",
                "    Type: AWS::SNS::TopicPolicy\n",
                "    Properties:\n",
                "      Topics:\n",
                "        - !Ref Notices\n",
                "      PolicyDocument:\n",
                "        Version: '2012-10-17'\n",
                "        Statement:\n",
                "          - Sid: AllowS3Publish\n",
                "            Effect: Allow\n",
                "            Principal:\n",
                "              Service: s3.amazonaws.com\n",
                "            Action: sns:Publish\n",
                "            Resource: !Ref Notices\n",
                "  IngestRole:\n",
                "    Type: AWS::IAM::Role\n",
                "    Properties:\n",
                "      AssumeRolePolicyDocument:\n",
                "        Version: '2012-10-17'\n",
                "        Statement:\n",
                "          - Effect: Allow\n",
                "            Principal:\n",
                "              Service: lambda.amazonaws.com\n",
                "            Action: sts:AssumeRole\n",
                "  IngestFn:\n",
                "    Type: AWS::Lambda::Function\n",
                "    Properties:\n",
                "      FunctionName: fan-out-ingest-fn\n",
                "      Runtime: python3.12\n",
                "      Handler: index.handler\n",
                "      Role: !GetAtt IngestRole.Arn\n",
                "      Code:\n",
                "        ZipFile: |\n",
                "          def handler(event, context):\n",
                "              return {}\n",
                "  IngestPermission:\n",
                "    Type: AWS::Lambda::Permission\n",
                "    Properties:\n",
                "      Action: lambda:InvokeFunction\n",
                "      Principal: s3.amazonaws.com\n",
                "      FunctionName: !GetAtt IngestFn.Arn\n",
                "  IngestBucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    DependsOn:\n",
                "      - NoticesPolicy\n",
                "      - IngestPermission\n",
                "    Properties:\n",
                "      BucketName: fan-out-ingest-landing\n",
                "      NotificationConfiguration:\n",
                "        LambdaConfigurations:\n",
                "          - Event: s3:ObjectCreated:Put\n",
                "            Function: \
                 arn:aws:lambda:us-east-1:000000000000:function:fan-out-ingest-fn\n",
                "            Filter:\n",
                "              S3Key:\n",
                "                Rules:\n",
                "                  - Name: prefix\n",
                "                    Value: incoming/\n",
                "        QueueConfigurations:\n",
                "          - Event: s3:ObjectCreated:*\n",
                "            Queue: arn:aws:sqs:us-east-1:000000000000:fan-out-ingest-queue\n",
                // Filtered on a prefix of its own: the type wildcard covers the Lambda
                // configuration's own `s3:ObjectCreated:Put`, and two configurations of one
                // bucket cannot share an event type unless their key filters tell them
                // apart.
                "            Filter:\n",
                "              S3Key:\n",
                "                Rules:\n",
                "                  - Name: prefix\n",
                "                    Value: queued/\n",
                "        TopicConfigurations:\n",
                "          - Event: s3:ObjectRemoved:*\n",
                "            Topic: !Ref Notices\n",
                "            Filter:\n",
                "              S3Key:\n",
                "                Rules:\n",
                "                  - Name: suffix\n",
                "                    Value: .csv\n",
                "Outputs:\n",
                "  BucketName:\n",
                "    Value: !Ref IngestBucket\n",
            ),
            buckets: &[Bucket {
                output_key: "BucketName",
                lambda: &[Expected {
                    destination: Destination::Literal(
                        "arn:aws:lambda:us-east-1:000000000000:function:fan-out-ingest-fn",
                    ),
                    events: &["s3:ObjectCreated:Put"],
                    rules: &[("prefix", "incoming/")],
                }],
                queue: &[Expected {
                    destination: Destination::Literal(
                        "arn:aws:sqs:us-east-1:000000000000:fan-out-ingest-queue",
                    ),
                    events: &["s3:ObjectCreated:*"],
                    rules: &[("prefix", "queued/")],
                }],
                topic: &[Expected {
                    destination: Destination::Topic("fan-out-ingest-notices"),
                    events: &["s3:ObjectRemoved:*"],
                    rules: &[("suffix", ".csv")],
                }],
                event_bridge: false,
            }],
        },
        NotificationCase {
            // EventBridge delivery enabled beside a destination configuration, and a second
            // bucket in the same stack declaring NO notifications at all: each bucket
            // carries its own configuration.
            case: "EventBridge delivery enabled, beside a bucket that declares none",
            stack_name: "event-bridge-ingest",
            template: concat!(
                "Resources:\n",
                "  WatchedRole:\n",
                "    Type: AWS::IAM::Role\n",
                "    Properties:\n",
                "      AssumeRolePolicyDocument:\n",
                "        Version: '2012-10-17'\n",
                "        Statement:\n",
                "          - Effect: Allow\n",
                "            Principal:\n",
                "              Service: lambda.amazonaws.com\n",
                "            Action: sts:AssumeRole\n",
                "  WatchedFn:\n",
                "    Type: AWS::Lambda::Function\n",
                "    Properties:\n",
                "      FunctionName: event-bridge-ingest-fn\n",
                "      Runtime: python3.12\n",
                "      Handler: index.handler\n",
                "      Role: !GetAtt WatchedRole.Arn\n",
                "      Code:\n",
                "        ZipFile: |\n",
                "          def handler(event, context):\n",
                "              return {}\n",
                "  WatchedPermission:\n",
                "    Type: AWS::Lambda::Permission\n",
                "    Properties:\n",
                "      Action: lambda:InvokeFunction\n",
                "      Principal: s3.amazonaws.com\n",
                "      FunctionName: !GetAtt WatchedFn.Arn\n",
                "  WatchedBucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    DependsOn: WatchedPermission\n",
                "    Properties:\n",
                "      BucketName: event-bridge-ingest-watched\n",
                "      NotificationConfiguration:\n",
                "        EventBridgeConfiguration:\n",
                "          EventBridgeEnabled: true\n",
                "        LambdaConfigurations:\n",
                "          - Event: s3:ObjectTagging:Put\n",
                "            Function: \
                 arn:aws:lambda:us-east-1:000000000000:function:event-bridge-ingest-fn\n",
                "  QuietBucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: event-bridge-ingest-quiet\n",
                "Outputs:\n",
                "  WatchedBucketName:\n",
                "    Value: !Ref WatchedBucket\n",
                "  QuietBucketName:\n",
                "    Value: !Ref QuietBucket\n",
            ),
            buckets: &[
                Bucket {
                    output_key: "WatchedBucketName",
                    lambda: &[Expected {
                        destination: Destination::Literal(
                            "arn:aws:lambda:us-east-1:000000000000:function:event-bridge-ingest-fn",
                        ),
                        events: &["s3:ObjectTagging:Put"],
                        rules: &[],
                    }],
                    queue: &[],
                    topic: &[],
                    event_bridge: true,
                },
                Bucket {
                    output_key: "QuietBucketName",
                    lambda: &[],
                    queue: &[],
                    topic: &[],
                    event_bridge: false,
                },
            ],
        },
        NotificationCase {
            // A declaration that states only `EventBridgeEnabled: false` enables nothing:
            // the bucket reports the empty configuration a bucket is created with.
            case: "EventBridge delivery declared and switched off",
            stack_name: "quiet-event-bridge",
            template: concat!(
                "Resources:\n",
                "  Landing:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: quiet-event-bridge-landing\n",
                "      NotificationConfiguration:\n",
                "        EventBridgeConfiguration:\n",
                "          EventBridgeEnabled: false\n",
                "Outputs:\n",
                "  BucketName:\n",
                "    Value: !Ref Landing\n",
            ),
            buckets: &[Bucket {
                output_key: "BucketName",
                lambda: &[],
                queue: &[],
                topic: &[],
                event_bridge: false,
            }],
        },
    ];

    for case in cases {
        let router = router();
        let (status, created) =
            create_stack(&router, case.stack_name, case.template, &acknowledging_iam(&[])).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateStack must answer 200, got {status} {created}",
            case.case
        );
        wait_for_the_transition().await;

        let (status, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeStacks must answer 200, got {status} {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
            case.case
        );

        for bucket in case.buckets {
            let name = output_value(&described, bucket.output_key);
            assert!(
                !name.is_empty(),
                "case {:?}: the stack must publish {:?}, got {described}",
                case.case,
                bucket.output_key
            );
            let (status, answer) = bucket_notifications(&router, &name).await;
            assert_eq!(
                status, 200,
                "case {:?}: s3:GetBucketNotificationConfiguration({name:?}) must answer 200, got \
                 {status} {answer}",
                case.case
            );

            for (block, destination, wanted) in [
                ("CloudFunctionConfiguration", "CloudFunction", bucket.lambda),
                ("QueueConfiguration", "Queue", bucket.queue),
                ("TopicConfiguration", "Topic", bucket.topic),
            ] {
                let reported = installed(&answer, block, destination);
                let wanted = expected(wanted);
                assert_eq!(
                    reported, wanted,
                    "case {:?}: the bucket {name:?} must report {block} {wanted:?}, got \
                     {reported:?} ({answer})",
                    case.case
                );
                for id in assigned_ids(&answer, block) {
                    assert!(
                        !id.is_empty(),
                        "case {:?}: every {block} of {name:?} must carry the Id Amazon S3 \
                         assigned, got {answer}",
                        case.case
                    );
                }
            }

            let reported_event_bridge = answer.contains("<EventBridgeConfiguration");
            assert_eq!(
                reported_event_bridge, bucket.event_bridge,
                "case {:?}: the bucket {name:?} must report EventBridge delivery enabled={}, got \
                 {answer}",
                case.case, bucket.event_bridge
            );
        }
    }
}

/// A declared configuration naming no destination at all is not installed quietly: the
/// resource fails, and the stack says so rather than reporting CREATE_COMPLETE over a
/// notification that went nowhere.
#[tokio::test]
async fn a_configuration_with_no_destination_fails_the_bucket() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Landing:\n",
        "    Type: AWS::S3::Bucket\n",
        "    Properties:\n",
        "      BucketName: destinationless-landing\n",
        "      NotificationConfiguration:\n",
        "        LambdaConfigurations:\n",
        "          - Event: s3:ObjectCreated:*\n",
    );
    let (status, created) = create_stack(&router, "destinationless", template, &[]).await;
    assert_eq!(
        status, 200,
        "CreateStack must answer 200, got {status} {created}"
    );
    wait_for_the_transition().await;

    let (status, described) = describe_stack(&router, "destinationless").await;
    assert_eq!(
        status, 200,
        "DescribeStacks must answer 200, got {status} {described}"
    );
    let reported = xml_text(&described, "StackStatus").unwrap_or_default();
    assert_ne!(
        reported, status::CREATE_COMPLETE,
        "a bucket whose declared notification could not be installed must not complete, got \
         {described}"
    );
}
