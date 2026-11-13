// SPDX-License-Identifier: CC0-1.0

//! Reading one VPC back off the ec2Query wire.
//!
//! [`super::ec2_query`] holds the plumbing every suite shares — building the form body,
//! dispatching it, and pulling elements out of the XML. These helpers sit on top of it for
//! the responses that carry ONE VPC (`CreateVpc`'s, and a `DescribeVpcs` narrowed to a
//! single `VpcId`), where a scalar member has to be read without the identically-named
//! members nested inside an association or a tag: a CIDR association has a `<state>` of
//! its own, and so does the VPC. The reading goes through `roxmltree`, the same parser the
//! instance suite uses, so nesting is resolved by the document rather than by string
//! surgery.

/// One form field of a request: the key as ec2Query spells it, and its raw value.
pub type Param = (String, String);

/// One form field: `("CidrBlock", "10.0.0.0/16")` as the call helpers want it.
pub fn param(name: &str, value: &str) -> Param {
    (name.to_string(), value.to_string())
}

/// The element names of the sets nested inside a `<vpc>`: a scalar member of the VPC is
/// never read from inside one of these.
const NESTED_SETS: [&str; 3] = [
    "cidrBlockAssociationSet",
    "ipv6CidrBlockAssociationSet",
    "tagSet",
];

/// The response as a document, or a panic naming the case.
pub fn document<'a>(case: &str, body: &'a str) -> roxmltree::Document<'a> {
    roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the body must be XML ({error}): {body}"))
}

/// The first child element of `node` named `name`.
pub fn child<'a, 'i>(
    node: roxmltree::Node<'a, 'i>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|inner| inner.is_element() && inner.tag_name().name() == name)
}

/// The text of the first child element of `node` named `name`.
pub fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name).map(|inner| inner.text().unwrap_or_default().to_string())
}

/// The one VPC a response carries: the element that has a `vpcId` child.
fn vpc<'a, 'i>(node: &'a roxmltree::Document<'i>) -> Option<roxmltree::Node<'a, 'i>> {
    node.descendants()
        .find(|inner| inner.is_element() && child(*inner, "vpcId").is_some())
}

/// The first `<item>` of the VPC's `name` set.
fn set_item<'a, 'i>(
    vpc: roxmltree::Node<'a, 'i>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'i>> {
    child(vpc, name).and_then(|set| child(set, "item"))
}

/// Every entry of the `name` set of `node` — the elements an ec2Query list member is
/// written as. Reading the TREE is what keeps a nested set inside its parent: a volume
/// status item's `<details>` entries are `<item>`s of their own.
pub fn set_items<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Vec<roxmltree::Node<'a, 'i>> {
    child(node, name)
        .map(|set| set.children().filter(roxmltree::Node::is_element).collect())
        .unwrap_or_default()
}

/// A scalar member of the one VPC a response carries, or a panic naming the case.
///
/// A member of the VPC itself (`state`, `cidrBlock`, …) is a direct child; one of its
/// scalar sub-structures (`blockPublicAccessStates/internetGatewayBlockMode`) is read
/// through that child. The nested SETS are skipped, so the VPC's own `<state>` is never
/// confused with an association's.
pub fn member(case: &str, body: &str, name: &str) -> String {
    let document = document(case, body);
    let vpc = vpc(&document)
        .unwrap_or_else(|| panic!("case {case:?}: no VPC (no <vpcId>) in {body}"));
    child_text(vpc, name)
        .or_else(|| {
            vpc.children()
                .filter(|node| {
                    node.is_element() && !NESTED_SETS.contains(&node.tag_name().name())
                })
                .find_map(|node| child_text(node, name))
        })
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the VPC in {body}"))
}

/// A member of the one IPv4 CIDR association a response carries. `state` is the
/// association's own, inside `cidrBlockState`.
pub fn ipv4_association(case: &str, body: &str, name: &str) -> String {
    association(case, body, "cidrBlockAssociationSet", "cidrBlockState", name)
}

/// A member of the one IPv6 CIDR association a response carries. `state` is the
/// association's own, inside `ipv6CidrBlockState`.
pub fn ipv6_association(case: &str, body: &str, name: &str) -> String {
    association(
        case,
        body,
        "ipv6CidrBlockAssociationSet",
        "ipv6CidrBlockState",
        name,
    )
}

/// A member of the first association of `set`, looking inside `state_member` for one the
/// association carries there.
fn association(case: &str, body: &str, set: &str, state_member: &str, name: &str) -> String {
    let document = document(case, body);
    let vpc = vpc(&document)
        .unwrap_or_else(|| panic!("case {case:?}: no VPC (no <vpcId>) in {body}"));
    let item = set_item(vpc, set)
        .unwrap_or_else(|| panic!("case {case:?}: no <{set}> item in {body}"));
    child_text(item, name)
        .or_else(|| child(item, state_member).and_then(|state| child_text(state, name)))
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in the {set} item of {body}"))
}

/// Whether a response carries an IPv6 CIDR association at all.
pub fn has_ipv6_association(body: &str) -> bool {
    let Ok(document) = roxmltree::Document::parse(body) else {
        return false;
    };
    vpc(&document)
        .and_then(|vpc| set_item(vpc, "ipv6CidrBlockAssociationSet"))
        .and_then(|item| child_text(item, "ipv6CidrBlock"))
        .is_some()
}

/// The tags of the one VPC a response carries, in the order it reported them.
pub fn vpc_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let document = document(case, body);
    let Some(vpc) = vpc(&document) else {
        return Vec::new();
    };
    let Some(set) = child(vpc, "tagSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// Whether `value` is an EC2 long resource id of `prefix`: the prefix, a hyphen and 17
/// lowercase hex digits, the form every EC2 resource id has taken since the long-id
/// change.
pub fn is_long_id(prefix: &str, value: &str) -> bool {
    value
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|suffix| {
            suffix.len() == 17
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

/// The `(managed, principal)` the one LAUNCH TEMPLATE a response carries answers in its
/// `operator` — the member a client reads to tell a customer-created template from a
/// service-managed one.
///
/// The operator is read as a child of the element that carries the template's own
/// `launchTemplateId`, so it is the template's and not a member of anything else the body
/// holds; it panics naming the case when the template carries no `operator` at all, that
/// absence being exactly what a reader of this member cannot act on. The ONE reading of that
/// member for every suite that asserts on it, whichever operation answered the body.
pub fn reported_operator(case: &str, body: &str) -> (Option<String>, Option<String>) {
    let parsed = document(case, body);
    let template = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "launchTemplateId").is_some())
        .unwrap_or_else(|| {
            panic!("case {case:?}: no launch template (no <launchTemplateId>) in {body}")
        });
    let operator = child(template, "operator").unwrap_or_else(|| {
        panic!("case {case:?}: no <operator> on the launch template in {body}")
    });
    (
        child_text(operator, "managed"),
        child_text(operator, "principal"),
    )
}
