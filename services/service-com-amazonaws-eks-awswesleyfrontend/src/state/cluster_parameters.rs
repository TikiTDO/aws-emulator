// SPDX-License-Identifier: CC0-1.0

//! The cluster-scope request parameters this service judges ITSELF, and the
//! sentence each refusal carries.
//!
//! RULE: Amazon EKS validates the parameters of a cluster-scope request against its
//! own rules — a cluster name's grammar, the page size a listing may ask for, the
//! Kubernetes versions it offers, the form of a security group id, and the two
//! endpoint-access switches — and refuses a violation with ITS OWN sentence for that
//! parameter, naming the member from the request and nothing else. The framework's
//! validating walk words a broken modelled constraint as one more constrained leaf
//! (`Value '<value>' at '<member>' failed to satisfy constraint: …`), which is a
//! different error contract from the one this API publishes, so the sentences live
//! here and every place that refuses one of these parameters — a handler, or the
//! pre-dispatch layer that owns the wording of a refusal the walk raises (see
//! [`crate::service::parameter_contract`]) — reads them from this one module.
//!
//! What is NOT here: the choice of error shape and status, which belongs to the
//! operation that answers (each one declares its own error channel), and the
//! `clusterName` member such a refusal carries, which is the value the request
//! supplied.

use std::net::IpAddr;
use std::time::SystemTime;

use super::{kubernetes_minor_version, ComAmazonawsEksAwswesleyfrontendState, SupportStatus};

/// The cluster-name grammar as Amazon EKS PRINTS it in a refusal: the modelled
/// `@pattern` of `ClusterName` ([`super::CLUSTER_NAME_PATTERN`]) without the escape
/// of the literal dash and without the end anchor, which is how the recorded service
/// renders it inside the sentence below.
const CLUSTER_NAME_GRAMMAR_AS_PRINTED: &str = "^[0-9A-Za-z][A-Za-z0-9-_]*";

/// The sentence a request that names no subnet for its control plane is refused with.
/// Entirely fixed text.
///
/// Amazon EKS "provisions elastic network interfaces in your VPC subnets", so a
/// control plane needs subnets to be provisioned in; this is the refusal both of the
/// ways a request can name none earn — omitting `resourcesVpcConfig` altogether, and
/// supplying one whose `subnetIds` is absent or empty.
pub const SUBNET_ID_REQUIRED_MESSAGE: &str = "Subnet Id is required";

/// The parameters whose required-refusal Amazon EKS words ITSELF, each with that
/// member's own sentence; every other member earns the general one below.
///
/// Recorded behaviour, one entry per member the corpus shows: the service states this
/// refusal in a sentence per parameter rather than from one template, so which sentence
/// a member earns is a fact about that member. `resourcesVpcConfig` is required for the
/// SUBNETS it carries, which is what the service reports missing when the whole member
/// is absent ([`SUBNET_ID_REQUIRED_MESSAGE`]).
const MEMBER_REQUIRED_SENTENCE: &[(&str, &str)] = &[
    ("roleArn", "The roleArn parameter is required"),
    ("resourcesVpcConfig", SUBNET_ID_REQUIRED_MESSAGE),
];

/// The sentence a request that omits a parameter this API requires is refused with.
///
/// Fixed text apart from the member's own name, so a caller is told WHICH parameter
/// it left out: "The parameter name is required." — or that member's own sentence,
/// for the members in [`MEMBER_REQUIRED_SENTENCE`].
pub fn parameter_required_message(member: &str) -> String {
    MEMBER_REQUIRED_SENTENCE
        .iter()
        .find(|(named, _)| *named == member)
        .map_or_else(
            || format!("The parameter {member} is required."),
            |(_, sentence)| (*sentence).to_string(),
        )
}

/// The sentence a parameter longer than its published maximum is refused with:
/// fixed text, the member's own name, and the maximum the model states for it.
pub fn parameter_too_long_message(member: &str, maximum: i64) -> String {
    format!("The parameter {member} should not be greater than {maximum} characters.")
}

/// The sentence a parameter that carries a value outside the cluster-name grammar is
/// refused with.
///
/// Fixed text apart from the member's own name: the grammar it states is the one
/// every cluster name in this API obeys ([`super::CLUSTER_NAME_PATTERN`]), and the
/// explanation after it spells that same grammar out in words. The offending value is
/// NOT echoed into the sentence — it is reported in the refusal's own `clusterName`
/// member instead.
pub fn parameter_invalid_characters_message(member: &str) -> String {
    format!(
        "The parameter {member} contains invalid characters. It should conform to the \
         regular expression \"{CLUSTER_NAME_GRAMMAR_AS_PRINTED}\"In other words, it should \
         start with a number or letter, and can contain numbers, letters, dashes, and \
         underscores."
    )
}

/// The smallest page a listing of this service may ask for, and the largest: every
/// `maxResults` member of this API is `@range {min: 1, max: 100}`
/// (`smithy_query(query="operation",operation="ListClusters,DescribeClusterVersions")`
/// reports `ListClustersRequestMaxResults` and `DescribeClusterVersionMaxResults` with
/// those bounds).
pub const MIN_LIST_MAX_RESULTS: i64 = 1;
/// See [`MIN_LIST_MAX_RESULTS`].
pub const MAX_LIST_MAX_RESULTS: i64 = 100;

/// The sentence a `maxResults` outside its bounds is refused with: fixed text whose
/// two numbers are the bounds the model declares for the member.
pub fn max_results_range_message() -> String {
    format!("maxResults needs to be between {MIN_LIST_MAX_RESULTS} and {MAX_LIST_MAX_RESULTS}")
}

/// The sentence a value outside the control-plane log types is refused with: the
/// rejected value from the request plus fixed text. It names the VALUE rather than the
/// member, which is why it holds wherever that member sits — a log type arrives inside
/// the `logging.clusterLogging[*].types` list.
pub fn unsupported_log_type_message(value: &str) -> String {
    format!("{value} is not a supported log type")
}

/// The sentence a member judged by an enum's value set is refused with when the value
/// is outside it: the member's own name and the values it accepts, in the order the
/// model declares them.
pub fn member_must_be_one_of_message(member: &str, accepted: &[String]) -> String {
    format!("{member} must be one of [{}]", accepted.join(", "))
}

/// The sentence the deprecated cluster-version `status` filter is refused with when
/// it carries a value outside `ClusterVersionStatus`: fixed text plus the rejected
/// value from the request.
pub fn cluster_version_status_message(value: &str) -> String {
    format!("Provided Cluster Status is invalid: {value}")
}

/// The sentence the `clusterVersions` filter is refused with when it does not name
/// Kubernetes versions. Entirely fixed text — it states the form the filter takes
/// rather than echoing what was sent.
pub const CLUSTER_VERSIONS_FILTER_MESSAGE: &str =
    "Provided Cluster Versions are invalid, it must be comma separated versions ie 1.27,1.28";

/// The sentence a `version` Amazon EKS does not offer a control plane for is refused
/// with: fixed text plus the version the request asked for.
pub fn unsupported_kubernetes_version_message(version: &str) -> String {
    format!("unsupported Kubernetes version {version}")
}

/// The sentence a `subnetIds` entry that is not a subnet id is refused with: fixed
/// text plus the offending id from the request.
pub fn invalid_subnet_message(subnet_id: &str) -> String {
    format!("The request contains an invalid subnet Id: {subnet_id}")
}

/// The sentence a `securityGroupIds` entry that is not a security group id is refused
/// with: fixed text plus the offending id from the request.
pub fn invalid_security_group_message(group_id: &str) -> String {
    format!("The request contains an invalid security group Id: {group_id}")
}

/// Whether `candidate` is an Amazon EC2 resource id of the kind `prefix` names.
///
/// EC2 identifies a resource as its kind's prefix followed by the resource's
/// hexadecimal id — eight digits for one created before the longer form, or seventeen
/// for one created since. A value outside that form names no resource of that kind in
/// any VPC, so a request carrying one is refused before anything is provisioned
/// against it and before EC2 is asked to resolve it.
fn is_ec2_resource_id(candidate: &str, prefix: &str) -> bool {
    let Some(body) = candidate.strip_prefix(prefix) else {
        return false;
    };
    (body.len() == 8 || body.len() == 17)
        && body
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte <= b'f'))
}

/// Whether `candidate` has the form of an Amazon EC2 security group id
/// ([`is_ec2_resource_id`]).
pub fn is_security_group_id(candidate: &str) -> bool {
    is_ec2_resource_id(candidate, "sg-")
}

/// Whether `candidate` has the form of an Amazon EC2 subnet id
/// ([`is_ec2_resource_id`]).
pub fn is_subnet_id(candidate: &str) -> bool {
    is_ec2_resource_id(candidate, "subnet-")
}

/// The sentence a `publicAccessCidrs` carrying values that are not CIDR blocks is
/// refused with: fixed text plus the offending values from the request, in the order
/// the request listed them, rendered as a list.
pub fn invalid_public_access_cidrs_message(invalid: &[String]) -> String {
    format!(
        "The following CIDRs are invalid in publicAccessCidrs: [{}]",
        invalid.join(", ")
    )
}

/// Whether `candidate` is a CIDR block: an IP address of either family and a prefix
/// length inside that family's width.
///
/// `publicAccessCidrs` is "The CIDR blocks that are allowed access to your cluster's
/// public Kubernetes API server endpoint" (`smithy_query(query="shape",
/// shape="VpcConfigRequest")`), and a value that is not one denies and allows nothing:
/// the address is parsed by the standard library rather than by a hand-rolled scan,
/// and the prefix is bounded by the address family it was parsed as — 32 bits for
/// IPv4, 128 for IPv6.
pub fn is_cidr_block(candidate: &str) -> bool {
    let Some((address, prefix)) = candidate.split_once('/') else {
        return false;
    };
    let Ok(address) = address.parse::<IpAddr>() else {
        return false;
    };
    // A prefix is written in decimal with no sign and no padding: `08` and `+8` name
    // no prefix length, which is what rejecting anything `u8` does not parse leaves.
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    let width = match address {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    prefix <= width
}

/// The entries of a `publicAccessCidrs` that are not CIDR blocks, in the order the
/// request listed them. Empty for a list this service accepts.
pub fn invalid_public_access_cidrs(requested: &[String]) -> Vec<String> {
    requested
        .iter()
        .filter(|cidr| !is_cidr_block(cidr))
        .cloned()
        .collect()
}

/// The sentence a request whose subnets cannot place a control plane in two
/// Availability Zones is refused with. Entirely fixed text.
///
/// `resourcesVpcConfig`'s own documentation states the rule — "You must specify at
/// least two subnets that are in different Availability Zones" — and fewer than two
/// subnets cannot be in two zones, so a request naming one (or none) earns this
/// refusal. The subnet minimum is waived for a local cluster on an Outpost, which is
/// created in a single subnet.
pub const SUBNETS_IN_TWO_AVAILABILITY_ZONES_MESSAGE: &str =
    "Subnets specified must be in at least two different AZs";

/// The sentence a request that turns BOTH paths to a cluster's Kubernetes API server
/// endpoint off is refused with. Entirely fixed text.
pub const ENDPOINT_ACCESS_BOTH_DISABLED_MESSAGE: &str =
    "Private and public endpoint access cannot be false";

/// Whether `candidate` names a Kubernetes minor version, which is the only form a
/// version of this API takes ([`super::kubernetes_minor_version`]): `major.minor`,
/// both numeric.
pub fn is_kubernetes_version(candidate: &str) -> bool {
    kubernetes_minor_version(candidate).is_some()
}

/// The versions in a `clusterVersions` filter, or `None` when the filter does not
/// name Kubernetes versions.
///
/// The filter is a `StringList` bound to the query string, and its own refusal states
/// the form it takes — "comma separated versions ie 1.27,1.28" — so an entry carrying
/// several comma-separated versions names all of them, and every version named has to
/// be one. An entry that is not a version (and an empty one) names none, which is
/// what `None` reports.
pub fn cluster_versions_filter(requested: &[String]) -> Option<Vec<String>> {
    let mut named = Vec::new();
    for entry in requested {
        for version in entry.split(',') {
            if !is_kubernetes_version(version) {
                return None;
            }
            named.push(version.to_string());
        }
    }
    Some(named)
}

/// Whether a cluster whose endpoint access is configured this way would be reachable
/// on NEITHER path.
///
/// The two switches have documented defaults — `endpointPublicAccess` "The default
/// value for this parameter is `true`" and `endpointPrivateAccess` "The default value
/// for this parameter is `false`" (`smithy_query(query="shape",
/// shape="VpcConfigRequest")`) — so a request that supplies neither leaves the public
/// path on. A cluster with both paths off has no Kubernetes API server endpoint any
/// caller could reach, which is what makes this combination a refusal rather than a
/// configuration.
pub fn endpoint_access_fully_disabled(public_access: Option<bool>, private_access: Option<bool>) -> bool {
    !public_access.unwrap_or(true) && !private_access.unwrap_or(false)
}

/// Whether `version` is a Kubernetes version Amazon EKS offers a control plane for at
/// `now`.
///
/// The versions it offers are the rows of the calendar this service publishes through
/// `DescribeClusterVersions` that are released and still inside standard or extended
/// support — the rows an UNFILTERED listing of that calendar reports, which is the set
/// a caller picks a version from. So the catalog a caller reads and the versions a
/// create accepts cannot disagree: a string that is not a row of it (a three-part
/// patch version, a minor version that was never published) and a row whose extended
/// support has ended both name no control plane this service can provision.
pub fn offers_cluster_version(
    store: &ComAmazonawsEksAwswesleyfrontendState,
    version: &str,
    now: SystemTime,
) -> bool {
    store.released_cluster_versions(now).any(|row| {
        row.cluster_version == version && row.support_status(now) != SupportStatus::Unsupported
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One kind of EC2 resource id, as the predicate that names one and its prefix.
    type Kind = (fn(&str) -> bool, &'static str);

    /// RULE: an EC2 resource id is its kind's prefix followed by EC2's hexadecimal id,
    /// in either published width; anything else names no resource of that kind.
    ///
    /// Source for the accepted rows: EC2's resource-id form, the kind's prefix plus 8
    /// or 17 hexadecimal digits.
    #[test]
    fn only_an_ec2_resource_id_is_one() {
        // (the predicate, its kind's prefix)
        let kinds: [Kind; 2] = [(is_security_group_id, "sg-"), (is_subnet_id, "subnet-")];

        for (names_one, prefix) in kinds {
            for body in [
                "12345678",
                "0123456789abcdef0",
                "ffffffff",
                "00000000000000000",
            ] {
                let accepted = format!("{prefix}{body}");
                assert!(
                    names_one(&accepted),
                    "case {accepted:?}: expected an EC2 resource id"
                );
            }

            // Rows differ along HOW the value misses the form: the prefix alone, a
            // width between the two published ones, non-hexadecimal digits, upper
            // case, and surrounding space.
            for body in ["", "1", "123456789", "xyzxyzxy", "ABCDEF12", " 12345678"] {
                let refused = format!("{prefix}{body}");
                assert!(
                    !names_one(&refused),
                    "case {refused:?}: expected no EC2 resource id"
                );
            }

            // And rows that miss the KIND: no prefix at all, and another kind's.
            for refused in ["not-an-id", "vpc-12345678", ""] {
                assert!(
                    !names_one(refused),
                    "case {refused:?}: expected no {prefix} resource id"
                );
            }
        }

        // A security group id is not a subnet id and the other way round: each kind is
        // named by its own prefix.
        assert!(
            !is_subnet_id("sg-12345678") && !is_security_group_id("subnet-12345678"),
            "a resource id of one kind names no resource of the other"
        );
    }

    /// RULE: the `clusterVersions` filter names Kubernetes minor versions, one entry
    /// or several comma-separated ones; anything else names none at all.
    #[test]
    fn the_cluster_versions_filter_names_kubernetes_versions() {
        // (case, the filter as sent, the versions it names)
        let accepted: Vec<(&str, Vec<String>, Vec<&str>)> = vec![
            ("an empty filter", vec![], vec![]),
            ("one version", vec!["1.33".to_string()], vec!["1.33"]),
            (
                "two entries",
                vec!["1.33".to_string(), "1.34".to_string()],
                vec!["1.33", "1.34"],
            ),
            (
                "one entry carrying both",
                vec!["1.27,1.28".to_string()],
                vec!["1.27", "1.28"],
            ),
        ];
        for (case, sent, expected) in accepted {
            assert_eq!(
                cluster_versions_filter(&sent).as_deref(),
                Some(
                    expected
                        .iter()
                        .map(|version| (*version).to_string())
                        .collect::<Vec<_>>()
                        .as_slice()
                ),
                "case {case:?}: expected the versions it names"
            );
        }

        // Rows differ along HOW the filter misses the form: a word, a major version
        // alone, a patch version, a prefixed version, an empty entry, and a trailing
        // comma.
        for refused in [
            vec!["not-a-version".to_string()],
            vec!["1".to_string()],
            vec!["1.33.0".to_string()],
            vec!["v1.33".to_string()],
            vec![String::new()],
            vec!["1.33,".to_string()],
            vec!["1.33".to_string(), "latest".to_string()],
        ] {
            assert!(
                cluster_versions_filter(&refused).is_none(),
                "case {refused:?}: expected a filter that names no versions"
            );
        }
    }

    /// RULE: a `publicAccessCidrs` entry is a CIDR block of either address family,
    /// and the refusal names every entry that is not one, in the request's order.
    #[test]
    fn only_a_cidr_block_of_either_family_is_one() {
        // The published defaults of the member itself are the first accepted rows:
        // "The default value is 0.0.0.0/0 and additionally ::/0 for dual-stack IPv6
        // clusters" (`smithy_query(query="shape", shape="VpcConfigRequest")`).
        for accepted in [
            "0.0.0.0/0",
            "::/0",
            "10.0.0.0/8",
            "192.168.1.1/32",
            "2001:db8::/32",
            "2001:db8::1/128",
        ] {
            assert!(
                is_cidr_block(accepted),
                "case {accepted:?}: expected a CIDR block"
            );
        }

        // Rows differ along HOW the value misses the form: no prefix, a prefix past
        // each family's width, a prefix that is not a number, no address, an address
        // that is not one, and a host name.
        for refused in [
            "10.0.0.0",
            "0.0.0.0/33",
            "2001:db8::/129",
            "10.0.0.0/eight",
            "/8",
            "10.0.0.256/8",
            "example.com/24",
            "",
        ] {
            assert!(
                !is_cidr_block(refused),
                "case {refused:?}: expected no CIDR block"
            );
        }

        let requested: Vec<String> = ["10.0.0.0/8", "0.0.0.0/33", "::/0", "nope"]
            .iter()
            .map(|cidr| (*cidr).to_string())
            .collect();
        let invalid = invalid_public_access_cidrs(&requested);
        assert_eq!(
            invalid,
            vec!["0.0.0.0/33".to_string(), "nope".to_string()],
            "expected only the entries that are not CIDR blocks, in order"
        );
        assert_eq!(
            invalid_public_access_cidrs_message(&invalid),
            "The following CIDRs are invalid in publicAccessCidrs: [0.0.0.0/33, nope]",
            "the sentence lists the offending entries"
        );
        assert!(
            invalid_public_access_cidrs(&["0.0.0.0/0".to_string()]).is_empty(),
            "a list of CIDR blocks offends nothing"
        );
    }

    /// RULE: a cluster is unreachable only when BOTH paths are off, and the two
    /// switches' documented defaults decide what an absent one is.
    #[test]
    fn both_endpoint_paths_off_is_the_only_unreachable_configuration() {
        // (case, endpointPublicAccess, endpointPrivateAccess, unreachable)
        let rows: [(&str, Option<bool>, Option<bool>, bool); 7] = [
            ("neither supplied", None, None, false),
            ("public off, private on", Some(false), Some(true), false),
            ("public off, private absent", Some(false), None, true),
            ("public off, private off", Some(false), Some(false), true),
            ("public on, private off", Some(true), Some(false), false),
            ("public on, private on", Some(true), Some(true), false),
            ("public absent, private off", None, Some(false), false),
        ];
        for (case, public_access, private_access, unreachable) in rows {
            assert_eq!(
                endpoint_access_fully_disabled(public_access, private_access),
                unreachable,
                "case {case:?}: expected unreachable={unreachable}"
            );
        }
    }

    /// RULE: the sentences are fixed text apart from the one value each names, and
    /// the bounds the page-size sentence states are the model's own.
    #[test]
    fn each_sentence_names_only_its_own_value() {
        for member in ["name", "clusterName"] {
            assert_eq!(
                parameter_required_message(member),
                format!("The parameter {member} is required."),
                "case {member:?}: the required sentence names the member"
            );
        }
        // The members the recorded corpus shows this service wording itself — rows
        // the issue did not cite.
        for (member, sentence) in MEMBER_REQUIRED_SENTENCE {
            assert_eq!(
                &parameter_required_message(member),
                sentence,
                "case {member:?}: expected this member's own required sentence"
            );
        }

        for member in ["name", "clusterName"] {
            let invalid = parameter_invalid_characters_message(member);
            assert!(
                invalid.starts_with(&format!("The parameter {member} contains invalid characters.")),
                "case {member:?}: expected the invalid-characters sentence, got {invalid:?}"
            );
            assert!(
                invalid.contains(CLUSTER_NAME_GRAMMAR_AS_PRINTED),
                "case {member:?}: expected the printed grammar, got {invalid:?}"
            );
        }

        for member in ["name", "clusterName"] {
            let maximum = super::super::MAX_CLUSTER_NAME_LEN as i64;
            assert_eq!(
                parameter_too_long_message(member, maximum),
                format!(
                    "The parameter {member} should not be greater than {maximum} characters."
                ),
                "case {member:?}: the length sentence names the member and the maximum"
            );
        }

        assert_eq!(
            max_results_range_message(),
            "maxResults needs to be between 1 and 100",
            "the page-size sentence states the modelled bounds"
        );

        for value in ["standard_support", "", "STANDARD-SUPPORT"] {
            assert_eq!(
                cluster_version_status_message(value),
                format!("Provided Cluster Status is invalid: {value}"),
                "case {value:?}: the status sentence echoes the rejected value"
            );
        }

        for version in ["1.33.0", "latest"] {
            assert_eq!(
                unsupported_kubernetes_version_message(version),
                format!("unsupported Kubernetes version {version}"),
                "case {version:?}: the version sentence echoes the request's version"
            );
        }

        for group_id in ["not-a-security-group-id", "sg-1"] {
            assert_eq!(
                invalid_security_group_message(group_id),
                format!("The request contains an invalid security group Id: {group_id}"),
                "case {group_id:?}: the security-group sentence echoes the offending id"
            );
        }

        for subnet_id in ["not-a-subnet-id", "subnet-1"] {
            assert_eq!(
                invalid_subnet_message(subnet_id),
                format!("The request contains an invalid subnet Id: {subnet_id}"),
                "case {subnet_id:?}: the subnet sentence echoes the offending id"
            );
        }
    }

    /// RULE: the versions a create accepts are the rows of the published calendar an
    /// unfiltered listing reports — released, and still inside standard or extended
    /// support.
    #[test]
    fn the_published_calendar_decides_which_versions_are_offered() {
        let store = ComAmazonawsEksAwswesleyfrontendState::default();
        let now = SystemTime::now();
        let offered: Vec<String> = store
            .released_cluster_versions(now)
            .filter(|row| row.support_status(now) != SupportStatus::Unsupported)
            .map(|row| row.cluster_version.clone())
            .collect();
        assert!(
            !offered.is_empty(),
            "setup: the calendar must offer at least one version"
        );
        for version in &offered {
            assert!(
                offers_cluster_version(&store, version, now),
                "case {version:?}: a version the catalog publishes is offered"
            );
        }
        // A row the clock has carried past the end of extended support is no longer
        // one Amazon EKS offers, so a create cannot land on it either.
        for version in store
            .released_cluster_versions(now)
            .filter(|row| row.support_status(now) == SupportStatus::Unsupported)
            .map(|row| row.cluster_version.clone())
            .collect::<Vec<_>>()
        {
            assert!(
                !offers_cluster_version(&store, &version, now),
                "case {version:?}: a version past extended support is not offered"
            );
        }
        // Rows differ along HOW the version misses the calendar: a patch version of a
        // published row, a minor version nothing published, a word, and the empty
        // string.
        for version in ["1.33.0", "9.99", "latest", ""] {
            assert!(
                !offers_cluster_version(&store, version, now),
                "case {version:?}: a version the calendar does not publish is not offered"
            );
        }
    }

    /// RULE: the cluster-name grammar decides which values a cluster-name member
    /// accepts, wherever it is enforced — including on a member the model leaves
    /// unconstrained.
    #[test]
    fn the_cluster_name_grammar_decides_which_names_are_accepted() {
        for accepted in ["a", "0", "probe-cluster", "probe_cluster", "Probe9"] {
            assert!(
                super::super::is_cluster_name(accepted),
                "case {accepted:?}: expected a cluster name"
            );
        }
        for refused in [
            "-leading-dash",
            "_leading-underscore",
            "has spaces",
            "has!punctuation",
            "arn:aws:eks:us-west-2:000000000000:cluster/probe",
            "",
        ] {
            assert!(
                !super::super::is_cluster_name(refused),
                "case {refused:?}: expected no cluster name"
            );
        }
    }
}
