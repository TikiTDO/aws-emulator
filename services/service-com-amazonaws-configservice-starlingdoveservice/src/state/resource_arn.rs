// SPDX-License-Identifier: CC0-1.0

//! The ARN of a resource Config has RECORDED, derived from the type and the id its
//! configuration item carries.
//!
//! A configuration item reports the "Amazon Resource Name (ARN) associated with the
//! resource" (`ConfigurationItem$arn`; `BaseConfigurationItem$arn`: "The Amazon
//! Resource Name (ARN) of the resource"), and it is the ONLY member of the item that
//! carries one — `resourceId` and `resourceName` carry the bare id and the bare name.
//! An observation the OWNING service handed an ARN over for keeps that ARN on the
//! inventory entry ([`super::DiscoveredResourceEntry::arn`]), because an ARN the service
//! that mints it stated is the one its resource has. For every other recorded resource
//! the member is DERIVED here from the two members that identify it:
//!
//! * the ARN's service field is the ARN namespace of the service that OWNS the
//!   resource type (the type's middle segment names that service);
//! * its Region and account fields are the ones the configuration item is reported
//!   under, except where that service's ARNs leave either empty;
//! * its resource field is the qualifier that service's ARNs give the resource type,
//!   followed by the recorded resource id.
//!
//! Each owning service publishes the ARN format of its own resources, so the forms are
//! a table ([`RESOURCE_ARN_FORMS`], sourced on the constant) keyed by resource type; a
//! type the table does not name takes the shape most services use, built the same way
//! from the type's own segments. A resource type NO Amazon Web Services service owns —
//! a CloudFormation registry type a caller records through `PutResourceConfig` — names
//! a resource that has no such ARN, and no call carries one, so nothing is derived for
//! it rather than an ARN being invented.
//!
//! Two recorded ids are not the resource's own name: Config records some resource types by
//! the resource's own ARN, and some by the URL its own service addresses the resource at
//! (an Amazon SQS queue url). Neither is an ARN resource field, so the first IS the answer
//! and the second is read down to the resource it names ([`arn_resource_id`]).

use super::is_amazon_resource_name;

/// The partition every ARN this emulator mints belongs to
/// (`::emulator_server::emulator_arn` builds `arn:aws:…`).
pub const ARN_PARTITION: &str = "aws";

/// The organization segment of a resource type the Amazon Web Services services own
/// (`AWS::<Service>::<Resource>`), as opposed to the registry type name of a custom
/// resource type. Matched case-insensitively, the way `PutResourceConfig` matches the
/// organization names it reserves.
const AWS_ORGANIZATION: &str = "AWS";

/// Which of an ARN's two scope fields — the Region and the account — the ARNs of one
/// service carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArnScope {
    /// Both: a regional resource of one account
    /// (`arn:aws:ec2:<region>:<account>:instance/<id>`).
    RegionAndAccount,
    /// The account only: a resource of a GLOBAL service, which belongs to an account
    /// but to no Region (`arn:aws:iam::<account>:role/<name>`).
    AccountOnly,
    /// The Region only: a resource whose ARN names where it lives but not whose it is
    /// (`arn:aws:apigateway:<region>::/restapis/<id>`).
    RegionOnly,
    /// Neither: a resource whose name is unique across every account and Region, so
    /// its ARN names no owner (`arn:aws:s3:::<bucket>`).
    Neither,
}

/// How the ARN of one resource type is formed.
#[derive(Debug, Clone, Copy)]
pub struct ResourceArnForm {
    /// The resource type this form belongs to, a
    /// `com.amazonaws.configservice#ResourceType` value.
    pub resource_type: &'static str,
    /// The ARN namespace of the service that owns the type — the ARN's service field,
    /// which is not always the type's middle segment in lowercase.
    pub namespace: &'static str,
    /// Which scope fields the ARN carries.
    pub scope: ArnScope,
    /// What the ARN's resource field carries BEFORE the resource's own id, its
    /// separator included: `"instance/"`, `"function:"`, or `""` for a type whose id
    /// stands alone in the resource field.
    pub qualifier: &'static str,
}

/// The ARN form of each resource type whose ARNs do not take the shape
/// [`recorded_resource_arn`] derives from the resource type's own segments.
///
/// Source: the ARN format each owning service publishes for its own resources (the
/// "Amazon Resource Names" formats of the AWS General Reference), one entry per
/// `com.amazonaws.configservice#ResourceType` value
/// (`smithy_query(query="shape", shape="ResourceType")`). A type absent from this
/// table is NOT unsupported — it falls back to the shape most services use, which is a
/// BEST-EFFORT form read off the type's own segments ([`recorded_resource_arn`]) — so
/// the table carries the types whose namespace, scope or qualifier differs from it, and
/// a type whose published form turns out to differ structurally belongs here.
///
/// A recorder in this emulator observes the resource types
/// [`super::recording::OBSERVABLE_RESOURCE_TYPES`] names, so the other entries state
/// the form for a type this emulator does not record YET rather than one it serves
/// today; they are here because the rule is one rule, and the table grows with what a
/// recorder learns to observe.
pub const RESOURCE_ARN_FORMS: &[ResourceArnForm] = &[
    // An Amazon S3 bucket name is unique across every account and Region, and its ARN
    // carries neither (`arn:aws:s3:::<bucket>`).
    ResourceArnForm {
        resource_type: "AWS::S3::Bucket",
        namespace: "s3",
        scope: ArnScope::Neither,
        qualifier: "",
    },
    // An S3 access point, unlike a bucket, belongs to one account in one Region.
    ResourceArnForm {
        resource_type: "AWS::S3::AccessPoint",
        namespace: "s3",
        scope: ArnScope::RegionAndAccount,
        qualifier: "accesspoint/",
    },
    // IAM is global: its ARNs carry the account and no Region.
    ResourceArnForm {
        resource_type: "AWS::IAM::Role",
        namespace: "iam",
        scope: ArnScope::AccountOnly,
        qualifier: "role/",
    },
    ResourceArnForm {
        resource_type: "AWS::IAM::User",
        namespace: "iam",
        scope: ArnScope::AccountOnly,
        qualifier: "user/",
    },
    ResourceArnForm {
        resource_type: "AWS::IAM::Group",
        namespace: "iam",
        scope: ArnScope::AccountOnly,
        qualifier: "group/",
    },
    ResourceArnForm {
        resource_type: "AWS::IAM::Policy",
        namespace: "iam",
        scope: ArnScope::AccountOnly,
        qualifier: "policy/",
    },
    ResourceArnForm {
        resource_type: "AWS::IAM::InstanceProfile",
        namespace: "iam",
        scope: ArnScope::AccountOnly,
        qualifier: "instance-profile/",
    },
    // Route 53 is global, and a hosted zone's ARN carries no account either.
    ResourceArnForm {
        resource_type: "AWS::Route53::HostedZone",
        namespace: "route53",
        scope: ArnScope::Neither,
        qualifier: "hostedzone/",
    },
    // CloudFront is global: its ARNs carry the account and no Region.
    ResourceArnForm {
        resource_type: "AWS::CloudFront::Distribution",
        namespace: "cloudfront",
        scope: ArnScope::AccountOnly,
        qualifier: "distribution/",
    },
    // A Lambda function's ARN separates the qualifier from the name with a colon.
    ResourceArnForm {
        resource_type: "AWS::Lambda::Function",
        namespace: "lambda",
        scope: ArnScope::RegionAndAccount,
        qualifier: "function:",
    },
    // A CloudWatch alarm's ARN does the same.
    ResourceArnForm {
        resource_type: "AWS::CloudWatch::Alarm",
        namespace: "cloudwatch",
        scope: ArnScope::RegionAndAccount,
        qualifier: "alarm:",
    },
    // An Amazon SQS queue's ARN carries the queue name alone, with no qualifier.
    ResourceArnForm {
        resource_type: "AWS::SQS::Queue",
        namespace: "sqs",
        scope: ArnScope::RegionAndAccount,
        qualifier: "",
    },
    // An Amazon SNS topic's ARN does the same.
    ResourceArnForm {
        resource_type: "AWS::SNS::Topic",
        namespace: "sns",
        scope: ArnScope::RegionAndAccount,
        qualifier: "",
    },
    // Both generations of Elastic Load Balancing mint ARNs in the `elasticloadbalancing`
    // namespace, which neither type's middle segment spells.
    ResourceArnForm {
        resource_type: "AWS::ElasticLoadBalancing::LoadBalancer",
        namespace: "elasticloadbalancing",
        scope: ArnScope::RegionAndAccount,
        qualifier: "loadbalancer/",
    },
    ResourceArnForm {
        resource_type: "AWS::ElasticLoadBalancingV2::LoadBalancer",
        namespace: "elasticloadbalancing",
        scope: ArnScope::RegionAndAccount,
        qualifier: "loadbalancer/",
    },
    ResourceArnForm {
        resource_type: "AWS::ElasticLoadBalancingV2::TargetGroup",
        namespace: "elasticloadbalancing",
        scope: ArnScope::RegionAndAccount,
        qualifier: "targetgroup/",
    },
    // An Amazon RDS DB instance's ARN spells its qualifier `db` and separates it with a
    // colon; a DB cluster's is `cluster`.
    ResourceArnForm {
        resource_type: "AWS::RDS::DBInstance",
        namespace: "rds",
        scope: ArnScope::RegionAndAccount,
        qualifier: "db:",
    },
    ResourceArnForm {
        resource_type: "AWS::RDS::DBCluster",
        namespace: "rds",
        scope: ArnScope::RegionAndAccount,
        qualifier: "cluster:",
    },
    // An Amazon OpenSearch Service domain's ARNs are minted in the `es` namespace.
    ResourceArnForm {
        resource_type: "AWS::OpenSearch::Domain",
        namespace: "es",
        scope: ArnScope::RegionAndAccount,
        qualifier: "domain/",
    },
    ResourceArnForm {
        resource_type: "AWS::Elasticsearch::Domain",
        namespace: "es",
        scope: ArnScope::RegionAndAccount,
        qualifier: "domain/",
    },
    // A Step Functions state machine's ARNs are minted in the `states` namespace, and
    // its qualifier is colon-separated.
    ResourceArnForm {
        resource_type: "AWS::StepFunctions::StateMachine",
        namespace: "states",
        scope: ArnScope::RegionAndAccount,
        qualifier: "stateMachine:",
    },
    // An Amazon API Gateway ARN names the Region but no account, and its resource field
    // is a PATH: `/restapis/<id>` for a REST API, `/apis/<id>` for an HTTP or WebSocket
    // one.
    ResourceArnForm {
        resource_type: "AWS::ApiGateway::RestApi",
        namespace: "apigateway",
        scope: ArnScope::RegionOnly,
        qualifier: "/restapis/",
    },
    ResourceArnForm {
        resource_type: "AWS::ApiGatewayV2::Api",
        namespace: "apigateway",
        scope: ArnScope::RegionOnly,
        qualifier: "/apis/",
    },
];

/// The ARN of the resource one configuration item records, or `None` when the item
/// records a resource that has no such ARN.
///
/// This is the ONE place a recorded resource is resolved to its ARN, so every
/// configuration-item read path — the resource's history, the aggregate reads and the
/// Config query language — reports the same value for the same recorded resource.
///
/// `aws_region` is the Region the configuration item is reported under, and the account
/// is this emulator's own ([`::emulator_server::emulator_account_id`]): a store stands
/// for one account in one Region, which is the account and Region the recorded resource
/// belongs to.
pub fn recorded_resource_arn(
    resource_type: &str,
    resource_id: &str,
    aws_region: &str,
) -> Option<String> {
    // A resource with no id is not identified at all, so there is nothing to name.
    if resource_id.is_empty() {
        return None;
    }
    // Config records some resource types by the resource's own ARN; then the recorded
    // id IS the ARN, and deriving a second one from it would name a resource that does
    // not exist.
    if is_amazon_resource_name(resource_id) {
        return Some(resource_id.to_string());
    }

    // Every resource type name carries `Organization::Service::Resource`: the
    // organization names who owns the type, and the other two the service that serves
    // the resource and the resource itself.
    let segments: Vec<&str> = resource_type.split("::").collect();
    let [organization, service, resource] = segments.as_slice() else {
        return None;
    };
    if organization.is_empty()
        || service.is_empty()
        || resource.is_empty()
        || !organization.eq_ignore_ascii_case(AWS_ORGANIZATION)
    {
        return None;
    }

    let (namespace, scope, qualifier) = match RESOURCE_ARN_FORMS
        .iter()
        .find(|form| form.resource_type == resource_type)
    {
        Some(form) => (
            form.namespace.to_string(),
            form.scope,
            form.qualifier.to_string(),
        ),
        // The shape most services use, read off the type's own segments: the owning
        // service's name in lowercase as the namespace, the resource's Region and
        // account, and the type's resource segment — hyphenated the way an ARN
        // qualifier spells it — as the qualifier. Derived rather than guessed per type,
        // so a resource type the table does not list still reports an ARN that names
        // the owning service and ends in the recorded resource id.
        None => (
            service.to_ascii_lowercase(),
            ArnScope::RegionAndAccount,
            format!("{}/", hyphenated(resource)),
        ),
    };

    let resource_field = format!("{qualifier}{}", arn_resource_id(resource_id));
    Some(match scope {
        // `::emulator_server::emulator_arn` is what every ARN this emulator mints is
        // built through: it carries the partition and the one account id this emulator
        // presents, and a global service's ARN is the same call with an empty Region.
        ArnScope::RegionAndAccount => {
            ::emulator_server::emulator_arn(&namespace, aws_region, &resource_field)
        }
        ArnScope::AccountOnly => ::emulator_server::emulator_arn(&namespace, "", &resource_field),
        // The two scopes whose ARNs name NO account are built here, because the account
        // field `emulator_arn` always fills has to stay empty; the partition is the same
        // one either way ([`ARN_PARTITION`]).
        ArnScope::RegionOnly => {
            format!("arn:{ARN_PARTITION}:{namespace}:{aws_region}::{resource_field}")
        }
        ArnScope::Neither => format!("arn:{ARN_PARTITION}:{namespace}:::{resource_field}"),
    })
}

/// The recorded resource id as an ARN's resource field carries it.
///
/// A resource type Config records by the URL its own service addresses the resource at —
/// an Amazon SQS queue, whose configuration item carries the queue url as its
/// `resourceId` — is named in its ARN by the resource that url addresses, which is its
/// LAST path segment (`https://sqs.<region>.amazonaws.com/<account>/<name>`). Every other
/// recorded id stands in the resource field as it is.
fn arn_resource_id(resource_id: &str) -> &str {
    let Some(path) = resource_id
        .strip_prefix("https://")
        .or_else(|| resource_id.strip_prefix("http://"))
    else {
        return resource_id;
    };
    path.rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(resource_id)
}

/// `name` in the lowercase, hyphen-separated spelling an ARN qualifier uses for a
/// resource type: `SecurityGroup` is `security-group` and `DBInstance` is `db-instance`,
/// while an acronym stays one word (`VPC` is `vpc`).
///
/// A resource type name is ASCII, so every character is classified with the
/// `is_ascii_*` family. The CloudFormation service crate carries its own copy of this
/// rule for the ARNs its templates resolve; the two are crate-private helpers of
/// independent service crates, each a few lines beside the table it serves, so neither
/// is hoisted into a shared crate until a third reader needs it.
fn hyphenated(name: &str) -> String {
    let characters: Vec<char> = name.chars().collect();
    let mut spelled = String::with_capacity(name.len() + 1);
    for (index, character) in characters.iter().enumerate() {
        let previous = index.checked_sub(1).and_then(|before| characters.get(before));
        let next = characters.get(index + 1);
        // A boundary falls where a word ENDS: before an uppercase letter that follows a
        // lowercase one or a digit, and before the last letter of an acronym when a
        // lowercase letter follows it (`VPCEndpoint` is `vpc-endpoint`).
        let after_word = previous
            .is_some_and(|previous| previous.is_ascii_lowercase() || previous.is_ascii_digit());
        let acronym_tail = previous.is_some_and(|previous| previous.is_ascii_uppercase())
            && next.is_some_and(|next| next.is_ascii_lowercase());
        if character.is_ascii_uppercase() && (after_word || acronym_tail) {
            spelled.push('-');
        }
        spelled.extend(character.to_lowercase());
    }
    spelled
}
