// SPDX-License-Identifier: CC0-1.0

//! `RequestEnvironmentInfo` and `RetrieveEnvironmentInfo` — "Initiates a request
//! to compile the specified type of information of the deployed environment …
//! Use RetrieveEnvironmentInfo to obtain the set of logs" / "Retrieves the
//! compiled information from a RequestEnvironmentInfo request" — driven end to
//! end over the service's awsQuery HTTP surface.
//!
//! RULE under test: `RetrieveEnvironmentInfo` reports the information the LAST
//! `RequestEnvironmentInfo` for the same environment and the same `InfoType`
//! compiled — one `EnvironmentInfoDescription` per Amazon EC2 instance that
//! environment was made of when the request was made, each naming that
//! `InfoType`, that instance, the instant the compile finished and the
//! presigned Amazon S3 URL the file compiled from that instance downloads at —
//! from the moment the compile the clock owes finishes until the files it
//! produced are deleted; before any request, before the compile finishes, for
//! another `InfoType` and after the deletion it reports an empty list, and
//! neither operation answers at all for an identifier that names no
//! environment.
//!
//! The harness is the one `lifecycle.rs` owns: the same `MultiServiceRouter` the
//! production binary mounts, spoken to with a form-urlencoded `Action=…` body,
//! with object storage mounted beside Elastic Beanstalk on one engine — which is
//! what the assembled emulator does, and what the compiled files are stored in.
//! Every assertion below is made over what the HTTP read answers, and every
//! reported URL is FOLLOWED, so an operation that persisted nothing, fabricated
//! entries, named an object nothing wrote or answered from anything other than
//! the store fails these tests. `Emulator::store` is reached in two tests only:
//! to bring the 15-minute deletion the clock owes forward, and to seed more
//! activity than any implemented operation records in one run — no implemented
//! operation can make a quarter of an hour pass.
//!
//! Each test is a TABLE over the class the rule generalises over: which
//! `InfoType` was asked for (every value of the modelled set), how many
//! instances the environment is made of, which identifier names it, how many
//! requests have been made, how much of the log one `InfoType` carries and what
//! the clock has made due.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::lifecycle::{Answer, Emulator, PAST_DUE, SOLUTION_STACK, TEST_REGION};

/// The application every environment below is launched under.
const APPLICATION: &str = "info-app";

/// `aws:elasticbeanstalk:environment` / `EnvironmentType` set to
/// `SingleInstance` — "launch one EC2 instance with no load balancer".
pub(crate) const SINGLE_INSTANCE: &[(&str, &str)] = &[
    (
        "OptionSettings.member.1.Namespace",
        "aws:elasticbeanstalk:environment",
    ),
    ("OptionSettings.member.1.OptionName", "EnvironmentType"),
    ("OptionSettings.member.1.Value", "SingleInstance"),
];

/// `aws:autoscaling:asg` / `MinSize` at 3 — "the minimum number of instances you
/// want in your Auto Scaling group", so the environment is made of three.
const MIN_SIZE_3: &[(&str, &str)] = &[
    ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
    ("OptionSettings.member.1.OptionName", "MinSize"),
    ("OptionSettings.member.1.Value", "3"),
];

/// The same option at 5.
const MIN_SIZE_5: &[(&str, &str)] = &[
    ("OptionSettings.member.1.Namespace", "aws:autoscaling:asg"),
    ("OptionSettings.member.1.OptionName", "MinSize"),
    ("OptionSettings.member.1.Value", "5"),
];

/// One reported `EnvironmentInfoDescription`.
///
/// `message` is "the retrieved information", which "currently contains a
/// presigned Amazon S3 URL" of the file compiled from that instance — a URL the
/// tests below FOLLOW, so an entry naming an object object storage does not hold
/// fails them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) info_type: String,
    pub(crate) ec2_instance_id: String,
    pub(crate) sample_timestamp: String,
    pub(crate) message: String,
}

/// Every entry of a `RetrieveEnvironmentInfo` answer, in the order it lists
/// them.
///
/// `pub(crate)`: the deployed-application suite reads the same entries to follow
/// the URL one of them reports, so neither suite keeps a second reader.
pub(crate) fn entries(answer: &Answer) -> Vec<Entry> {
    answer
        .elements_at("EnvironmentInfo/member")
        .iter()
        .map(|entry| Entry {
            info_type: entry.value("InfoType").unwrap_or_default().to_string(),
            ec2_instance_id: entry.value("Ec2InstanceId").unwrap_or_default().to_string(),
            sample_timestamp: entry
                .value("SampleTimestamp")
                .unwrap_or_default()
                .to_string(),
            // The envelope escapes the `&` separating the URL's query
            // parameters, and a caller's SDK hands the URL back unescaped.
            message: entry
                .value("Message")
                .unwrap_or_default()
                .replace("&amp;", "&"),
        })
        .collect()
}

/// Whether the answer carries an `EnvironmentInfo` element at all — an empty
/// `<EnvironmentInfo></EnvironmentInfo>` is a reported-but-empty list, which
/// [`Answer::value`] reads as absent.
fn reports_the_list(answer: &Answer) -> bool {
    answer.body.contains("<EnvironmentInfo")
}

/// The query parameters a presigned SigV4 URL carries, which the `Message` must
/// therefore carry: the algorithm, the credential scope, the signing instant,
/// how long it is valid, which headers it signed and the signature itself.
const PRESIGNED_PARAMETERS: &[&str] = &[
    "X-Amz-Algorithm=AWS4-HMAC-SHA256",
    "X-Amz-Credential=",
    "X-Amz-Date=",
    "X-Amz-Expires=",
    "X-Amz-SignedHeaders=",
    "X-Amz-Signature=",
];

/// Everything before the query string of the URL the file compiled from
/// `ec2_instance_id` must be reported at: the base URL this emulator is reached
/// on — a `Message` is an address the caller FOLLOWS, so it names a host this
/// emulator answers — then the account's Elastic Beanstalk storage bucket,
/// addressed in the path, and the service's own key for that file.
fn expected_url_prefix(info_type: &str, environment_id: &str, ec2_instance_id: &str) -> String {
    format!(
        "{}/{}/{}?",
        smithy_server_runtime::UNSERVED_URL_BASE,
        eb::state::storage_bucket_name(TEST_REGION),
        eb::state::environment_info_key(info_type, environment_id, ec2_instance_id)
    )
}

/// Call `action`, failing the named case unless it answered 200.
pub(crate) async fn ok(
    emulator: &Emulator,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> Answer {
    let answer = emulator.call(action, params).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from {action}, got {} and body {}",
        answer.status, answer.body
    );
    answer
}

/// Create [`APPLICATION`].
pub(crate) async fn application(emulator: &Emulator) {
    ok(
        emulator,
        "fixture",
        "CreateApplication",
        &[("ApplicationName", APPLICATION)],
    )
    .await;
}

/// Launch `environment` with `extra` parameters and wait out the launch the
/// clock owes, so it is `Ready`; answers its `EnvironmentId`.
pub(crate) async fn launch(
    emulator: &Emulator,
    environment: &str,
    extra: &[(&str, &str)],
    case: &str,
) -> String {
    let mut params: Vec<(&str, &str)> = vec![
        ("ApplicationName", APPLICATION),
        ("EnvironmentName", environment),
        ("SolutionStackName", SOLUTION_STACK.as_str()),
    ];
    params.extend_from_slice(extra);
    let launched = ok(emulator, case, "CreateEnvironment", &params).await;
    let environment_id = launched.expect("EnvironmentId", case).to_string();
    std::thread::sleep(PAST_DUE);
    environment_id
}

/// The Amazon EC2 instances `DescribeEnvironmentResources` reports for
/// `environment` — the inventory the compile must sample, read from the
/// operation that owns it rather than from any list of this suite's own.
async fn reported_instances(emulator: &Emulator, environment: &str, case: &str) -> BTreeSet<String> {
    let answer = ok(
        emulator,
        case,
        "DescribeEnvironmentResources",
        &[("EnvironmentName", environment)],
    )
    .await;
    answer
        .values_at("EnvironmentResources/Instances/member/Id")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// `RetrieveEnvironmentInfo` for `info_type`, by whichever identifier `by`
/// names.
pub(crate) async fn retrieve(
    emulator: &Emulator,
    by: (&str, &str),
    info_type: &str,
    case: &str,
) -> Vec<Entry> {
    let answer = ok(
        emulator,
        case,
        "RetrieveEnvironmentInfo",
        &[by, ("InfoType", info_type)],
    )
    .await;
    assert!(
        reports_the_list(&answer),
        "case {case:?}: RetrieveEnvironmentInfo must report an EnvironmentInfo list, got {}",
        answer.body
    );
    entries(&answer)
}

/// Epoch seconds now, as the bound a reported `SampleTimestamp` is checked
/// against.
fn clock_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// The compiled information
// ---------------------------------------------------------------------------

/// One row of the main table: an environment of a known instance count, the
/// `InfoType` asked for, and another `InfoType` nobody asked for.
#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    environment: &'static str,
    options: &'static [(&'static str, &'static str)],
    info_type: &'static str,
    /// An `InfoType` the row does NOT request, so the answer for it must stay
    /// empty: the information is held per `InfoType`, not per environment.
    other_info_type: &'static str,
    expected_instances: usize,
}

/// The table: every value of the modelled `EnvironmentInfoType` set, over
/// environments made of one, three and five instances.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "tail from a single-instance environment",
            environment: "info-tail-single",
            options: SINGLE_INSTANCE,
            info_type: "tail",
            other_info_type: "bundle",
            expected_instances: 1,
        },
        Case {
            name: "bundle from a load-balanced environment at its default size",
            environment: "info-bundle-default",
            options: &[],
            info_type: "bundle",
            other_info_type: "analyze",
            expected_instances: eb::state::DEFAULT_MIN_SIZE,
        },
        Case {
            name: "analyze from a group of three",
            environment: "info-analyze-three",
            options: MIN_SIZE_3,
            info_type: "analyze",
            other_info_type: "tail",
            expected_instances: 3,
        },
        Case {
            name: "tail from a group of five",
            environment: "info-tail-five",
            options: MIN_SIZE_5,
            info_type: "tail",
            other_info_type: "analyze",
            expected_instances: 5,
        },
    ]
}

/// RULE: the information a request compiled is reported once the compile is
/// due, as one entry per instance the environment was made of, under the
/// `InfoType` it was asked for.
#[tokio::test]
async fn the_compiled_information_is_reported_per_instance_once_the_compile_is_due() {
    // The table cannot drift from the model: every `InfoType` it uses is one
    // the model declares, and every declared value is covered by a row.
    let modelled: BTreeSet<&str> = eb::ops::types::ENVIRONMENT_INFO_TYPE_VALUES
        .iter()
        .copied()
        .collect();
    let covered: BTreeSet<&str> = cases().iter().map(|case| case.info_type).collect();
    assert_eq!(
        covered, modelled,
        "the table must cover every modelled EnvironmentInfoType, covers {covered:?} of {modelled:?}"
    );

    let emulator = Emulator::with_object_storage();
    application(&emulator).await;

    for case in cases() {
        let name = case.name;
        let environment_id = launch(&emulator, case.environment, case.options, name).await;
        // The line every launched environment's activity log carries, so the
        // downloaded file is checked against state this suite did not write.
        let launch_line = format!("Successfully launched environment: {}", case.environment);
        let instances = reported_instances(&emulator, case.environment, name).await;
        assert_eq!(
            instances.len(),
            case.expected_instances,
            "case {name:?}: expected {} instances in the environment, got {instances:?}",
            case.expected_instances
        );

        // Nothing has been requested yet, so there is nothing to retrieve.
        let before_request = retrieve(
            &emulator,
            ("EnvironmentName", case.environment),
            case.info_type,
            name,
        )
        .await;
        assert!(
            before_request.is_empty(),
            "case {name:?}: information nobody requested must not be retrievable, got {before_request:?}"
        );

        let requested_at = clock_now();
        let requested = ok(
            &emulator,
            name,
            "RequestEnvironmentInfo",
            &[
                ("EnvironmentName", case.environment),
                ("InfoType", case.info_type),
            ],
        )
        .await;
        assert!(
            !reports_the_list(&requested),
            "case {name:?}: RequestEnvironmentInfo answers nothing but an envelope, got {}",
            requested.body
        );

        // The compile the request started is not finished yet, so the
        // information is still not retrievable.
        let while_compiling = retrieve(
            &emulator,
            ("EnvironmentName", case.environment),
            case.info_type,
            name,
        )
        .await;
        assert!(
            while_compiling.is_empty(),
            "case {name:?}: a compile the clock has not finished must report nothing, got {while_compiling:?}"
        );

        std::thread::sleep(PAST_DUE);
        let compiled = retrieve(
            &emulator,
            ("EnvironmentName", case.environment),
            case.info_type,
            name,
        )
        .await;
        let retrieved_at = clock_now();
        assert_eq!(
            compiled.len(),
            case.expected_instances,
            "case {name:?}: expected one entry per instance ({}), got {compiled:?}",
            case.expected_instances
        );
        let sampled: BTreeSet<String> = compiled
            .iter()
            .map(|entry| entry.ec2_instance_id.clone())
            .collect();
        assert_eq!(
            sampled, instances,
            "case {name:?}: the entries must name the environment's own instances, got {sampled:?} for {instances:?}"
        );
        for entry in &compiled {
            assert_eq!(
                entry.info_type, case.info_type,
                "case {name:?}: every entry reports the InfoType asked for, got {entry:?}"
            );
            let sampled_at = crate::harness::instant(&entry.sample_timestamp, name);
            assert!(
                sampled_at >= requested_at && sampled_at <= retrieved_at,
                "case {name:?}: the SampleTimestamp must fall between the request and the retrieve \
                 ({requested_at}..={retrieved_at}), got {sampled_at} in {entry:?}"
            );
            assert!(
                entry.message.starts_with(&expected_url_prefix(
                    case.info_type,
                    &environment_id,
                    &entry.ec2_instance_id
                )),
                "case {name:?}: the Message must be the Amazon S3 URL of this instance's compiled \
                 file, got {entry:?}"
            );
            for parameter in PRESIGNED_PARAMETERS {
                assert!(
                    entry.message.contains(parameter),
                    "case {name:?}: the Message must be a presigned URL carrying {parameter}, got \
                     {entry:?}"
                );
            }
            // The URL is followed, with no credential but its own: a presigned
            // URL the emulator verifies, naming an object object storage really
            // holds, whose bytes are the environment's own activity log.
            let downloaded = emulator.get_url(&entry.message).await;
            assert_eq!(
                downloaded.status, 200,
                "case {name:?}: following the compiled file's URL must answer 200, got {} and \
                 body {}",
                downloaded.status, downloaded.body
            );
            assert!(
                downloaded.body.contains(&launch_line),
                "case {name:?}: the compiled file must carry the environment's own activity log \
                 (expected a line containing {launch_line:?}), got {:?}",
                downloaded.body
            );
        }

        // The information is held per `InfoType`: another one was never asked
        // for, so it reports nothing.
        let other = retrieve(
            &emulator,
            ("EnvironmentName", case.environment),
            case.other_info_type,
            name,
        )
        .await;
        assert!(
            other.is_empty(),
            "case {name:?}: {:?} was never requested, so it must report nothing, got {other:?}",
            case.other_info_type
        );
    }
}

/// RULE: both identifiers name the same compile — the information a request
/// made by `EnvironmentName` compiled is the information a retrieve by
/// `EnvironmentId` reports, and the other way round.
#[tokio::test]
async fn either_identifier_names_the_same_compiled_information() {
    let emulator = Emulator::with_object_storage();
    application(&emulator).await;
    let environment = "info-identifiers";
    let environment_id = launch(&emulator, environment, MIN_SIZE_3, "identifiers").await;

    /// One row: which identifier the request names and which one the retrieve
    /// names.
    struct Row<'a> {
        name: &'static str,
        requested_by: (&'a str, &'a str),
        retrieved_by: (&'a str, &'a str),
    }
    // One row per (identifier the request names, identifier the retrieve
    // names), so neither operation can be the only one that resolves both.
    let by_name = ("EnvironmentName", environment);
    let by_id = ("EnvironmentId", environment_id.as_str());
    let rows = [
        Row {
            name: "name then name",
            requested_by: by_name,
            retrieved_by: by_name,
        },
        Row {
            name: "name then id",
            requested_by: by_name,
            retrieved_by: by_id,
        },
        Row {
            name: "id then id",
            requested_by: by_id,
            retrieved_by: by_id,
        },
        Row {
            name: "id then name",
            requested_by: by_id,
            retrieved_by: by_name,
        },
    ];
    let instances = reported_instances(&emulator, environment, "identifiers").await;
    for row in rows {
        let name = row.name;
        ok(
            &emulator,
            name,
            "RequestEnvironmentInfo",
            &[row.requested_by, ("InfoType", "tail")],
        )
        .await;
        std::thread::sleep(PAST_DUE);
        let compiled = retrieve(&emulator, row.retrieved_by, "tail", name).await;
        let sampled: BTreeSet<String> = compiled
            .iter()
            .map(|entry| entry.ec2_instance_id.clone())
            .collect();
        assert_eq!(
            sampled, instances,
            "case {name:?}: both identifiers must name the same environment's compile, got {compiled:?}"
        );
    }
}

/// RULE: a second request for the same information RECOMPILES it — until that
/// compile finishes nothing is retrievable, and afterwards the entries carry
/// the newer sample instant, never both samples.
#[tokio::test]
async fn requesting_the_same_information_again_replaces_what_the_last_request_compiled() {
    let emulator = Emulator::with_object_storage();
    application(&emulator).await;
    let environment = "info-recompiled";
    launch(&emulator, environment, MIN_SIZE_3, "recompile").await;
    let instances = reported_instances(&emulator, environment, "recompile").await;

    // Three requests in a row: each one's compile replaces the previous one's
    // result, so the reported sample instant strictly increases and the entry
    // count never grows.
    let mut previous: Option<f64> = None;
    for round in 1..=3u32 {
        let case = format!("recompile round {round}");
        ok(
            &emulator,
            &case,
            "RequestEnvironmentInfo",
            &[("EnvironmentName", environment), ("InfoType", "tail")],
        )
        .await;
        let while_recompiling =
            retrieve(&emulator, ("EnvironmentName", environment), "tail", &case).await;
        assert!(
            while_recompiling.is_empty(),
            "case {case:?}: a recompile in flight replaces the previous result, so nothing is \
             retrievable until it finishes, got {while_recompiling:?}"
        );

        std::thread::sleep(PAST_DUE);
        let compiled = retrieve(&emulator, ("EnvironmentName", environment), "tail", &case).await;
        let sampled: BTreeSet<String> = compiled
            .iter()
            .map(|entry| entry.ec2_instance_id.clone())
            .collect();
        assert_eq!(
            sampled, instances,
            "case {case:?}: one entry per instance and no more, got {compiled:?}"
        );
        let sampled_at = crate::harness::instant(
            &compiled
                .first()
                .unwrap_or_else(|| panic!("case {case:?}: expected entries, got {compiled:?}"))
                .sample_timestamp,
            &case,
        );
        if let Some(previous) = previous {
            assert!(
                sampled_at > previous,
                "case {case:?}: a recompile must report a newer SampleTimestamp than {previous}, got {sampled_at}"
            );
        }
        previous = Some(sampled_at);
    }
}

/// RULE: the compiled files are deleted after the documented 15 minutes, and
/// from then on the information is not retrievable and the store holds nothing
/// for it.
#[tokio::test]
async fn information_is_not_retrievable_once_the_compiled_files_are_deleted() {
    // "The files are deleted after 15 minutes" — the documented duration keeps
    // its documented value.
    assert_eq!(
        eb::state::ENVIRONMENT_INFO_RETENTION,
        Duration::from_secs(15 * 60),
        "the documented retention is 15 minutes, the service holds {:?}",
        eb::state::ENVIRONMENT_INFO_RETENTION
    );

    let emulator = Emulator::with_object_storage();
    application(&emulator).await;
    let environment = "info-deleted";
    let environment_id = launch(&emulator, environment, MIN_SIZE_3, "deleted").await;

    // One row per modelled `InfoType`, so the deletion is not a property of one
    // of them.
    for info_type in eb::ops::types::ENVIRONMENT_INFO_TYPE_VALUES {
        let case = format!("deleted {info_type}");
        ok(
            &emulator,
            &case,
            "RequestEnvironmentInfo",
            &[("EnvironmentName", environment), ("InfoType", info_type)],
        )
        .await;
        std::thread::sleep(PAST_DUE);
        let compiled = retrieve(&emulator, ("EnvironmentName", environment), info_type, &case).await;
        assert!(
            !compiled.is_empty(),
            "case {case:?}: the compiled information must be retrievable before it is deleted, got {compiled:?}"
        );
        let urls: Vec<String> = compiled
            .iter()
            .map(|entry| entry.message.clone())
            .collect();
        for url in &urls {
            let downloaded = emulator.get_url(url).await;
            assert_eq!(
                downloaded.status, 200,
                "case {case:?}: a file that is not deleted yet must download, got {} and body {}",
                downloaded.status, downloaded.body
            );
        }

        // Bring the deletion forward: no implemented operation can make 15
        // minutes pass, so the record's expiry instant is moved to one the
        // clock has already passed — the instant the compile FINISHED.
        {
            let state = emulator.store();
            let mut store = state.lock();
            let record = store
                .environment_info
                .get_mut(&(environment_id.clone(), (*info_type).to_string()))
                .unwrap_or_else(|| panic!("case {case:?}: the request's compile must be stored"));
            record.expires_at = record.due_at;
        }

        let after_deletion =
            retrieve(&emulator, ("EnvironmentName", environment), info_type, &case).await;
        assert!(
            after_deletion.is_empty(),
            "case {case:?}: deleted files must not be reported, got {after_deletion:?}"
        );
        let held = emulator.store().lock().environment_info.len();
        assert_eq!(
            held, 0,
            "case {case:?}: the deleted compile must be dropped from the store, {held} record(s) left"
        );
        // "The files are deleted": the URLs that downloaded them no longer do.
        for url in &urls {
            let downloaded = emulator.get_url(url).await;
            assert_ne!(
                downloaded.status, 200,
                "case {case:?}: a deleted file must no longer download, got body {}",
                downloaded.body
            );
        }
    }
}

/// RULE: how much of the environment's log one compile carries follows the
/// `InfoType`: a `tail` carries the LAST lines of it, every other value the
/// whole of it.
#[tokio::test]
async fn a_tail_carries_the_last_lines_of_the_log_and_every_other_type_the_whole_of_it() {
    let emulator = Emulator::with_object_storage();
    application(&emulator).await;
    let environment = "info-log-extent";
    let environment_id = launch(&emulator, environment, SINGLE_INSTANCE, "log extent").await;

    // More activity than a tail carries, so the two extents differ. Seeded
    // through the store: no implemented operation records this many events for
    // one environment, and what is under test is still read over HTTP.
    let seeded = eb::state::ENVIRONMENT_INFO_TAIL_LINES + 50;
    let last_seeded = format!("seeded activity line {}", seeded - 1);
    {
        let state = emulator.store();
        let mut store = state.lock();
        let subject = eb::state::EventSubject {
            environment_id: Some(environment_id.clone()),
            environment_name: Some(environment.to_string()),
            ..Default::default()
        };
        for line in 0..seeded {
            store.record_event(
                "seeded-request",
                eb::state::SEVERITY_INFO,
                format!("seeded activity line {line}"),
                subject.clone(),
            );
        }
    }

    // One row per extent: the last lines, and the whole log.
    for info_type in ["tail", "bundle"] {
        let case = format!("log extent {info_type}");
        ok(
            &emulator,
            &case,
            "RequestEnvironmentInfo",
            &[("EnvironmentName", environment), ("InfoType", info_type)],
        )
        .await;
        std::thread::sleep(PAST_DUE);
        let compiled = retrieve(&emulator, ("EnvironmentName", environment), info_type, &case).await;
        let entry = compiled
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: expected an entry, got {compiled:?}"));
        let downloaded = emulator.get_url(&entry.message).await;
        assert_eq!(
            downloaded.status, 200,
            "case {case:?}: the compiled file must download, got {} and body {}",
            downloaded.status, downloaded.body
        );
        let lines: Vec<&str> = downloaded.body.lines().collect();
        let first = lines.first().copied().unwrap_or_default();
        assert!(
            downloaded.body.contains(&last_seeded),
            "case {case:?}: the compiled file must carry the newest lines of the log \
             ({last_seeded:?}), got {} lines ending {:?}",
            lines.len(),
            lines.last().copied().unwrap_or_default()
        );
        // A log file reads oldest-first, so every line is dated at or after the
        // line before it.
        let dates: Vec<f64> = lines
            .iter()
            .map(|line| {
                crate::harness::instant(line.split(' ').next().unwrap_or_default(), &case)
            })
            .collect();
        assert!(
            dates.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case:?}: the compiled log must read oldest-first, got {dates:?}"
        );
        if info_type == eb::state::ENVIRONMENT_INFO_TYPE_TAIL {
            assert_eq!(
                lines.len(),
                eb::state::ENVIRONMENT_INFO_TAIL_LINES,
                "case {case:?}: a tail carries the last {} lines, got {}",
                eb::state::ENVIRONMENT_INFO_TAIL_LINES,
                lines.len()
            );
            assert!(
                !first.contains("createEnvironment"),
                "case {case:?}: a tail must not reach back to the start of the log, got {first:?}"
            );
        } else {
            assert!(
                lines.len() > eb::state::ENVIRONMENT_INFO_TAIL_LINES,
                "case {case:?}: the whole log is longer than a tail of it, got {} lines",
                lines.len()
            );
            assert!(
                first.contains("createEnvironment"),
                "case {case:?}: the whole log starts at the environment's first event, got \
                 {first:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What neither operation answers
// ---------------------------------------------------------------------------
/// RULE: an identifier that names no environment is refused with
/// `InvalidParameterValue`, naming the member and the value it carried, and a
/// request that names neither identifier with `MissingRequiredParameter` — by
/// both operations alike.
#[tokio::test]
async fn an_identifier_that_names_no_environment_is_refused_by_both_operations() {
    let emulator = Emulator::with_object_storage();
    application(&emulator).await;
    launch(&emulator, "info-present", SINGLE_INSTANCE, "present").await;

    struct Refusal {
        name: &'static str,
        params: Vec<(&'static str, &'static str)>,
        code: &'static str,
        message: String,
    }
    let refusals = |action: &'static str| {
        vec![
            Refusal {
                name: "a name no environment answers to",
                params: vec![("EnvironmentName", "info-ghost"), ("InfoType", "tail")],
                code: "InvalidParameterValue",
                message: eb::state::missing_environment_message(None, Some("info-ghost")),
            },
            Refusal {
                name: "an id no environment answers to",
                params: vec![("EnvironmentId", "e-00000000000"), ("InfoType", "bundle")],
                code: "InvalidParameterValue",
                message: eb::state::missing_environment_message(Some("e-00000000000"), None),
            },
            Refusal {
                name: "neither identifier",
                params: vec![("InfoType", "analyze")],
                code: "MissingRequiredParameter",
                message: "Must specify either EnvironmentId or EnvironmentName.".to_string(),
            },
        ]
        .into_iter()
        .map(move |refusal| (action, refusal))
        .collect::<Vec<(&'static str, Refusal)>>()
    };

    for (action, refusal) in refusals("RequestEnvironmentInfo")
        .into_iter()
        .chain(refusals("RetrieveEnvironmentInfo"))
    {
        let case = format!("{action}: {}", refusal.name);
        let answer = emulator.call(action, &refusal.params).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            refusal.code,
            "case {case:?}: expected {}, got body {}",
            refusal.code,
            answer.body
        );
        assert_eq!(
            answer.error_message(),
            refusal.message,
            "case {case:?}: unexpected message in {}",
            answer.body
        );
    }
}

/// RULE: the information is compiled FROM the environment's instances, so an
/// environment whose instances this service has deleted — one shutting down or
/// already shut down — cannot have information requested for it.
#[tokio::test]
async fn information_cannot_be_requested_for_an_environment_that_is_shutting_down() {
    let emulator = Emulator::with_object_storage();
    application(&emulator).await;
    let environment = "info-terminated";
    launch(&emulator, environment, SINGLE_INSTANCE, "terminated").await;
    ok(
        &emulator,
        "terminated",
        "TerminateEnvironment",
        &[("EnvironmentName", environment)],
    )
    .await;

    // One row per status the shutdown passes through: the refusal holds while
    // the termination is in flight and once it has completed.
    for (name, wait) in [("Terminating", false), ("Terminated", true)] {
        if wait {
            std::thread::sleep(PAST_DUE);
        }
        let case = format!("request while {name}");
        let answer = emulator
            .call(
                "RequestEnvironmentInfo",
                &[("EnvironmentName", environment), ("InfoType", "tail")],
            )
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} and body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidParameterValue",
            "case {case:?}: unexpected code in {}",
            answer.body
        );
        assert_eq!(
            answer.error_message(),
            format!(
                "Environment {environment} is not in a state from which environment information \
                 can be requested. Current status: {name}."
            ),
            "case {case:?}: unexpected message in {}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// What the request records
// ---------------------------------------------------------------------------

/// RULE: a request records the activity it starts in the event log, and the
/// completion of the compile the clock owes is recorded under the SAME
/// `RequestId` once it comes due.
#[tokio::test]
async fn the_request_and_the_completion_the_clock_owes_are_recorded_as_one_activity() {
    let emulator = Emulator::with_object_storage();
    application(&emulator).await;
    let environment = "info-events";
    launch(&emulator, environment, SINGLE_INSTANCE, "events").await;

    ok(
        &emulator,
        "events",
        "RequestEnvironmentInfo",
        &[("EnvironmentName", environment), ("InfoType", "tail")],
    )
    .await;

    let starting = format!(
        "{} is starting.",
        eb::state::REQUEST_ENVIRONMENT_INFO_ACTIVITY
    );
    let completed = format!(
        "{} completed successfully.",
        eb::state::REQUEST_ENVIRONMENT_INFO_ACTIVITY
    );

    // Before the compile is due only the start is recorded.
    let logged = logged_activity(&emulator, environment, "events before due").await;
    let messages: Vec<&String> = logged.iter().map(|(message, _)| message).collect();
    assert!(
        messages.contains(&&starting) && !messages.contains(&&completed),
        "case {:?}: expected only {starting:?} in the log, got {logged:?}",
        "events before due"
    );

    std::thread::sleep(PAST_DUE);
    let logged = logged_activity(&emulator, environment, "events after due").await;
    let start_request_id = logged
        .iter()
        .find(|(message, _)| *message == starting)
        .map(|(_, request_id)| request_id.clone());
    let completion_request_id = logged
        .iter()
        .find(|(message, _)| *message == completed)
        .map(|(_, request_id)| request_id.clone());
    assert!(
        start_request_id.is_some() && start_request_id == completion_request_id,
        "case {:?}: the completion the clock owed must be recorded under the request's own \
         RequestId, got {logged:?}",
        "events after due"
    );
}

/// The `(Message, RequestId)` of every event `DescribeEvents` reports for
/// `environment`.
async fn logged_activity(
    emulator: &Emulator,
    environment: &str,
    case: &str,
) -> Vec<(String, String)> {
    let answer = ok(
        emulator,
        case,
        "DescribeEvents",
        &[("EnvironmentName", environment)],
    )
    .await;
    answer
        .elements_at("Events/member")
        .iter()
        .map(|event| {
            (
                event.value("Message").unwrap_or_default().to_string(),
                event.value("RequestId").unwrap_or_default().to_string(),
            )
        })
        .collect()
}
