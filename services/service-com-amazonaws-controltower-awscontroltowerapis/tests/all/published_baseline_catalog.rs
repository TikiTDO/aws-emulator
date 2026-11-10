// SPDX-License-Identifier: CC0-1.0

//! The baseline catalog's PUBLISHED values, as `ListBaselines` and `GetBaseline` serve
//! them over HTTP.
//!
//! Drives both operations through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees.
//!
//! The RULE under test: a baseline is AWS-managed catalog data, identical in every
//! partition, region and account, so each entry's name, description and the 16-character
//! id segment of its ARN are AWS's own fixed values, which a client discovers by name or
//! ARN and then hands back to `GetBaseline`/`EnableBaseline` — the emulator reproduces
//! them rather than wording or minting its own. Only the partition and region segments of
//! a served ARN come from the request, and the account segment is empty.
//!
//! The sibling suites (`crate::baseline_catalog`, `crate::list_baselines`) assert the
//! listing and resolution rules against the service's own catalog constant; this suite
//! pins the published VALUES independently of that constant, so a re-worded description or
//! a re-minted id segment fails here even though both copies would still agree with each
//! other.
//!
//! The table below is AWS's published baseline catalog: the entries `ListBaselines`
//! returns in every account, including the ones no issue cites (`BackupAdminBaseline`,
//! `CentralConfigBaseline`, `CentralSecurityRolesBaseline`, `ConfigBaseline`). The four
//! descriptions flagged in `state::BASELINE_CATALOG` as following AWS's documented wording
//! rather than being known verbatim are pinned here too, so correcting one means
//! correcting both copies together. Its member names and types come from
//! `smithy_query(query="shape",
//! shape="BaselineSummary,GetBaselineOutput,BaselineArn")` on this crate's `smithy-model/`:
//! a `BaselineSummary` is `arn` (required), `name` (required) and `description`, and
//! `BaselineArn`'s pattern is
//! `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$`.

use smithy_server_runtime::MultiServiceRouter;

use crate::landing_zone_lifecycle::{call_in_region, emulator, Answer, SIGNED_REGION};

/// One published catalog entry: the name AWS publishes, the fixed id segment its ARN ends
/// in, and the fixed description text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Published {
    name: &'static str,
    id: &'static str,
    description: &'static str,
}

/// AWS's published baseline catalog, in the order `ListBaselines` reports it.
const PUBLISHED_CATALOG: &[Published] = &[
    Published {
        name: "AuditBaseline",
        id: "4T4HA1KMO10S6311",
        description: "Sets up resources to monitor security and compliance of accounts \
                      in your organization.",
    },
    Published {
        name: "AWSControlTowerBaseline",
        id: "17BSJV3IGJ2QSGA2",
        description: "Sets up resources and mandatory controls for member accounts \
                      within the target OU, required for AWS Control Tower governance.",
    },
    Published {
        name: "BackupAdminBaseline",
        id: "H6C5JFCJJ3CPU3J5",
        description: "Sets up a delegated administrator account for AWS Backup in your \
                      organization.",
    },
    Published {
        name: "BackupBaseline",
        id: "APO9ATVPBKFRRGLK",
        description: "Sets up backup resources for member accounts within the target \
                      OU.",
    },
    Published {
        name: "BackupCentralVaultBaseline",
        id: "3WPD0NA6TJ9AOMU2",
        description: "Sets up a central AWS Backup vault in your organization.",
    },
    Published {
        name: "CentralConfigBaseline",
        id: "YX7VMZML5IG8EJUD",
        description: "Sets up central resources for AWS Config in your organization.",
    },
    Published {
        name: "CentralSecurityRolesBaseline",
        id: "A6EEAE10F08193F2",
        description: "Sets up central resources for security monitoring within your \
                      organization.",
    },
    Published {
        name: "ConfigBaseline",
        id: "1QBGH2G48YVGDQ3Y",
        description: "Sets up resources for AWS Config in accounts in your \
                      organization.",
    },
    Published {
        name: "IdentityCenterBaseline",
        id: "LN25R72TTG6IGPTQ",
        description: "Sets up shared resources for AWS Identity Center, which prepares \
                      the AWSControlTowerBaseline to set up Identity Center access for \
                      accounts.",
    },
    Published {
        name: "LogArchiveBaseline",
        id: "J8HX46AHS5MIKQPD",
        description: "Sets up a central repository for logs of API activities and \
                      resource configurations from accounts in your organization.",
    },
];

/// The regions the rule is checked in, spanning what it generalises over (the request's
/// signed region, which is the only part of a served baseline ARN that moves).
const REQUEST_REGIONS: &[&str] = &[SIGNED_REGION, "us-east-1", "eu-central-1", "ap-south-1"];

/// The ARN `region` serves a published baseline under: that region, and the EMPTY account
/// segment a baseline no account owns carries.
fn published_arn(region: &str, entry: &Published) -> String {
    format!("arn:aws:controltower:{region}::baseline/{}", entry.id)
}

/// One listed or read baseline, as a client reads it off the response body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    arn: String,
    name: String,
    description: String,
}

/// The `arn`/`name`/`description` members of a body, missing ones read as empty.
fn reported(answer: &Answer, entry: &serde_json::Value) -> Reported {
    let member = |name: &str| {
        entry
            .get(name)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    assert!(
        !member("arn").is_empty() && !member("name").is_empty(),
        "both required members must be reported, got {:?}",
        answer.body
    );
    Reported {
        arn: member("arn"),
        name: member("name"),
        description: member("description"),
    }
}

/// POST `/list-baselines` from `region`, with no pagination members at all.
async fn list_baselines(router: &MultiServiceRouter, region: &str) -> Answer {
    call_in_region(router, region, "/list-baselines", serde_json::json!({})).await
}

/// POST `/get-baseline` from `region` with `identifier`.
async fn get_baseline(router: &MultiServiceRouter, region: &str, identifier: &str) -> Answer {
    call_in_region(
        router,
        region,
        "/get-baseline",
        serde_json::json!({ "baselineIdentifier": identifier }),
    )
    .await
}

/// RULE: `ListBaselines` publishes AWS's catalog — every entry's fixed name, fixed
/// description and fixed ARN id segment — whatever region the request is signed for, the
/// region being the only part of an entry that moves with the request.
#[tokio::test]
async fn the_listing_publishes_the_aws_catalog_in_every_region() {
    let router = emulator();

    for region in REQUEST_REGIONS {
        let case = format!("the listing signed for {region}");
        let answer = list_baselines(&router, region).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            answer.status, answer.body
        );

        let entries = answer
            .get("baselines")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a baselines list, got {:?}",
                    answer.body
                )
            })
            .iter()
            .map(|entry| reported(&answer, entry))
            .collect::<Vec<Reported>>();

        let expected: Vec<Reported> = PUBLISHED_CATALOG
            .iter()
            .map(|entry| Reported {
                arn: published_arn(region, entry),
                name: entry.name.to_string(),
                description: entry.description.to_string(),
            })
            .collect();

        assert_eq!(
            entries, expected,
            "case {case:?}: expected AWS's published catalog, got {:?}",
            answer.body
        );
    }
}

/// RULE (the resolution side of it): the ARN a published entry carries resolves, through
/// `GetBaseline`, to THAT entry's name and description — so an ARN a client read out of the
/// listing, or embedded from AWS's documentation, names the baseline AWS names.
#[tokio::test]
async fn every_published_arn_resolves_to_its_own_baseline() {
    let router = emulator();

    for entry in PUBLISHED_CATALOG {
        for region in REQUEST_REGIONS {
            // Asked for in the ARN form AWS documents, from a request signed for the
            // region in the ARN and from one signed for another: the id segment is the
            // identity, so both answer the same baseline.
            for (form, identifier) in [
                ("the ARN of the signed region", published_arn(region, entry)),
                (
                    "an ARN of another region",
                    published_arn("us-gov-west-1", entry),
                ),
            ] {
                let case = format!("{} via {form}, signed for {region}", entry.name);
                let answer = get_baseline(&router, region, &identifier).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected 200 for {identifier:?}, got {} body {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    reported(&answer, &answer.body),
                    Reported {
                        arn: published_arn(region, entry),
                        name: entry.name.to_string(),
                        description: entry.description.to_string(),
                    },
                    "case {case:?}: expected the published entry, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// RULE (the other side of a fixed catalog): an id segment AWS's catalog does not publish
/// names no baseline, so it is `ResourceNotFoundException` (404) — including one that
/// differs from a published id in a single character.
#[tokio::test]
async fn an_id_the_published_catalog_does_not_carry_is_not_found() {
    let mut cases: Vec<(String, String)> = PUBLISHED_CATALOG
        .iter()
        .map(|entry| {
            let (head, last) = entry.id.split_at(15);
            let rotated = if last == "Z" { "Y" } else { "Z" };
            (
                format!("{} with its last character changed", entry.name),
                format!("{head}{rotated}"),
            )
        })
        .collect();
    cases.push((
        "an id of no baseline at all".to_string(),
        "9XQV3TLK8MDR5WZA".to_string(),
    ));

    let router = emulator();
    for (case, id) in cases {
        let identifier = format!("arn:aws:controltower:{SIGNED_REGION}::baseline/{id}");
        let answer = get_baseline(&router, SIGNED_REGION, &identifier).await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected 404 ResourceNotFoundException for {identifier:?}, \
             got {:?} body {:?}",
            (answer.status, answer.error_code.as_deref()),
            answer.body
        );
    }
}
