// SPDX-License-Identifier: CC0-1.0

//! The application NAMES the Amazon EMR release catalog reports.
//!
//! THE RULE these cases assert: a reference-data name is an identifier, so every
//! `Applications[].Name` `DescribeReleaseLabel` reports is the single spelling Amazon
//! EMR publishes for that application — the spelling `RunJobFlow`'s `Application$Name`
//! accepts and `DescribeCluster` reports back — reported verbatim and never case-folded,
//! while a caller's `ReleaseLabelFilter$Application`, whose doc example is lower case,
//! keeps matching it without regard to case.
//!
//! Every case drives the real HTTP path through the shared [`Emr`] harness, and the
//! expectations are read back out of the service through its own operations, so what
//! these tests pin is the rule rather than one recorded answer.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, Emr};

/// Every application identifier the release catalog can report, written out by hand from
/// the Amazon EMR Release Guide's "Application versions" tables (the same source
/// `state::APPLICATION_CATALOG` records).
///
/// Several of these capitalize INSIDE the word (`HBase`, `HCatalog`, `MXNet`,
/// `JupyterEnterpriseGateway`, `JupyterHub`, `TensorFlow`, `ZooKeeper`), so no one case
/// transform of a lower-cased name produces this list: the spelling has to be stored per
/// application.
const PUBLISHED_APPLICATION_NAMES: &[&str] = &[
    "Flink",
    "Ganglia",
    "HBase",
    "HCatalog",
    "Hadoop",
    "Hive",
    "Hudi",
    "Hue",
    "Iceberg",
    "JupyterEnterpriseGateway",
    "JupyterHub",
    "Livy",
    "MXNet",
    "Oozie",
    "Phoenix",
    "Pig",
    "Presto",
    "Spark",
    "Sqoop",
    "TensorFlow",
    "Tez",
    "Trino",
    "Zeppelin",
    "ZooKeeper",
];

/// One row of the per-application spelling rule: a release label, identifiers that
/// release must report exactly, and spellings of those same applications it must never
/// report.
#[derive(Debug)]
struct SpellingRow {
    label: &'static str,
    exactly: &'static [&'static str],
    never: &'static [&'static str],
}

/// Rows spanning the three Amazon EMR major lines the catalog publishes, each checked on
/// the applications whose identifier a case transform would get wrong, plus applications
/// (`Livy`, `Pig`, `Tez`, `Flink`) a transform would get right — the rule holds for both
/// kinds. The forbidden spellings are the ones the wrong rules produce: the all-lower
/// name, and the capitalize-first-letter-only transform of it.
const SPELLING_ROWS: &[SpellingRow] = &[
    SpellingRow {
        label: "emr-5.33.1",
        exactly: &["HBase", "ZooKeeper", "JupyterHub", "Livy", "Pig", "Tez"],
        never: &["hbase", "Hbase", "zookeeper", "Zookeeper", "jupyterhub", "Jupyterhub", "livy"],
    },
    SpellingRow {
        label: "emr-6.7.0",
        exactly: &["HBase", "ZooKeeper", "JupyterEnterpriseGateway", "JupyterHub", "Flink"],
        never: &[
            "hbase",
            "Hbase",
            "zookeeper",
            "Zookeeper",
            "jupyterenterprisegateway",
            "Jupyterenterprisegateway",
            "jupyterhub",
            "flink",
        ],
    },
    SpellingRow {
        label: "emr-7.5.0",
        exactly: &["HBase", "ZooKeeper", "JupyterEnterpriseGateway", "Spark", "Hadoop", "Trino"],
        never: &["hbase", "Hbase", "zookeeper", "Zookeeper", "spark", "hadoop", "trino"],
    },
];

/// The `(Name, Version)` pairs `DescribeReleaseLabel` reports for `label`.
async fn applications(emr: &Emr, label: &str) -> Vec<(String, String)> {
    let payload = emr
        .ok("DescribeReleaseLabel", json!({ "ReleaseLabel": label }))
        .await;
    at(&payload, &["Applications"])
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {label:?}: DescribeReleaseLabel must report Applications, got {payload}")
        })
        .iter()
        .map(|application| {
            (
                at(application, &["Name"]).as_str().unwrap_or_default().to_string(),
                at(application, &["Version"]).as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The names `DescribeReleaseLabel` reports for `label`.
async fn application_names(emr: &Emr, label: &str) -> Vec<String> {
    applications(emr, label)
        .await
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// Every release label the service publishes, read through `ListReleaseLabels` and its
/// pages rather than from a copy of the catalog.
async fn published_labels(emr: &Emr, request: Value) -> Vec<String> {
    let mut labels: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = request.clone();
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let payload = emr.ok("ListReleaseLabels", body).await;
        let page = at(&payload, &["ReleaseLabels"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("ListReleaseLabels must answer a ReleaseLabels list, got {payload}")
            });
        labels.extend(
            page.iter()
                .map(|label| label.as_str().unwrap_or_default().to_string()),
        );
        assert!(
            labels.len() <= 10_000,
            "the listing must terminate, got {} labels",
            labels.len()
        );
        match at(&payload, &["NextToken"]).as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return labels,
        }
    }
}

/// Across the whole published catalog, every reported name is one of the identifiers
/// Amazon EMR publishes — spelled exactly — and the catalog reports all of them.
#[tokio::test]
async fn every_reported_application_name_is_the_published_identifier() {
    let emr = Emr::new();
    let labels = published_labels(&emr, json!({})).await;
    assert!(
        labels.len() > 10,
        "the catalog must publish the Amazon EMR releases, got {labels:?}"
    );

    let mut seen: Vec<String> = Vec::new();
    for label in &labels {
        let reported = applications(&emr, label).await;
        assert!(
            !reported.is_empty(),
            "case {label:?}: an Amazon EMR release bundles applications, got {reported:?}"
        );
        for (name, version) in &reported {
            assert!(
                PUBLISHED_APPLICATION_NAMES.contains(&name.as_str()),
                "case {label:?}: {name:?} is not an application identifier Amazon EMR \
                 publishes; expected one of {PUBLISHED_APPLICATION_NAMES:?}"
            );
            // An identifier is reported as published, so lower-casing it is a change:
            // every published name carries at least one upper-case letter.
            assert_ne!(
                name,
                &name.to_lowercase(),
                "case {label:?}: the identifier must be reported as published, \
                 got the case-folded {name:?} with version {version:?}"
            );
            if !seen.contains(name) {
                seen.push(name.clone());
            }
        }
    }

    seen.sort();
    let mut expected: Vec<String> = PUBLISHED_APPLICATION_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    expected.sort();
    assert_eq!(
        seen, expected,
        "case \"whole catalog\": every published identifier must appear somewhere in the \
         catalog and nothing else may, got {seen:?}"
    );
}

/// The spelling is stored per application, not derived: an identifier that capitalizes
/// inside the word is reported that way, and no case transform of it is ever reported.
#[tokio::test]
async fn an_identifier_is_spelled_per_application_and_not_transformed() {
    let emr = Emr::new();
    for SpellingRow {
        label,
        exactly,
        never,
    } in SPELLING_ROWS
    {
        let reported = application_names(&emr, label).await;
        for name in *exactly {
            assert!(
                reported.iter().any(|found| found == name),
                "case {label:?}: expected the identifier {name:?}, got {reported:?}"
            );
        }
        for name in *never {
            assert!(
                !reported.iter().any(|found| found == name),
                "case {label:?}: {name:?} is not how Amazon EMR spells that application, \
                 got {reported:?}"
            );
        }
    }
}

/// What the catalog reports is what `RunJobFlow` takes: the identifiers read off a
/// release go straight back in as `Applications`, and `DescribeCluster` reports the same
/// spellings back.
#[tokio::test]
async fn the_reported_identifiers_are_what_run_job_flow_accepts() {
    let emr = Emr::new();
    for label in ["emr-5.36.2", "emr-6.15.0", "emr-7.5.0"] {
        let reported = application_names(&emr, label).await;
        assert!(
            reported.len() > 5,
            "case {label:?}: the release bundles applications to launch, got {reported:?}"
        );
        let requested: Vec<Value> = reported
            .iter()
            .map(|name| json!({ "Name": name }))
            .collect();
        let cluster_id = emr
            .run_job_flow(json!({
                "Name": format!("catalog-{label}"),
                "ReleaseLabel": label,
                "Applications": requested,
                "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE, "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
                "Instances": {
                    "InstanceCount": 1,
                    "MasterInstanceType": "m5.xlarge",
                    "KeepJobFlowAliveWhenNoSteps": true,
                },
            }))
            .await;
        let cluster = emr.describe_cluster(&cluster_id).await;
        let echoed: Vec<String> = at(&cluster, &["Applications"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {label:?}: DescribeCluster must report Applications, got {cluster}")
            })
            .iter()
            .map(|application| at(application, &["Name"]).as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            echoed, reported,
            "case {label:?}: a cluster launched from the catalogued identifiers must report \
             those identifiers, got {echoed:?}"
        );
    }
}

/// The filter's documented lower-case form still matches: `Application` is compared
/// without regard to case, so each identifier, its lower-case form and its upper-case
/// form keep exactly the releases `DescribeReleaseLabel` reports that application for.
#[tokio::test]
async fn a_filter_matches_the_identifier_however_it_is_spelled() {
    let emr = Emr::new();
    let catalog = published_labels(&emr, json!({})).await;

    // What each release bundles, read back through the operation that reports one
    // release's applications: nothing below restates the catalog.
    let mut bundles: Vec<(String, Vec<String>)> = Vec::new();
    for label in &catalog {
        let names = application_names(&emr, label).await;
        bundles.push((label.clone(), names));
    }

    for identifier in PUBLISHED_APPLICATION_NAMES {
        let expected: Vec<String> = bundles
            .iter()
            .filter(|(_, names)| names.iter().any(|name| name == identifier))
            .map(|(label, _)| label.clone())
            .collect();
        assert!(
            !expected.is_empty(),
            "case {identifier:?}: some release must bundle it, got {expected:?}"
        );
        for spelling in [
            (*identifier).to_string(),
            identifier.to_lowercase(),
            identifier.to_uppercase(),
        ] {
            let matched = published_labels(
                &emr,
                json!({ "Filters": { "Application": spelling.clone() } }),
            )
            .await;
            assert_eq!(
                matched, expected,
                "case Application={spelling:?}: the filter must keep exactly the releases \
                 DescribeReleaseLabel reports {identifier:?} for, got {matched:?}"
            );
        }
    }
}
