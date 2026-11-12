// SPDX-License-Identifier: CC0-1.0

//! `GetJobs`: the FULL job definitions this endpoint's Region holds.
//!
//! RULE: `GetJobs` "retrieves all current job definitions" — so it reports, for the
//! Region the request is served in and no other, the whole definition of every job the
//! store holds, element for element the projection `GetJob` serves, one
//! `MaxResults`-sized page at a time with a `NextToken` that resumes the walk where the
//! previous page stopped.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many jobs are stored, the
//! page size asked for (absent, both published bounds of `MaxResults` and one past the
//! stored count), which members the stored definitions carry, and which Region's endpoint
//! asks.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, emulator, names_in, ok, REGION};

/// A role ARN `CreateJob` accepts. An IAM role is account-global, so the same one backs a
/// job in every Region.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/glue-definition-listing-role";

/// The Regions, other than the harness's own, these tests address an endpoint in.
const OTHER_REGIONS: [&str; 2] = ["eu-west-1", "ap-southeast-1"];

/// A `CreateJob` body for `name`, pointed at a script in Amazon S3.
fn job_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
    })
}

/// Create `name` through `region`'s endpoint, with `body` as sent.
async fn create_job_in(router: &MultiServiceRouter, region: &str, body: &Value) {
    let (status, answer) = call_in_region(router, region, "CreateJob", body).await;
    assert_eq!(
        status, 200,
        "case {region}: CreateJob must answer 200, got {status} with {answer}"
    );
}

/// Every job definition a PAGED `GetJobs` sweep of `region`'s endpoint reports, following
/// the service's own `NextToken` to exhaustion with pages of `page_size` (absent when
/// `None`).
async fn sweep_definitions_in(
    router: &MultiServiceRouter,
    region: &str,
    page_size: Option<i32>,
) -> Vec<Value> {
    let case = format!("{region} / MaxResults {page_size:?}");
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(size) = page_size {
            body["MaxResults"] = json!(size);
        }
        if let Some(cursor) = token.as_ref() {
            body["NextToken"] = json!(cursor);
        }
        let (status, answer) = call_in_region(router, region, "GetJobs", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: GetJobs must answer 200, got {status} with {answer}"
        );
        let page = answer["Jobs"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: the answer must report a Jobs list, got {answer}"))
            .clone();
        if let Some(size) = page_size {
            assert!(
                page.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case}: a page must hold at most {size} definitions, got {}",
                page.len()
            );
        }
        seen.extend(page);
        token = answer["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            seen.len() <= 1_000,
            "case {case}: the walk must terminate, got {} definitions so far",
            seen.len()
        );
    }
    seen
}

/// The definition `GetJob` reports for `name` through `region`'s endpoint.
async fn read_definition_in(router: &MultiServiceRouter, region: &str, name: &str) -> Value {
    let (status, answer) =
        call_in_region(router, region, "GetJob", &json!({ "JobName": name })).await;
    assert_eq!(
        status, 200,
        "case {region} / {name}: GetJob must answer 200, got {status} with {answer}"
    );
    answer["Job"].clone()
}

/// Every stored job definition is reported, in name order, one bounded page at a time, and
/// each one is the SAME definition the single-job read serves.
#[tokio::test]
async fn every_stored_job_definition_is_reported_and_paged() {
    // (case, how many jobs are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no jobs at all", 0, None),
        ("one job, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize` range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page one short of the stored count", 4, Some(3)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        let expected: Vec<String> = (0..*stored).map(|index| format!("job-{index:02}")).collect();
        // Created out of name order, so the listing's own order is what is asserted.
        for name in expected.iter().rev() {
            create_job_in(&router, REGION, &job_body(name)).await;
        }

        let swept = sweep_definitions_in(&router, REGION, *page_size).await;
        let listed: Vec<String> = swept
            .iter()
            .map(|job| {
                job["Name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: every definition carries a Name, got {job}"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            listed, expected,
            "case {case}: expected every stored job in name order, got {listed:?}"
        );

        // Element for element the definition `GetJob` serves: one record, two reads.
        for (job, name) in swept.iter().zip(expected.iter()) {
            let read = read_definition_in(&router, REGION, name).await;
            assert_eq!(
                job, &read,
                "case {case}: the listed definition of {name} must be the one GetJob serves, \
                 expected {read}, got {job}"
            );
        }
    }
}

/// The whole definition is reported, whatever members the create stored on it: the
/// listing is not a name list with a shape around it.
///
/// The rows differ along which optional members the stored definition carries.
#[tokio::test]
async fn the_listing_reports_the_members_each_definition_carries() {
    let router = emulator();

    // (case, the CreateJob body)
    let mut rows: Vec<(&str, Value)> = Vec::new();
    rows.push(("only the required members", job_body("plain-job")));

    let mut described = job_body("described-job");
    described["Description"] = json!("nightly usage rollup");
    described["GlueVersion"] = json!("4.0");
    rows.push(("a description and a Glue version", described));

    let mut tuned = job_body("tuned-job");
    tuned["MaxRetries"] = json!(3);
    tuned["Timeout"] = json!(120);
    tuned["ExecutionProperty"] = json!({ "MaxConcurrentRuns": 4 });
    tuned["DefaultArguments"] = json!({ "--job-language": "python", "--enable-metrics": "" });
    rows.push(("retries, a timeout, concurrency and arguments", tuned));

    let mut tagged = job_body("tagged-job");
    tagged["Tags"] = json!({ "team": "data", "env": "prod" });
    tagged["WorkerType"] = json!("G.1X");
    tagged["NumberOfWorkers"] = json!(5);
    rows.push(("tags and a worker shape", tagged));

    for (_, body) in &rows {
        create_job_in(&router, REGION, body).await;
    }

    let swept = sweep_definitions_in(&router, REGION, None).await;
    for (case, body) in &rows {
        let name = body["Name"].as_str().expect("the body names the job");
        let listed = swept
            .iter()
            .find(|job| job["Name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("case {case}: {name} must be listed, got {swept:?}"));
        // Every member the create sent reappears on the listed definition, unchanged —
        // except the tags, which `Job` (`smithy_query shape=Job`) has no member for: a
        // job's tags are read through `GetTags`, so they are not part of the definition
        // this listing projects.
        let mut expected = body.clone();
        if let Some(members) = expected.as_object_mut() {
            members.remove("Tags");
        }
        crate::wire::assert_echoed(case, &expected, listed);
        let read = read_definition_in(&router, REGION, name).await;
        assert_eq!(
            listed, &read,
            "case {case}: the listed definition must be the one GetJob serves, expected \
             {read}, got {listed}"
        );
    }
}

/// A job definition is reported only by the Region it was created in: each endpoint's
/// sweep carries its own records, at every page size, and never another Region's.
#[tokio::test]
async fn the_listing_reports_only_the_endpoints_own_region() {
    let router = emulator();

    // Two jobs per Region, each carrying that Region's own script location, so a leaked
    // record is visible even when two Regions share a job name.
    const SHARED: &str = "shared-definition-name";
    let regions: Vec<&str> = [REGION].into_iter().chain(OTHER_REGIONS).collect();
    let mut owned: Vec<(&str, Vec<String>)> = Vec::new();
    for region in &regions {
        let mut names = vec![format!("etl-{region}"), SHARED.to_string()];
        for name in &names {
            let mut body = job_body(name);
            body["Command"]["ScriptLocation"] = json!(format!("s3://etl-scripts/{region}/{name}.py"));
            create_job_in(&router, region, &body).await;
        }
        names.sort();
        owned.push((region, names));
    }

    for (region, names) in &owned {
        for page_size in [None, Some(1), Some(2), Some(3)] {
            let case = format!("{region} / MaxResults {page_size:?}");
            let swept = sweep_definitions_in(&router, region, page_size).await;
            let listed: Vec<String> = swept
                .iter()
                .map(|job| job["Name"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                &listed, names,
                "case {case}: the endpoint must report its own definitions and no other \
                 Region's, got {listed:?}"
            );
            // And each record is the one THAT Region stored, not another Region's copy of
            // the same name.
            for job in &swept {
                let name = job["Name"].as_str().unwrap_or_default();
                assert_eq!(
                    job["Command"]["ScriptLocation"].as_str(),
                    Some(format!("s3://etl-scripts/{region}/{name}.py").as_str()),
                    "case {case}: {name} must be the record this Region stored, got {job}"
                );
            }
        }
    }
}

/// The listing is a read: sweeping it changes nothing a later caller sees, however many
/// times it is swept.
#[tokio::test]
async fn sweeping_the_listing_changes_nothing() {
    let router = emulator();
    for name in ["read-only-a", "read-only-b", "read-only-c"] {
        create_job_in(&router, REGION, &job_body(name)).await;
    }

    let first = sweep_definitions_in(&router, REGION, Some(2)).await;
    for round in 0..3 {
        let again = sweep_definitions_in(&router, REGION, Some(2)).await;
        assert_eq!(
            again, first,
            "case round {round}: a read must report the same definitions, got {again:?}"
        );
    }
    // Nothing a run listing can see was started either.
    for job in &first {
        let name = job["Name"].as_str().expect("a listed definition carries a Name");
        let history = ok(&router, "GetJobRuns", &json!({ "JobName": name })).await;
        assert_eq!(
            history["JobRuns"].as_array().map(Vec::len),
            Some(0),
            "case {name}: reading the definitions must start no run, got {history}"
        );
    }
}

/// The name listing and the definition listing agree name for name: the two reads walk the
/// same records, so neither reports a job the other does not.
#[tokio::test]
async fn the_definition_listing_agrees_with_the_name_listing() {
    let router = emulator();
    for name in ["agree-02", "agree-00", "agree-01"] {
        create_job_in(&router, REGION, &job_body(name)).await;
    }

    let definitions = ok(&router, "GetJobs", &json!({})).await;
    let names = ok(&router, "ListJobs", &json!({})).await;
    assert_eq!(
        names_in(&definitions, "Jobs"),
        crate::wire::string_list(&names, "JobNames"),
        "the two listings must agree name for name, got {definitions} and {names}"
    );
}

/// A page size outside the published range is refused by the modelled constraint rather
/// than served as some other page size.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    create_job_in(&router, REGION, &job_body("bounded-definition")).await;

    // `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1_001)] {
        let (status, body) = call(&router, "GetJobs", &json!({ "MaxResults": max_results })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }
}
