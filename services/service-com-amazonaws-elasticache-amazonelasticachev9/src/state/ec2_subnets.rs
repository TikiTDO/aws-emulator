// SPDX-License-Identifier: CC0-1.0

//! The subnets the ACCOUNT has, as the service that owns them reports them.
//!
//! A subnet is not a resource of this service: it is Amazon EC2's. A client builds its
//! network with `ec2:CreateVpc`/`ec2:CreateSubnet`, or uses the default VPC EC2
//! materialises for every Region of the account, and then names those subnets in
//! `CreateCacheSubnetGroup$SubnetIds` or `CreateServerlessCache$SubnetIds`. So EC2 is the
//! ONE answer to "which subnets does the account have", and an identifier a request names
//! is resolved there and nowhere else — reached the way any emulated service reaches
//! another, through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
//! service's crate and nothing leaves the process.
//!
//! The VPC and the Availability Zone a cache subnet group reports come from the record EC2
//! answered with, never from anything built here, so the placement this service reports is
//! the network the account really has. An identifier EC2 reports no subnet for resolves to
//! nothing, and that is the only case a create is refused for.
//!
//! Nothing is cached: a subnet created a moment ago resolves at once, and one that was
//! deleted stops resolving at once.

use ::std::collections::{BTreeMap, HashMap, HashSet};

use super::AccountSubnet;

/// The `DescribeSubnets` filter that selects subnets BY ID (`subnet-id`, listed by that
/// operation's own `Filters` documentation).
///
/// A filter is what asks the question rather than `SubnetIds`: an identifier the account
/// has no subnet for is then an absent row instead of a failure of the whole call, so every
/// identifier the request named is still resolved against what EC2 does hold.
const SUBNET_ID_FILTER: &str = "subnet-id";

/// The `DescribeSubnets` filter that selects the DEFAULT subnet of each Availability Zone
/// (`default-for-az`, listed by that operation's own `Filters` documentation): "Indicates
/// whether this is the default subnet for the Availability Zone".
///
/// Those are the subnets a request that names none is placed in, because the member
/// publishes that selection as being made "across AZs in your default VPC" and a default
/// VPC's subnets are exactly its per-zone default subnets.
const DEFAULT_FOR_AZ_FILTER: &str = "default-for-az";

/// The value of a true-valued EC2 boolean filter.
const FILTER_TRUE: &str = "true";

/// The most subnet identifiers ONE hop asks about, as values of one `subnet-id` filter.
///
/// The owner evaluates a filter against every subnet the account stores, so the cost of a
/// hop is (identifiers asked x subnets stored): this caps the first factor, so one hop
/// costs a constant times what the account stores rather than something the request
/// chooses. Both callers refuse a request naming more than [`super::SUBNETS_PER_GROUP`]
/// subnets BEFORE they resolve any of them, and that bound is below this one, so one
/// request costs ONE hop whatever its body carries.
const SUBNET_IDS_PER_HOP: usize = 64;

/// The most pages of one `DescribeSubnets` answer this reads.
///
/// EC2 pages its answer and reports a `NextToken` while subnets of the selection remain, so
/// a selection larger than one page is followed rather than silently truncated — otherwise a
/// subnet the account HAS would resolve to nothing because of where it landed. The cap keeps
/// the hop finite whatever the owner holds; at the default page size of 1000 it is already
/// well past the owner's own ceiling on stored subnets.
const SUBNET_PAGES_PER_HOP: usize = 8;

/// Every subnet of `region` that matches the filter `(name, values)`, as EC2 reports it,
/// across up to [`SUBNET_PAGES_PER_HOP`] pages of its answer.
///
/// FAIL-CLOSED: a hop that could not be made at all contributes nothing, so the
/// identifiers it would have answered for resolve to nothing and the create is refused.
/// Nothing about a subnet is invented here, and a subnet that cannot be shown to be the
/// account's is not one a resource is placed in — these operations PLACE the resource in
/// those subnets and report their VPC and zone, so with no record there is nothing to place
/// it in or to report.
async fn described(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    values: Vec<String>,
) -> Vec<AccountSubnet> {
    let Some(client) = super::ec2_client::ec2(ctx, region) else {
        ::tracing::warn!("the request names no Region label, so EC2 cannot be asked");
        return Vec::new();
    };
    let mut reported: Vec<AccountSubnet> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..SUBNET_PAGES_PER_HOP {
        let filter = ::aws_sdk_ec2::types::Filter::builder()
            .name(name)
            .set_values(Some(values.clone()))
            .build();
        match client
            .describe_subnets()
            .filters(filter)
            .set_next_token(next_token.take())
            .send()
            .await
        {
            Ok(output) => {
                reported.extend(output.subnets().iter().filter_map(|subnet| {
                    Some(AccountSubnet {
                        subnet_id: subnet.subnet_id()?.to_string(),
                        // EC2 reports the VPC a subnet belongs to and the zone it sits in; a
                        // record missing either carries what this service can report of it,
                        // and nothing is invented for it here.
                        vpc_id: subnet.vpc_id().unwrap_or_default().to_string(),
                        availability_zone: subnet
                            .availability_zone()
                            .unwrap_or_default()
                            .to_string(),
                    })
                }));
                match output.next_token() {
                    Some(token) => next_token = Some(token.to_string()),
                    // The whole selection has been reported.
                    None => break,
                }
            }
            Err(error) => {
                // At `warn`: a create cannot be placed when the owner of subnets cannot be
                // asked, so a deployment where this hop fails is one where no cache subnet
                // group can be created at all — worth seeing, and it IS why an identifier did
                // not resolve.
                ::tracing::warn!(
                    %region,
                    filter = name,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the subnets a create named could not be resolved against EC2"
                );
                break;
            }
        }
    }
    reported
}

/// Every subnet identifier a request names, resolved against the subnets the account has in
/// `region`, in the order the request listed them.
///
/// `Err` carries the identifiers that name NO subnet of the account, in that same order, so
/// the refusal names exactly what was wrong with the request and nothing is stored.
///
/// ONE source — the service that owns subnets (see this module's own documentation) — asked
/// about every identifier the request named at once, deduplicated, so the work does not
/// grow with how many times one subnet is named, and bounded by [`SUBNET_IDS_PER_HOP`]
/// identifiers per hop.
pub async fn resolve_requested_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[&str],
) -> Result<Vec<AccountSubnet>, Vec<String>> {
    let mut distinct: Vec<&str> = Vec::with_capacity(subnet_ids.len());
    let mut seen: HashSet<&str> = HashSet::new();
    for subnet_id in subnet_ids {
        if seen.insert(subnet_id) {
            distinct.push(subnet_id);
        }
    }

    let mut held: HashMap<String, AccountSubnet> = HashMap::new();
    for chunk in distinct.chunks(SUBNET_IDS_PER_HOP) {
        let values = chunk.iter().map(|id| (*id).to_string()).collect();
        for subnet in described(ctx, region, SUBNET_ID_FILTER, values).await {
            held.insert(subnet.subnet_id.clone(), subnet);
        }
    }

    let mut resolved = Vec::with_capacity(subnet_ids.len());
    let mut unresolved: Vec<String> = Vec::new();
    for subnet_id in subnet_ids {
        match held.get(*subnet_id) {
            Some(subnet) => resolved.push(subnet.clone()),
            None => unresolved.push((*subnet_id).to_string()),
        }
    }
    if unresolved.is_empty() {
        Ok(resolved)
    } else {
        Err(unresolved)
    }
}

/// The subnets ElastiCache SELECTS for a resource in `region` whose request named none: the
/// first `count` of the account's default subnets, one per Availability Zone, in zone
/// order.
///
/// They are the account's own subnets as EC2 reports them, so what this service selects is
/// always something a request could have named itself — and a Region where the account has
/// fewer than `count` default subnets yields the ones it has rather than an invented id.
pub async fn default_subnets(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    count: usize,
) -> Vec<AccountSubnet> {
    // One per zone, in zone order: a default VPC has one default subnet per Availability
    // Zone, and keying by zone is what makes the selection the documented "across AZs"
    // rather than several subnets of one zone.
    let by_zone: BTreeMap<String, AccountSubnet> = described(
        ctx,
        region,
        DEFAULT_FOR_AZ_FILTER,
        vec![FILTER_TRUE.to_string()],
    )
    .await
    .into_iter()
    .map(|subnet| (subnet.availability_zone.clone(), subnet))
    .collect();
    by_zone.into_values().take(count).collect()
}
