// SPDX-License-Identifier: CC0-1.0

//! The VPC security groups the ACCOUNT has, as the service that owns them reports them.
//!
//! A VPC security group is not a resource of this service: it is Amazon EC2's. A client
//! creates one with `ec2:CreateSecurityGroup`, or uses the default group every VPC comes
//! with, and then names it in a `SecurityGroupIds` member — `CreateCacheCluster`,
//! `CreateReplicationGroup`, `CreateServerlessCache` and the three modify operations that
//! change which groups guard an existing resource. So EC2 is the ONE answer to "which
//! security groups does the account have, and which VPC is each one in", and an identifier a
//! request names is resolved there and nowhere else — reached the way any emulated service
//! reaches another, through the in-process callback ([`super::ec2_client`]), so this crate
//! names no other service's crate and nothing leaves the process.
//!
//! Two things are read off EC2's own record, and a request is refused for either:
//!
//! * an identifier EC2 reports NO group for — it can guard nothing, so there is nothing to
//!   attach ([`unrecognized_message`]);
//! * a group that IS the account's but sits in another VPC than the subnets the resource is
//!   placed in — a group only ever guards network interfaces of its own VPC, so it could not
//!   guard this resource either ([`vpc_mismatch_message`]).
//!
//! Both are answered before anything is written: a member whose network access was never
//! established must not be reported as attached, because the caller would then believe its
//! cache is reachable only from a group that does not guard it.
//!
//! Nothing is cached: a group created a moment ago resolves at once, and one that was
//! deleted stops resolving at once.

use ::std::collections::HashSet;

/// The `DescribeSecurityGroups` filter that selects groups BY ID (`group-id`, listed by that
/// operation's own `Filters` documentation).
///
/// A filter is what asks the question rather than `GroupIds`: an identifier the account has
/// no group for is then an absent row instead of a failure of the whole call, so every
/// identifier the request named is still resolved against what EC2 does hold.
const GROUP_ID_FILTER: &str = "group-id";

/// The most group identifiers ONE question asks about, as values of one `group-id` filter.
///
/// The owner evaluates a filter against every security group the account stores, so the
/// cost of asking by identifier is (identifiers asked x groups stored). A request names as
/// many identifiers as it likes, so past this many the question asked is "which groups does
/// the account have" instead — see [`resolve_requested_security_groups`] — and either way
/// ONE request costs ONE question, bounded by what the account stores rather than by what
/// the request carries.
const GROUP_IDS_PER_QUESTION: usize = 64;

/// The most pages of one `DescribeSecurityGroups` answer this reads.
///
/// EC2 pages its answer and reports a `NextToken` while groups of the selection remain, so
/// a selection larger than one page is followed rather than silently truncated — otherwise
/// a group the account HAS would resolve to nothing because of where it landed. The cap
/// keeps the question finite whatever the owner holds.
const GROUP_PAGES_PER_QUESTION: usize = 8;

/// One VPC security group of the account in one Region: the identifier a request names, and
/// the VPC it guards network interfaces of.
///
/// Both fields are read off the record Amazon EC2 — the service that OWNS security groups —
/// answered with; nothing here builds either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSecurityGroup {
    /// The EC2 security-group identifier, as a request names it.
    pub group_id: String,
    /// The VPC this group belongs to.
    pub vpc_id: String,
}

/// Every security group EC2 reports in `region` — the ones `values` asks about, or every
/// group the account has when `values` is `None` — across up to
/// [`GROUP_PAGES_PER_QUESTION`] pages.
///
/// FAIL-CLOSED: a question that could not be asked at all contributes nothing, so the
/// identifiers it would have answered for resolve to nothing and the request is refused. A
/// group that cannot be shown to be the account's is not one a resource is guarded by.
async fn described(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    values: Option<Vec<String>>,
) -> Vec<AccountSecurityGroup> {
    let Some(client) = super::ec2_client::ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return Vec::new();
    };
    let mut reported: Vec<AccountSecurityGroup> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..GROUP_PAGES_PER_QUESTION {
        let filter = values.clone().map(|values| {
            ::aws_sdk_ec2::types::Filter::builder()
                .name(GROUP_ID_FILTER)
                .set_values(Some(values))
                .build()
        });
        match client
            .describe_security_groups()
            .set_filters(filter.map(|filter| vec![filter]))
            .set_next_token(next_token.take())
            .send()
            .await
        {
            Ok(output) => {
                reported.extend(output.security_groups().iter().filter_map(|group| {
                    Some(AccountSecurityGroup {
                        group_id: group.group_id()?.to_string(),
                        // EC2 reports the VPC a group belongs to; a record missing it
                        // carries what this service can report of it, and nothing is
                        // invented for it here.
                        vpc_id: group.vpc_id().unwrap_or_default().to_string(),
                    })
                }));
                match output.next_token() {
                    Some(token) => next_token = Some(token.to_string()),
                    // The whole selection has been reported.
                    None => break,
                }
            }
            Err(error) => {
                // At `warn`: a resource cannot be guarded by groups whose owner cannot be
                // asked, so a deployment where this question fails is one where no request
                // that names a security group is accepted at all — worth seeing, and it IS
                // why an identifier did not resolve.
                ::tracing::warn!(
                    %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the security groups a request named could not be resolved against EC2"
                );
                break;
            }
        }
    }
    reported
}

/// Every security-group identifier a request names, resolved against the groups the account
/// has in `region`, in the order the request listed them and without repeats.
///
/// `Err` carries the identifiers that name NO group of the account, in that same order, so
/// the refusal names exactly what was wrong with the request and nothing is stored.
///
/// ONE source — the service that owns security groups (see this module's own documentation)
/// — asked ONE question, deduplicated, so the work does not grow with how many times one
/// group is named: the identifiers themselves up to [`GROUP_IDS_PER_QUESTION`] of them, and
/// past that what the account HAS, which costs the same whatever a request carries.
///
/// A reported group is matched EXACTLY against what the request asked for, so a value that
/// is a filter pattern rather than an identifier resolves to nothing however many groups the
/// owner reports for it.
pub async fn resolve_requested_security_groups(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    security_group_ids: &[&str],
) -> Result<Vec<AccountSecurityGroup>, Vec<String>> {
    let mut distinct: Vec<&str> = Vec::with_capacity(security_group_ids.len());
    let mut seen: HashSet<&str> = HashSet::new();
    for group_id in security_group_ids {
        if seen.insert(group_id) {
            distinct.push(group_id);
        }
    }
    if distinct.is_empty() {
        return Ok(Vec::new());
    }

    let asked = (distinct.len() <= GROUP_IDS_PER_QUESTION)
        .then(|| distinct.iter().map(|id| (*id).to_string()).collect());
    let held: ::std::collections::HashMap<String, AccountSecurityGroup> =
        described(ctx, region, asked)
            .await
            .into_iter()
            .map(|group| (group.group_id.clone(), group))
            .collect();

    let mut resolved = Vec::with_capacity(distinct.len());
    let mut unresolved: Vec<String> = Vec::new();
    for group_id in distinct {
        match held.get(group_id) {
            Some(group) => resolved.push(group.clone()),
            None => unresolved.push(group_id.to_string()),
        }
    }
    if unresolved.is_empty() {
        Ok(resolved)
    } else {
        Err(unresolved)
    }
}

/// The groups a request names, resolved, or the sentence the request is refused with for an
/// identifier that resolves to nothing.
///
/// The one entry point for an operation that takes a `SecurityGroupIds`, so the six of them
/// cannot drift apart about which identifiers are acceptable or how a bad one is reported.
/// A request that names none resolves to no groups and is refused for nothing.
pub async fn resolved_or_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    security_group_ids: Option<&[String]>,
) -> Result<Vec<AccountSecurityGroup>, String> {
    let requested: Vec<&str> = security_group_ids
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .collect();
    if requested.is_empty() {
        return Ok(Vec::new());
    }
    resolve_requested_security_groups(ctx, region, &requested)
        .await
        .map_err(|unresolved| unrecognized_message(&unresolved))
}

/// How ElastiCache words a request naming security groups the account does not have:
/// `Some security group Id not recognized by EC2: securityGroupIds[[<ids>]],
/// awsAccountId[<account>]`.
///
/// The variable parts are the identifiers that did not resolve — rendered as the list they
/// are, comma-separated inside the inner brackets — and the account the request was made
/// for, which is this emulator's own account (`engine::emulator_account_id`); the rest of
/// the sentence is fixed text.
pub fn unrecognized_message(unresolved: &[String]) -> String {
    format!(
        "Some security group Id not recognized by EC2: securityGroupIds[[{}]], awsAccountId[{}]",
        unresolved.join(", "),
        ::engine::emulator_account_id()
    )
}

/// The VPC of the first resolved group that is NOT in `placement_vpc`, or `None` when every
/// group guards interfaces of the VPC the resource is placed in.
///
/// A group with no VPC on EC2's record is not compared: there is nothing to compare it
/// against, and inventing a VPC for it would refuse a request for a value nobody sent.
pub fn vpc_mismatch<'a>(
    resolved: &'a [AccountSecurityGroup],
    placement_vpc: &str,
) -> Option<&'a str> {
    resolved
        .iter()
        .find(|group| !group.vpc_id.is_empty() && group.vpc_id != placement_vpc)
        .map(|group| group.vpc_id.as_str())
}

/// How ElastiCache words a request whose security groups are in another VPC than the cache
/// subnet group the resource is placed in: `Subnet group [<name>] belongs to a different VPC
/// [<subnet-group-vpc>] than [<security-group-vpc>]`.
///
/// The variable parts are the subnet group the resource is placed in, the VPC that group's
/// subnets belong to, and the VPC the security group belongs to — all three taken from the
/// request and from the records their owners answered with; the rest of the sentence is
/// fixed text, and it carries no trailing stop.
pub fn vpc_mismatch_message(
    cache_subnet_group_name: &str,
    cache_subnet_group_vpc: &str,
    security_group_vpc: &str,
) -> String {
    format!(
        "Subnet group [{cache_subnet_group_name}] belongs to a different VPC \
         [{cache_subnet_group_vpc}] than [{security_group_vpc}]"
    )
}
