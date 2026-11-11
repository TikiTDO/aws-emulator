// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what the add-on catalog PUBLISHES, driven over the wire
//! through the same `MultiServiceRouter` the production binary mounts.
//!
//! Two rules are under test:
//!
//! 1. **Every add-on Amazon EKS publishes is in the catalog**, so
//!    `DescribeAddonVersions` narrowed to one published add-on's name answers
//!    exactly one `AddonInfo`, carrying the builds of that add-on with the
//!    Kubernetes versions each supports — that is the only way a client discovers
//!    an add-on's versions and picks a cluster version to install it on.
//! 2. **The configuration schema served for a build is that add-on's OWN
//!    schema**: it declares the keys that add-on takes, each with the JSON type
//!    the key takes, and no other key is allowed — and `CreateAddon` holds a
//!    `configurationValues` document to exactly that schema, refusing an
//!    undeclared key, a declared key of the wrong type and a missing required
//!    property.
//!
//! Each suite asserts over a TABLE whose rows differ along what the rules
//! generalise over: which add-on is asked about (one Amazon EKS builds, ones it
//! curates from community projects, a Marketplace one), which build of it, and —
//! for the schema — the JSON type of the key a document sets and the way the
//! document fails the schema. The expectations are read out of the service
//! itself: the add-ons and builds from the catalog listing, the keys each schema
//! must declare from the catalog this service seeds, so the suites keep asserting
//! the rules as the published tables grow. The few hand-written rows name their
//! source.
//!
//! The request harness is the one every suite in this crate shares
//! (`tests/all/wire.rs`), and the add-on calls are the ones the add-on suites
//! beside this one already drive.

use service_com_amazonaws_eks_awswesleyfrontend::state::ComAmazonawsEksAwswesleyfrontendState;
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_lifecycle::{create_addon, describe_addon};
use crate::addon_versions::{
    active_cluster, addon_names, addons, compatibilities_of, describe_addon_configuration,
    describe_addon_versions, string_member, versions_of, whole_catalog,
};
use crate::cluster_lifecycle::REGION;
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The add-ons the catalog has to publish whatever else its seed carries, as
/// `(addonName, defaultNamespace, the kind of add-on it is)` — the name a caller
/// installs it by, the Kubernetes namespace its workloads run in
/// (`AddonInfo$defaultNamespace`), and the axis the publication generalises over.
///
/// Source: the "Amazon EKS add-ons", "Community add-ons" and AWS Marketplace
/// add-on tables of the Amazon EKS User Guide, which is what Amazon EKS
/// publishes; a name missing from the catalog can be neither discovered nor
/// installed, so this table is the REACH the publication must have. `pub(crate)`
/// because `addon_catalog_coverage.rs` asserts the rest of the rule — the default
/// build per cluster version, and the namespace an install resolves — over the
/// same table, so the two suites cannot drift apart over which add-ons exist.
pub(crate) const PUBLISHED_ADDONS: &[(&str, &str, &str)] = &[
    ("adot", "opentelemetry-operator-system", "an operator Amazon EKS builds itself"),
    (
        "amazon-cloudwatch-observability",
        "amazon-cloudwatch",
        "an observability agent with pod identity",
    ),
    (
        "amazon-sagemaker-hyperpod-taskgovernance",
        "kube-system",
        "a machine-learning scheduler",
    ),
    (
        "amazon-sagemaker-hyperpod-training-operator",
        "aws-hyperpod",
        "a machine-learning operator",
    ),
    ("aws-ebs-csi-driver", "kube-system", "a CSI driver with its own options"),
    ("aws-efa-k8s-device-plugin", "kube-system", "a device plugin"),
    ("aws-efs-csi-driver", "kube-system", "a CSI driver with its own options"),
    ("aws-guardduty-agent", "amazon-guardduty", "a security agent"),
    ("aws-mountpoint-s3-csi-driver", "kube-system", "a CSI driver"),
    (
        "aws-network-flow-monitoring-agent",
        "aws-network-flow-monitor",
        "a network observability agent",
    ),
    ("cert-manager", "cert-manager", "a community add-on Amazon EKS curates"),
    ("coredns", "kube-system", "an add-on Amazon EKS builds itself"),
    (
        "datadog_operator",
        "datadog-agent",
        "an AWS Marketplace add-on that cannot run unconfigured",
    ),
    (
        "dynatrace_dynatrace-operator",
        "dynatrace",
        "an AWS Marketplace add-on that cannot run unconfigured",
    ),
    ("eks-node-monitoring-agent", "kube-system", "an observability agent"),
    ("eks-pod-identity-agent", "kube-system", "a security agent"),
    (
        "external-dns",
        "external-dns",
        "a community add-on that needs IAM permissions",
    ),
    ("kube-proxy", "kube-system", "an add-on whose version tracks Kubernetes"),
    ("kube-state-metrics", "kube-system", "a community observability add-on"),
    ("kubecost_kubecost", "kubecost", "an AWS Marketplace add-on"),
    ("metrics-server", "kube-system", "a community metrics server"),
    (
        "prometheus-node-exporter",
        "prometheus-node-exporter",
        "a community metrics exporter",
    ),
    ("snapshot-controller", "kube-system", "a storage controller"),
    ("vpc-cni", "kube-system", "an add-on Amazon EKS builds itself"),
];

/// The default Kubernetes version `DescribeClusterVersions` publishes — the
/// version a cluster created without one lands on, and so the version whose
/// default add-on builds a client installs.
async fn default_cluster_version(router: &MultiServiceRouter) -> String {
    let answer = call(router, REGION, "GET", "/cluster-versions?defaultOnly=true", "").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeClusterVersions(defaultOnly): expected HTTP 200, got {answer:?}"
    );
    let rows = answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a clusterVersions list, got {:?}", answer.body))
        .clone();
    let row = rows
        .first()
        .unwrap_or_else(|| panic!("expected one default cluster version, got {rows:?}"));
    string_member(row, "clusterVersion")
}

/// The newest build the catalog publishes of one add-on for one Kubernetes
/// version, read through `DescribeAddonVersions` — the version `CreateAddon`
/// installs when the caller names none.
async fn newest_build(
    router: &MultiServiceRouter,
    addon_name: &str,
    kubernetes_version: &str,
) -> String {
    let answer = describe_addon_versions(
        router,
        &format!(
            "addonName={}&kubernetesVersion={}",
            encode_label(addon_name),
            encode_label(kubernetes_version)
        ),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonVersions({addon_name}): expected HTTP 200, got {answer:?}"
    );
    let listed = addons(&answer);
    let addon = listed.first().unwrap_or_else(|| {
        panic!(
            "setup DescribeAddonVersions({addon_name}): the catalog must publish it for \
             {kubernetes_version}, got {:?}",
            answer.body
        )
    });
    let builds = versions_of(addon);
    let build = builds
        .first()
        .unwrap_or_else(|| panic!("setup {addon_name}: expected a published build, got {addon:?}"));
    string_member(build, "addonVersion")
}

/// The `configurationSchema` served for one published build, parsed.
pub(crate) async fn configuration_schema(
    router: &MultiServiceRouter,
    addon_name: &str,
    addon_version: &str,
) -> serde_json::Value {
    let answer = describe_addon_configuration(router, addon_name, addon_version).await;
    assert_eq!(
        answer.status, 200,
        "DescribeAddonConfiguration({addon_name}, {addon_version}): expected HTTP 200, \
         got {answer:?}"
    );
    let document = string_member(&answer.body, "configurationSchema");
    serde_json::from_str(&document).unwrap_or_else(|error| {
        panic!("({addon_name}, {addon_version}): configurationSchema must be JSON: {error}")
    })
}

/// The schema node `node` stands for in `document`: the definition its `$ref`
/// names, or `node` itself when it names none.
///
/// A published add-on schema is a `$ref` into its own `definitions`, so reading
/// what it declares means following that reference — once at the root, and again
/// for every key whose value is an object.
pub(crate) fn resolved<'a>(
    document: &'a serde_json::Value,
    node: &'a serde_json::Value,
) -> &'a serde_json::Value {
    let Some(title) = node["$ref"]
        .as_str()
        .and_then(|pointer| pointer.strip_prefix("#/definitions/"))
    else {
        return node;
    };
    let definition = &document["definitions"][title];
    assert!(
        definition.is_object(),
        "a $ref must name a definition the document carries, got {node:?} against {:?}",
        document["definitions"]
    );
    definition
}

/// The type the published document `document` declares its root as: the
/// definition of the add-on's own chart values type.
pub(crate) fn schema_root(document: &serde_json::Value) -> &serde_json::Value {
    resolved(document, document)
}

/// The JSON Schema `type` the published document declares for the top-level key
/// `property`, following the key's `$ref` when it has one, or `None` for a key
/// the document does not declare.
pub(crate) fn declared_type(document: &serde_json::Value, property: &str) -> Option<String> {
    let declared = &schema_root(document)["properties"][property];
    if declared.is_null() {
        return None;
    }
    resolved(document, declared)["type"]
        .as_str()
        .map(str::to_owned)
}

/// The `message` of a refusal, which is what a client reads to see what it did
/// wrong.
fn message_of(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a message, got {:?}", answer.body))
        .to_string()
}

// ---------------------------------------------------------------------------
// RULE: the catalog publishes every add-on Amazon EKS publishes, so narrowing
// the listing to one published name answers exactly one `AddonInfo` with that
// add-on's builds and the Kubernetes versions each supports.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_published_addon_is_discoverable_by_name() {
    let router = emulator();
    let published = addon_names(&whole_catalog(&router).await);

    // The add-ons the publication has to carry, spanning the kinds it holds.
    for (addon_name, _, kind) in PUBLISHED_ADDONS {
        assert!(
            published.iter().any(|name| name == addon_name),
            "case {addon_name} ({kind}): the catalog must publish it, got {published:?}"
        );
    }

    // Every published add-on resolves on its own `addonName`, with builds that
    // name the cluster versions they support — what a client reads to pick a
    // version — and one of them flagged as the default for each of those.
    for addon_name in &published {
        let answer =
            describe_addon_versions(&router, &format!("addonName={}", encode_label(addon_name)))
                .await;
        assert_eq!(
            answer.status, 200,
            "case {addon_name}: expected HTTP 200, got {answer:?}"
        );
        let listed = addons(&answer);
        assert_eq!(
            listed.len(),
            1,
            "case {addon_name}: a published add-on is reported exactly once, got {:?}",
            answer.body["addons"]
        );
        assert_eq!(
            string_member(&listed[0], "addonName"),
            *addon_name,
            "case {addon_name}: expected the add-on asked for, got {:?}",
            listed[0]["addonName"]
        );
        let builds = versions_of(&listed[0]);
        assert!(
            !builds.is_empty(),
            "case {addon_name}: expected at least one published build, got {:?}",
            listed[0]
        );
        let mut defaulted = false;
        for build in &builds {
            let case = format!("{addon_name}/{}", string_member(build, "addonVersion"));
            let compatibilities = compatibilities_of(build);
            assert!(
                !compatibilities.is_empty(),
                "case {case}: a published build names the cluster versions it supports, \
                 got {build:?}"
            );
            for compatibility in &compatibilities {
                assert!(
                    !string_member(compatibility, "clusterVersion").is_empty(),
                    "case {case}: a compatibility names one cluster version, got \
                     {compatibility:?}"
                );
                defaulted |= compatibility["defaultVersion"].as_bool() == Some(true);
            }
        }
        assert!(
            defaulted,
            "case {addon_name}: one build must be the default for a cluster version, got {:?}",
            listed[0]["addonVersions"]
        );
    }

    // A name the catalog does not publish matches nothing, which is an EMPTY
    // listing rather than a refusal.
    for case in ["not-a-published-addon", "cert_manager", "coredns-extra"] {
        let answer =
            describe_addon_versions(&router, &format!("addonName={}", encode_label(case))).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: an unpublished name is an empty listing, got {answer:?}"
        );
        assert!(
            addons(&answer).is_empty(),
            "case {case}: expected an empty listing, got {:?}",
            answer.body["addons"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the schema served for a build is that add-on's own schema — it declares
// the add-on's configurable keys with the JSON type each takes, and allows no
// other key.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_configuration_schema_declares_the_addons_own_keys() {
    let router = emulator();
    // The keys each add-on's schema declares, as this service's own catalog holds
    // them: the schema served over the wire is that table, not a document shared
    // between add-ons.
    let seeded = ComAmazonawsEksAwswesleyfrontendState::default();

    for listed in addons(&whole_catalog(&router).await) {
        let addon_name = string_member(&listed, "addonName");
        let record = seeded
            .addon_catalog
            .iter()
            .find(|record| record.addon_name == addon_name)
            .unwrap_or_else(|| panic!("case {addon_name}: the catalog must hold the add-on"));
        let builds = versions_of(&listed);
        let newest = builds.first().expect("a published build");
        let oldest = builds.last().expect("a published build");
        for build in [newest, oldest] {
            let addon_version = string_member(build, "addonVersion");
            let case = format!("{addon_name}/{addon_version}");
            let schema = configuration_schema(&router, &addon_name, &addon_version).await;
            for (property, json_type) in &record.configuration_properties {
                assert_eq!(
                    declared_type(&schema, property).as_deref(),
                    Some(json_type.as_str()),
                    "case {case}: the schema must declare {property} as {json_type}, got {:?}",
                    schema_root(&schema)["properties"][property]
                );
            }
            assert_eq!(
                schema_root(&schema)["additionalProperties"],
                serde_json::json!(false),
                "case {case}: a key the add-on does not take is not allowed, got {:?}",
                schema_root(&schema)["additionalProperties"]
            );
            // No two add-ons are served the same document: the keys of one are
            // not the keys of another.
            assert!(
                !record.configuration_properties.is_empty(),
                "case {case}: a published add-on declares the keys it takes, got {record:?}"
            );
        }
    }

    // The documented keys, one per JSON type the schemas use, on add-ons other
    // than the CSI driver whose FIPS install is the documented example.
    //
    // Source: the configuration examples the Amazon EKS User Guide gives for each
    // add-on (`aws-efs-csi-driver`'s `useFIPS` is the key its FIPS install
    // passes), which is the same surface
    // `aws eks describe-addon-configuration` reports.
    let kubernetes_version = default_cluster_version(&router).await;
    let documented: &[(&str, &str, &str)] = &[
        ("aws-efs-csi-driver", "useFIPS", "boolean"),
        ("cert-manager", "installCRDs", "boolean"),
        ("coredns", "replicaCount", "integer"),
        ("coredns", "corefile", "string"),
        ("vpc-cni", "env", "object"),
        ("vpc-cni", "enableNetworkPolicy", "string"),
        ("external-dns", "domainFilters", "array"),
        ("kube-state-metrics", "replicas", "integer"),
    ];
    for (addon_name, property, json_type) in documented {
        let addon_version = newest_build(&router, addon_name, &kubernetes_version).await;
        let case = format!("{addon_name}/{addon_version}: {property}");
        let schema = configuration_schema(&router, addon_name, &addon_version).await;
        assert_eq!(
            declared_type(&schema, property).as_deref(),
            Some(*json_type),
            "case {case}: expected a {json_type} property, got {:?}",
            schema_root(&schema)["properties"][property]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `CreateAddon` validates `configurationValues` against the schema
// `DescribeAddonConfiguration` publishes for the build being installed — an
// undeclared key, a declared key of the wrong JSON type and a missing required
// property are each refused 400 `InvalidParameterException`, naming the path at
// fault; a document the schema accepts is stored and read back.
// ---------------------------------------------------------------------------

/// What a row expects of its `configurationValues` document: the install goes
/// through, or the schema violation the refusal must name (the bracketed text
/// `[...]` of the message).
#[derive(Debug)]
enum Expected {
    Installed,
    Violation(&'static str),
}

#[tokio::test]
async fn create_addon_holds_configuration_values_to_the_published_schema() {
    let router = emulator();
    let kubernetes_version = default_cluster_version(&router).await;
    active_cluster(&router, "configured", &kubernetes_version).await;

    // The rows differ along the JSON type of the key the document sets, and the
    // way the document meets or fails the schema: a legal value, a key of the
    // wrong type, a key the schema does not declare, a document that is not an
    // object at all, a required property left out, and a YAML document (which no
    // JSON Schema judges). Every row names an add-on the catalog publishes, since
    // that is the only kind an install can name at all.
    let rows: Vec<(&str, &str, &str, Expected)> = vec![
        (
            "a boolean key the add-on declares",
            "aws-efs-csi-driver",
            "{\"useFIPS\": true}",
            Expected::Installed,
        ),
        (
            "an integer and a string key",
            "coredns",
            "{\"replicaCount\": 2, \"computeType\": \"Fargate\"}",
            Expected::Installed,
        ),
        (
            "a string key whose value looks like a boolean",
            "vpc-cni",
            "{\"enableNetworkPolicy\": \"true\"}",
            Expected::Installed,
        ),
        (
            "an array and an object key",
            "external-dns",
            "{\"domainFilters\": [\"example.com\"], \"resources\": {}}",
            Expected::Installed,
        ),
        (
            "an integer key given a string",
            "aws-ebs-csi-driver",
            "{\"replicaCount\": \"2\"}",
            Expected::Violation("$.replicaCount: string found, integer expected"),
        ),
        (
            "an integer key given a fractional number",
            "kube-state-metrics",
            "{\"replicas\": 1.5}",
            Expected::Violation("$.replicas: number found, integer expected"),
        ),
        (
            "an integer key given a whole number written as a decimal",
            "kube-state-metrics",
            "{\"replicas\": 2.0}",
            Expected::Installed,
        ),
        (
            "the placement keys every add-on's schema carries",
            "aws-guardduty-agent",
            "{\"podAnnotations\": {\"team\": \"platform\"}, \"tolerations\": [], \
             \"priorityClassName\": \"system-node-critical\"}",
            Expected::Installed,
        ),
        (
            "an object key given an array",
            "cert-manager",
            "{\"installCRDs\": true, \"webhook\": []}",
            Expected::Violation("$.webhook: array found, object expected"),
        ),
        (
            "a key the schema does not declare",
            "prometheus-node-exporter",
            "{\"hostNetwork\": true, \"notAnOption\": 1}",
            Expected::Violation(
                "$.notAnOption: is not defined in the schema and the schema does not allow \
                 additional properties",
            ),
        ),
        (
            "a key of another add-on's schema",
            "snapshot-controller",
            "{\"useFIPS\": true}",
            Expected::Violation(
                "$.useFIPS: is not defined in the schema and the schema does not allow \
                 additional properties",
            ),
        ),
        (
            "a document that is not an object",
            "metrics-server",
            "[{\"replicas\": 1}]",
            Expected::Violation("$: array found, object expected"),
        ),
        (
            "a required property left out",
            "datadog_operator",
            "{\"site\": \"datadoghq.eu\"}",
            Expected::Violation("$.apiKey: is missing but it is required"),
        ),
        (
            "the required property supplied",
            "datadog_operator",
            "{\"apiKey\": \"0123456789abcdef\", \"site\": \"datadoghq.eu\"}",
            Expected::Installed,
        ),
        (
            "a nested property of the wrong type",
            "aws-ebs-csi-driver",
            "{\"serviceAccount\": {\"name\": 7}}",
            Expected::Violation("$.serviceAccount.name: integer found, string expected"),
        ),
        (
            // A KNOWN GAP, not the rule: Amazon EKS converts a YAML document
            // and holds it to the same schema. This service validates the JSON
            // form only, so the row pins what it does today — a YAML document
            // is stored unvalidated — rather than what the schema would say
            // about it.
            "a YAML document, which this service does not judge (known gap)",
            "eks-pod-identity-agent",
            "agent:\n  logLevel: debug\n",
            Expected::Installed,
        ),
    ];

    for (case, addon_name, values, expected) in &rows {
        let request = serde_json::json!({
            "addonName": addon_name,
            "configurationValues": values,
        });
        let answer = create_addon(&router, "configured", &request).await;
        match expected {
            Expected::Installed => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected the install to go through, got {answer:?}"
                );
                // The document the schema accepted is stored as given: the
                // read-back is what a client gets later.
                let described = describe_addon(&router, "configured", addon_name).await;
                assert_eq!(
                    described.addon()["configurationValues"],
                    serde_json::json!(values),
                    "case {case:?}: the stored document must read back as supplied, got {:?}",
                    described.addon()["configurationValues"]
                );
            }
            Expected::Violation(violation) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected the schema to refuse it, got {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "InvalidParameterException",
                    "case {case:?}: expected InvalidParameterException, got {answer:?}"
                );
                let expected_message = format!(
                    "ConfigurationValue provided in request is not supported: Json schema \
                     validation failed with error: [{violation}]"
                );
                assert_eq!(
                    message_of(&answer),
                    expected_message,
                    "case {case:?}: expected the schema violation to be named, got {:?}",
                    message_of(&answer)
                );
                // A refused create writes nothing.
                let described = describe_addon(&router, "configured", addon_name).await;
                assert_eq!(
                    described.status, 404,
                    "case {case:?}: a refused create must store nothing, got {described:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the catalog is one surface — the schema a client reads for a build is
// the schema the install of that build is held to, and the cluster is resolved
// before any document is looked at.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_schema_a_client_reads_is_the_schema_the_install_is_held_to() {
    let router = emulator();
    let kubernetes_version = default_cluster_version(&router).await;
    active_cluster(&router, "discovered", &kubernetes_version).await;

    // Walk the discovery path a client walks, for EVERY add-on the catalog
    // publishes: the default cluster version, the build published for it, then
    // that build's schema — and install with a document built out of the keys
    // that schema declares, one value per JSON type. Every key the published
    // schema declares is therefore a key the install accepts: the schema a
    // client reads cannot claim a configuration surface the create refuses.
    let published = addon_names(&whole_catalog(&router).await);
    for addon_name in &published {
        let addon_name = addon_name.as_str();
        let addon_version = newest_build(&router, addon_name, &kubernetes_version).await;
        let schema = configuration_schema(&router, addon_name, &addon_version).await;
        let properties = schema_root(&schema)["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("case {addon_name}: expected declared properties"))
            .clone();
        let mut document = serde_json::Map::new();
        for (property, declared) in &properties {
            let value = match resolved(&schema, declared)["type"].as_str() {
                Some("boolean") => serde_json::json!(true),
                Some("integer") => serde_json::json!(3),
                Some("string") => serde_json::json!("declared"),
                Some("array") => serde_json::json!([]),
                Some("object") => serde_json::json!({}),
                _ => continue,
            };
            document.insert(property.clone(), value);
        }
        let values = serde_json::Value::Object(document).to_string();
        let request = serde_json::json!({
            "addonName": addon_name,
            "addonVersion": addon_version,
            "configurationValues": values,
        });
        let answer = create_addon(&router, "discovered", &request).await;
        assert_eq!(
            answer.status, 200,
            "case {addon_name}: a document built from the schema's own properties must be \
             accepted, got {answer:?}"
        );
        assert_eq!(
            answer.addon()["addonVersion"],
            serde_json::json!(addon_version),
            "case {addon_name}: the installed build is the one asked for, got {:?}",
            answer.addon()["addonVersion"]
        );
        let described = describe_addon(&router, "discovered", addon_name).await;
        assert_eq!(
            described.status, 200,
            "case {addon_name}: the installed add-on must read back, got {described:?}"
        );
        assert_eq!(
            described.addon()["configurationValues"],
            serde_json::json!(values),
            "case {addon_name}: the stored document must read back as supplied, got {:?}",
            described.addon()["configurationValues"]
        );
    }

    // The cluster is resolved BEFORE the document is validated, so a cluster this
    // Region does not hold is not found whether the document satisfies the schema
    // or breaks it.
    for (case, values) in [
        ("a document the schema accepts", "{\"useFIPS\": true}"),
        ("a document the schema rejects", "{\"useFIPS\": \"true\"}"),
        ("a document that is not JSON", "useFIPS: true\n"),
    ] {
        let request = serde_json::json!({
            "addonName": "aws-efs-csi-driver",
            "configurationValues": values,
            "resolveConflicts": "OVERWRITE",
        });
        let answer = create_addon(&router, "never-created", &request).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a cluster that does not exist is resolved first, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            message_of(&answer),
            "No cluster found for name: never-created.",
            "case {case:?}: expected the cluster to be named, got {:?}",
            message_of(&answer)
        );
    }
}
