// SPDX-License-Identifier: CC0-1.0

//! A parent stack reading its NESTED stack's outputs, end to end over the awsQuery wire.
//!
//! RULE: an `AWS::CloudFormation::Stack` resource publishes one `Fn::GetAtt` attribute per
//! output the stack it created published — `Outputs.<NestedStackOutputName>` — and its value is
//! the value that child stack published under that key. So a parent output whose `Value` reads
//! one is reported by `DescribeStacks` with its `OutputValue` beside its `OutputKey`, and that
//! value names the resource the child's own template really created.
//!
//! The rows differ along what the rule generalises over: which SPELLING of `Fn::GetAtt` reads
//! the attribute (the dotted short form, the two-part array, the three-part array, an `Fn::Sub`
//! variable, a composition inside `Fn::Join`), how many outputs of one child are read and how
//! many children one parent reads, what KIND of value the child publishes (a resource's
//! physical id, one of its attributes, a literal), and whether the output key named is one the
//! child publishes at all — the one row with no value to report, since an output the child does
//! not publish has none and an invented one would be worse than none.

use serde_json::{json, Map, Value};

use crate::change_set_lifecycle::settle_the_store;
use crate::harness::{create_stack, router_with_engine};
use crate::nested_stacks::{staged_url, STACK_RESOURCE_TYPE};
use crate::stack_lifecycle::{describe_stack, xml_blocks, xml_text};
use crate::stack_resource_provisioning::s3;
use crate::template_by_reference::{create_staging_bucket, stage};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The literal one child output publishes, so a row can read a value that is neither an id nor
/// an attribute of any resource.
const PUBLISHED_LITERAL: &str = "published-by-the-child";

/// An Amazon S3 bucket's ARN, which carries neither a region nor an account id — the form that
/// type's "Return values" section publishes for `Fn::GetAtt Arn`.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// A child template declaring ONE bucket and publishing three KINDS of output over it: the
/// resource's physical id, one of its published attributes, and a literal.
fn child_template(bucket: &str) -> String {
    json!({
        "Resources": {
            "Data": {"Type": "AWS::S3::Bucket", "Properties": {"BucketName": bucket}}
        },
        "Outputs": {
            "BucketName": {"Value": {"Ref": "Data"}},
            "BucketArn": {"Value": {"Fn::GetAtt": ["Data", "Arn"]}},
            "Label": {"Value": PUBLISHED_LITERAL}
        }
    })
    .to_string()
}

/// What one of the parent's outputs must report.
#[derive(Debug)]
enum Expected {
    /// Exactly this text.
    Exact(String),
    /// The name of the bucket the child created: the value must be that name AND Amazon S3 must
    /// hold the bucket, which is what says the child's template really deployed rather than the
    /// parent having made a name up.
    Bucket(String),
    /// No `OutputValue` at all.
    NoValue,
}

/// One child stack of a row: the logical id that declares it, the key its template is staged
/// under, and the bucket that template creates.
struct Child {
    logical_id: &'static str,
    key: &'static str,
    bucket: String,
}

impl Child {
    fn new(logical_id: &'static str, key: &'static str, bucket: &str) -> Self {
        Self {
            logical_id,
            key,
            bucket: bucket.to_string(),
        }
    }
}

/// One row: a parent stack, the nested stacks it declares, the outputs it reads off them and
/// what each of those outputs must report.
struct Row {
    case: &'static str,
    stack: &'static str,
    children: Vec<Child>,
    /// The parent's own `Outputs`: the key, and the `Value` expression that reads the child.
    outputs: Vec<(&'static str, Value)>,
    expected: Vec<(&'static str, Expected)>,
}

fn rows() -> Vec<Row> {
    let short = "getatt-short-data";
    let pair = "getatt-pair-data";
    let triple = "getatt-triple-data";
    let composed = "getatt-composed-data";
    let kinds = "getatt-kinds-data";
    let first = "getatt-two-first-data";
    let second = "getatt-two-second-data";
    let partial = "getatt-partial-data";
    vec![
        Row {
            case: "the dotted short form",
            stack: "getatt-short",
            children: vec![Child::new("Child", "getatt-short-child.json", short)],
            outputs: vec![(
                "NestedBucketName",
                json!({"Fn::GetAtt": "Child.Outputs.BucketName"}),
            )],
            expected: vec![("NestedBucketName", Expected::Bucket(short.to_string()))],
        },
        Row {
            case: "the two-part array form, the attribute path as one element",
            stack: "getatt-pair",
            children: vec![Child::new("Child", "getatt-pair-child.json", pair)],
            outputs: vec![(
                "NestedBucketName",
                json!({"Fn::GetAtt": ["Child", "Outputs.BucketName"]}),
            )],
            expected: vec![("NestedBucketName", Expected::Bucket(pair.to_string()))],
        },
        Row {
            case: "the three-part array form, the attribute path element by element",
            stack: "getatt-triple",
            children: vec![Child::new("Child", "getatt-triple-child.json", triple)],
            outputs: vec![(
                "NestedBucketName",
                json!({"Fn::GetAtt": ["Child", "Outputs", "BucketName"]}),
            )],
            expected: vec![("NestedBucketName", Expected::Bucket(triple.to_string()))],
        },
        Row {
            case: "an Fn::Sub variable, and an output composed into a longer value",
            stack: "getatt-composed",
            children: vec![Child::new("Child", "getatt-composed-child.json", composed)],
            outputs: vec![
                ("NestedArn", json!({"Fn::Sub": "${Child.Outputs.BucketArn}"})),
                (
                    "NestedPrefix",
                    json!({
                        "Fn::Join": [
                            "/",
                            [{"Fn::GetAtt": "Child.Outputs.BucketName"}, "data"]
                        ]
                    }),
                ),
            ],
            expected: vec![
                ("NestedArn", Expected::Exact(bucket_arn(composed))),
                (
                    "NestedPrefix",
                    Expected::Exact(format!("{composed}/data")),
                ),
            ],
        },
        Row {
            case: "every kind of value one child publishes",
            stack: "getatt-kinds",
            children: vec![Child::new("Child", "getatt-kinds-child.json", kinds)],
            outputs: vec![
                (
                    "NestedBucketName",
                    json!({"Fn::GetAtt": "Child.Outputs.BucketName"}),
                ),
                (
                    "NestedBucketArn",
                    json!({"Fn::GetAtt": "Child.Outputs.BucketArn"}),
                ),
                ("NestedLabel", json!({"Fn::GetAtt": "Child.Outputs.Label"})),
            ],
            expected: vec![
                ("NestedBucketName", Expected::Bucket(kinds.to_string())),
                ("NestedBucketArn", Expected::Exact(bucket_arn(kinds))),
                (
                    "NestedLabel",
                    Expected::Exact(PUBLISHED_LITERAL.to_string()),
                ),
            ],
        },
        Row {
            case: "two children under different logical ids",
            stack: "getatt-two",
            children: vec![
                Child::new("First", "getatt-two-first-child.json", first),
                Child::new("Second", "getatt-two-second-child.json", second),
            ],
            outputs: vec![
                (
                    "FirstBucketName",
                    json!({"Fn::GetAtt": "First.Outputs.BucketName"}),
                ),
                (
                    "SecondBucketName",
                    json!({"Fn::GetAtt": ["Second", "Outputs.BucketName"]}),
                ),
            ],
            expected: vec![
                ("FirstBucketName", Expected::Bucket(first.to_string())),
                ("SecondBucketName", Expected::Bucket(second.to_string())),
            ],
        },
        Row {
            case: "an output key the child does not publish, beside one it does",
            stack: "getatt-partial",
            children: vec![Child::new("Child", "getatt-partial-child.json", partial)],
            outputs: vec![
                (
                    "NestedBucketName",
                    json!({"Fn::GetAtt": "Child.Outputs.BucketName"}),
                ),
                (
                    "NestedMissing",
                    json!({"Fn::GetAtt": "Child.Outputs.NotDeclared"}),
                ),
            ],
            expected: vec![
                ("NestedBucketName", Expected::Bucket(partial.to_string())),
                ("NestedMissing", Expected::NoValue),
            ],
        },
    ]
}

/// The parent's template: one nested-stack resource per child, and the outputs the row reads.
fn parent_template(row: &Row) -> String {
    let mut resources = Map::new();
    for child in &row.children {
        resources.insert(
            child.logical_id.to_string(),
            json!({
                "Type": STACK_RESOURCE_TYPE,
                "Properties": {"TemplateURL": staged_url(child.key)}
            }),
        );
    }
    let mut outputs = Map::new();
    for (key, value) in &row.outputs {
        outputs.insert((*key).to_string(), json!({"Value": value}));
    }
    json!({"Resources": resources, "Outputs": outputs}).to_string()
}

/// Every `Output` one `DescribeStacks` answer reports, as `(OutputKey, Option<OutputValue>)`.
///
/// Read entry by entry rather than as two parallel lists: whether an entry carries an
/// `OutputValue` at all is the very thing these rows are about, and a missing value would shift
/// a parallel pairing instead of reading as the absence it is.
fn reported_outputs(body: &str) -> Vec<(String, Option<String>)> {
    match xml_blocks(body, "Outputs").into_iter().next() {
        Some(block) => xml_blocks(&block, "member")
            .iter()
            .map(|entry| {
                (
                    xml_text(entry, "OutputKey").unwrap_or_default(),
                    xml_text(entry, "OutputValue"),
                )
            })
            .collect(),
        None => Vec::new(),
    }
}

#[tokio::test]
async fn a_parent_reports_the_outputs_its_nested_stacks_published() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    for row in rows() {
        for child in &row.children {
            stage(&router, child.key, &child_template(&child.bucket)).await;
        }
    }

    for row in rows() {
        let case = row.case;
        let template = parent_template(&row);
        let (code, body) = create_stack(&router, row.stack, &template, &[]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the parent must be created, got {code} {body}"
        );
        // Each level of nesting is one span of the clock: the parent's resource is held open
        // until CloudFormation reports the child settled, so the store is settled until it is.
        for _ in 0..5 {
            settle_the_store(&router, &engine).await;
        }

        let (code, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: expected the parent complete, got {code} {body}"
        );
        let reported = reported_outputs(&body);
        for (key, expect) in &row.expected {
            let (_, actual) = reported
                .iter()
                .find(|(held, _)| held == key)
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected an output {key:?}, got {reported:?}"
                    )
                });
            match expect {
                Expected::NoValue => assert!(
                    actual.is_none(),
                    "case {case:?}: expected {key:?} to carry no OutputValue, got {actual:?}"
                ),
                Expected::Exact(text) => assert_eq!(
                    actual.as_deref(),
                    Some(text.as_str()),
                    "case {case:?}: expected {key:?} to report {text:?}, got {actual:?}"
                ),
                Expected::Bucket(bucket) => {
                    assert_eq!(
                        actual.as_deref(),
                        Some(bucket.as_str()),
                        "case {case:?}: expected {key:?} to report the child's bucket \
                         {bucket:?}, got {actual:?}"
                    );
                    let head = s3(&router, "HEAD", &format!("/{bucket}"), b"").await;
                    assert_eq!(
                        head.status, 200,
                        "case {case:?}: expected the bucket {bucket:?} the output names to \
                         exist, got {}",
                        head.status
                    );
                }
            }
        }
    }
}
