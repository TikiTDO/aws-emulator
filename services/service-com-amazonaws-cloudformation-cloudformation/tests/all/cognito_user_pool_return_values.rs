// SPDX-License-Identifier: CC0-1.0

//! The published return values of an Amazon Cognito user pool a stack declares, read back
//! out of the stack's `Outputs` over the awsQuery wire.
//!
//! RULE under test: **`Fn::GetAtt` resolves EVERY attribute a resource type's published
//! "Return values" section defines, and an attribute that is not the resource's name, its
//! ARN or its physical id — the three CloudFormation's uniform naming already answers — is
//! computed from the resource's OWN identity in the form the service that owns the type
//! publishes for it.** So every output of a stack whose deployment completed carries an
//! `OutputValue`: an output is reported without one only where the expression names
//! something this emulator does not hold, and a published attribute of a declared resource
//! is never that.
//!
//! The reported case is an `AWS::Cognito::UserPool` whose `ProviderName` an output
//! published: `Fn::GetAtt` answered nothing for it, so `DescribeStacks` reported that
//! output with an `OutputKey` and no `OutputValue` and a client building a key/value map
//! from `Stacks[0].Outputs` could read nothing out of the stack at all.
//!
//! The rows differ along what the rule generalises over:
//!
//! * the ATTRIBUTE — every name
//!   `state::template::published_attribute_names("AWS::Cognito::UserPool")` carries is a
//!   column of every row, so the catalogue itself drives the table and an attribute added
//!   to it without a value fails this suite. `ProviderURL` and `Arn` are therefore asserted
//!   beside the reported `ProviderName`;
//! * the SPELLING the output reaches the attribute through — the long-form `Fn::GetAtt`,
//!   the YAML short form and a `Fn::Sub` variable — crossed with both template languages;
//! * the pool's IDENTITY — a pool the template NAMES and one it leaves CloudFormation to
//!   name: either way the identity the attributes are read off is the id Amazon Cognito
//!   MINTED for the pool its create made, since the name is only what that create was made
//!   under;
//! * the REGION the stack is created in, which both the ARN and the pool's endpoint host
//!   carry.
//!
//! Every expected value is computed from the identity the SAME response reports for the
//! pool (its `Ref`), never from a literal written here, so a row asserts that the
//! attributes name the pool the stack published rather than that they equal one recorded
//! string.

use serde_json::{json, Map, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use service_com_amazonaws_cloudformation_cloudformation::state::template::published_attribute_names;

use crate::harness::{call_in, member, router, wait_for_the_transition, xml_text, REGION};
use crate::stack_lifecycle::ACCOUNT_ID;
use crate::stack_outputs::{output_for, wire_outputs};

/// The resource type whose published return values this suite is about.
const USER_POOL: &str = "AWS::Cognito::UserPool";

/// The logical id every row declares the pool under.
const LOGICAL_ID: &str = "UserPool";

/// The output key carrying `Ref` of the pool — the identity every other output's expected
/// value is computed from.
const REF_KEY: &str = "PoolRef";

/// An attribute name the type does NOT publish, which CloudFormation's uniform naming does
/// not answer either and which no row declares as a property: a near miss of the reported
/// `ProviderName`.
const UNPUBLISHED_ATTRIBUTE: &str = "ProviderEndpoint";

/// Which language a row's template is written in, and therefore which spelling of
/// `Fn::GetAtt` its outputs use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// A JSON body: the long form `{"Fn::GetAtt": ["UserPool", "<attribute>"]}`.
    Json,
    /// A YAML body: the short form `!GetAtt UserPool.<attribute>`.
    Yaml,
}

/// One row: where the stack lives, how its template is written and whether it names the
/// pool.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    region: &'static str,
    spelling: Spelling,
    /// The `UserPoolName` the template declares, or `None` for a pool CloudFormation names.
    declared_name: Option<&'static str>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "a named pool, JSON, long-form Fn::GetAtt",
            stack_name: "cognito-pool-json",
            region: REGION,
            spelling: Spelling::Json,
            declared_name: Some("ci-named-pool"),
        },
        Case {
            case: "a pool CloudFormation names itself, JSON",
            stack_name: "cognito-pool-generated",
            region: REGION,
            spelling: Spelling::Json,
            declared_name: None,
        },
        Case {
            case: "a named pool, YAML, short-form Fn::GetAtt",
            stack_name: "cognito-pool-yaml",
            region: REGION,
            spelling: Spelling::Yaml,
            declared_name: Some("ci-yaml-pool"),
        },
        Case {
            case: "a pool in another Region, YAML",
            stack_name: "cognito-pool-elsewhere",
            region: "eu-west-2",
            spelling: Spelling::Yaml,
            declared_name: Some("ci-elsewhere-pool"),
        },
    ]
}

/// The output key an attribute is published under, per spelling: the attribute reached
/// directly, and the same attribute reached through a `Fn::Sub` variable.
fn direct_key(attribute: &str) -> String {
    format!("Att{attribute}")
}

fn substituted_key(attribute: &str) -> String {
    format!("Sub{attribute}")
}

/// A template declaring one user pool and publishing, for every attribute the type
/// publishes, that attribute reached directly and through a `Fn::Sub` variable, beside
/// `Ref` of the pool itself.
fn template(case: &Case) -> String {
    let attributes = published_attribute_names(USER_POOL);
    match case.spelling {
        Spelling::Json => {
            let mut properties = Map::new();
            if let Some(name) = case.declared_name {
                properties.insert("UserPoolName".to_string(), json!(name));
            }
            // A pool declared with the two properties a user pool is configured with
            // beside its name, so the declaration is a pool and not just a type name.
            properties.insert("MfaConfiguration".to_string(), json!("OFF"));
            properties.insert(
                "AdminCreateUserConfig".to_string(),
                json!({ "AllowAdminCreateUserOnly": true }),
            );
            let mut outputs = Map::new();
            outputs.insert(
                REF_KEY.to_string(),
                json!({ "Value": { "Ref": LOGICAL_ID } }),
            );
            for attribute in attributes {
                outputs.insert(
                    direct_key(attribute),
                    json!({ "Value": { "Fn::GetAtt": [LOGICAL_ID, attribute] } }),
                );
                outputs.insert(
                    substituted_key(attribute),
                    json!({ "Value": { "Fn::Sub": format!("${{{LOGICAL_ID}.{attribute}}}") } }),
                );
            }
            json!({
                "Resources": {
                    LOGICAL_ID: { "Type": USER_POOL, "Properties": Value::Object(properties) }
                },
                "Outputs": Value::Object(outputs),
            })
            .to_string()
        }
        Spelling::Yaml => {
            let mut body = String::from("Resources:\n  UserPool:\n    Type: AWS::Cognito::UserPool\n    Properties:\n      MfaConfiguration: OFF\n");
            if let Some(name) = case.declared_name {
                body.push_str(&format!("      UserPoolName: {name}\n"));
            }
            body.push_str("Outputs:\n");
            body.push_str(&format!("  {REF_KEY}:\n    Value: !Ref {LOGICAL_ID}\n"));
            for attribute in attributes {
                body.push_str(&format!(
                    "  {}:\n    Value: !GetAtt {LOGICAL_ID}.{attribute}\n",
                    direct_key(attribute)
                ));
                body.push_str(&format!(
                    "  {}:\n    Value: !Sub \"${{{LOGICAL_ID}.{attribute}}}\"\n",
                    substituted_key(attribute)
                ));
            }
            body
        }
    }
}

/// What a published attribute of a user pool with identity `pool` in `region` must be.
///
/// Source: the "Return values" section the CloudFormation resource and property type
/// reference publishes for `AWS::Cognito::UserPool` — `Arn` is the pool's ARN
/// (`arn:aws:cognito-idp:<region>:<account>:userpool/<pool id>`), `ProviderName` is the
/// pool's provider name and `ProviderURL` the URL of that provider, which Amazon Cognito
/// serves a user pool's OpenID metadata under
/// (`cognito-idp.<region>.amazonaws.com/<pool id>`, the same host as an `https` URL).
fn expected(attribute: &str, region: &str, pool: &str) -> String {
    let host = format!("cognito-idp.{region}.amazonaws.com");
    match attribute {
        "Arn" => format!("arn:aws:cognito-idp:{region}:{ACCOUNT_ID}:userpool/{pool}"),
        "ProviderName" => format!("{host}/{pool}"),
        "ProviderURL" => format!("https://{host}/{pool}"),
        other => panic!(
            "the published attribute {other:?} of {USER_POOL} has no expected value here: \
             add the value its \"Return values\" section publishes"
        ),
    }
}

/// `CreateStack` in `region`, waiting past the instant its create settles, answered with
/// the `DescribeStacks` body.
async fn deploy(case: &Case, body: &str) -> String {
    let router = router();
    let (status_code, response) = call_in(
        &router,
        case.region,
        "CreateStack",
        &[
            member("StackName", case.stack_name),
            member("TemplateBody", body),
        ],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {:?}: expected HTTP 200 from CreateStack, got {status_code} with body \
         {response}",
        case.case
    );
    wait_for_the_transition().await;
    let (status_code, response) = call_in(
        &router,
        case.region,
        "DescribeStacks",
        &[member("StackName", case.stack_name)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {:?}: expected DescribeStacks to answer, got {status_code} with body \
         {response}",
        case.case
    );
    assert_eq!(
        xml_text(&response, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {:?}: expected the stack to reach {:?}, got body {response}",
        case.case,
        status::CREATE_COMPLETE
    );
    response
}

#[tokio::test]
async fn every_published_user_pool_attribute_resolves_in_a_stacks_outputs() {
    let attributes = published_attribute_names(USER_POOL);
    assert!(
        attributes.contains(&"ProviderName"),
        "the catalogue must publish the reported attribute ProviderName for {USER_POOL}, \
         it carries {attributes:?}"
    );

    for case in cases() {
        let response = deploy(&case, &template(&case)).await;
        let outputs = wire_outputs(&response);

        // The symptom itself, over the WHOLE response: a stack whose deployment completed
        // reports no output without a value.
        for output in &outputs {
            let key = output.key.clone().unwrap_or_default();
            assert!(
                output.value.is_some(),
                "case {:?}: every Output of a completed stack must carry an OutputValue; \
                 {key:?} carries none ({outputs:?})",
                case.case
            );
        }

        // The pool's own identity, as the same response reports it. Every expected value
        // below is computed from THIS, so the row asserts that the published attributes
        // name the pool the stack published.
        let pool = output_for(&outputs, REF_KEY)
            .and_then(|output| output.value.clone())
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the stack to publish Ref of the pool, got \
                     {outputs:?}",
                    case.case
                )
            });
        // "Ref returns the user pool ID", which is the id Amazon Cognito MINTED for the pool
        // its create made — `UserPoolIdType`, `^[\w-]+_[0-9a-zA-Z]+$`, built from the Region
        // the pool lives in — whether or not the template named the pool: the name the
        // declaration states, or the one CloudFormation generated for it, is only what the
        // create was made under.
        assert!(
            pool.starts_with(&format!("{}_", case.region))
                && pool.rsplit('_').next().is_some_and(|minted| {
                    !minted.is_empty() && minted.chars().all(|c| c.is_ascii_alphanumeric())
                }),
            "case {:?}: expected the id Amazon Cognito minted for the pool, got {pool:?}",
            case.case
        );

        for attribute in attributes {
            let wanted = expected(attribute, case.region, &pool);
            for key in [direct_key(attribute), substituted_key(attribute)] {
                let reported = output_for(&outputs, &key)
                    .and_then(|output| output.value.clone())
                    .unwrap_or_else(|| {
                        panic!(
                            "case {:?}: no resolved Output {key:?} for the published \
                             attribute {attribute:?}; got {outputs:?}",
                            case.case
                        )
                    });
                assert_eq!(
                    reported, wanted,
                    "case {:?}: expected the published attribute {attribute:?} of the pool \
                     {pool:?} to be {wanted:?}, got {reported:?}",
                    case.case
                );
            }
        }
    }
}

/// The other half of the rule: an attribute the type does NOT publish — and that
/// CloudFormation's uniform `Arn`/`Id`/`Name` naming does not answer, and no property
/// declares — resolves to nothing, so the catalogue is not a blanket "every name answers".
#[tokio::test]
async fn an_attribute_the_type_does_not_publish_resolves_to_nothing() {
    let case = Case {
        case: "a near miss of the published ProviderName",
        stack_name: "cognito-pool-near-miss",
        region: REGION,
        spelling: Spelling::Json,
        declared_name: Some("ci-near-miss-pool"),
    };
    let body = json!({
        "Resources": {
            LOGICAL_ID: {
                "Type": USER_POOL,
                "Properties": { "UserPoolName": case.declared_name }
            }
        },
        "Outputs": {
            REF_KEY: { "Value": { "Ref": LOGICAL_ID } },
            "Published": { "Value": { "Fn::GetAtt": [LOGICAL_ID, "ProviderName"] } },
            "NearMiss": {
                "Value": { "Fn::GetAtt": [LOGICAL_ID, UNPUBLISHED_ATTRIBUTE] }
            },
        },
    })
    .to_string();

    let response = deploy(&case, &body).await;
    let outputs = wire_outputs(&response);
    // The pool's own identity, as the same response reports it: the id Amazon Cognito minted,
    // which is what every published attribute of the pool is read off.
    let pool = output_for(&outputs, REF_KEY)
        .and_then(|output| output.value.clone())
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the stack to publish Ref of the pool, got {outputs:?}",
                case.case
            )
        });
    let published = output_for(&outputs, "Published").and_then(|output| output.value.clone());
    assert_eq!(
        published.as_deref(),
        Some(expected("ProviderName", case.region, &pool).as_str()),
        "case {:?}: expected the published attribute to resolve, got {published:?}",
        case.case
    );
    let near_miss = output_for(&outputs, "NearMiss").unwrap_or_else(|| {
        panic!(
            "case {:?}: expected the declared output to be reported, got {outputs:?}",
            case.case
        )
    });
    assert!(
        near_miss.value.is_none(),
        "case {:?}: expected {UNPUBLISHED_ATTRIBUTE:?}, which {USER_POOL} does not \
         publish, to carry no OutputValue, got {:?}",
        case.case,
        near_miss.value
    );
}
