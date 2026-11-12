// SPDX-License-Identifier: CC0-1.0

//! The Braket hybrid-job loop, end to end over HTTP.
//!
//! `CreateJob` writes the one record `GetJob` answers from, so these suites pin the
//! rules that pair governs:
//!
//! * a `jobArn` the store does not hold is a MISSING RESOURCE (404
//!   `ResourceNotFoundException`), while one outside the modelled `JobArn` `@pattern`
//!   is invalid INPUT (400 `ValidationException`) — the two are never swapped;
//! * every member `GetJob` reports comes from the stored record: what the create
//!   supplied reads back unchanged, and the status, timestamps, events and queue
//!   position are computed from it;
//! * a job is returned in the non-terminal `QUEUED` status and reaches `RUNNING` and
//!   then its terminal status at the instants the clock owes;
//! * `clientToken` is an `@idempotencyToken`, and a job's ARN carries its name, so a
//!   repeated token replays one job and a repeated name is a conflict.
//!
//! Each rule is asserted over a TABLE spanning the class it generalises over (the part
//! of the ARN grammar a value misses, the shape of the job being created, the device
//! status, the attribute asked for), not over one recorded call. Everything goes over
//! HTTP through the production router, reusing the harness in `quantum_task_loop.rs`.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::quantum_task_loop::{encode, member, Braket, Reply};

/// The region the harness's SigV4 credential scope names, so a minted ARN carries it.
const REGION: &str = "eu-west-1";

/// The account id every emulated ARN carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Slack between measuring a job's age in the test and the handler reading the clock
/// for it: inside this band both neighbouring statuses are owed, so both are accepted.
const CLOCK_GUARD: Duration = Duration::from_millis(150);

/// Values that are not `JobArn`s, one row per part of the modelled `@pattern`
/// (`^arn:aws[a-z\-]*:braket:[a-z0-9\-]+:[0-9]{12}:job/.*$`) a value can miss.
/// `(case, value)`.
const MALFORMED_JOB_ARNS: &[(&str, &str)] = &[
    ("not an ARN at all", "not-a-valid-job-arn"),
    ("a bare job name", "my-job"),
    ("the arn prefix in upper case", "ARN:aws:braket:us-east-1:000000000000:job/j1"),
    ("too few fields", "arn:aws:braket:us-east-1:000000000000"),
    (
        "a partition outside the aws family",
        "arn:azure:braket:us-east-1:000000000000:job/j1",
    ),
    ("another service's namespace", "arn:aws:s3:us-east-1:000000000000:job/j1"),
    (
        "another Braket resource type",
        "arn:aws:braket:us-east-1:000000000000:quantum-task/t1",
    ),
    (
        "a resource type this one is only a prefix of",
        "arn:aws:braket:us-east-1:000000000000:jobs/j1",
    ),
    ("no resource type separator", "arn:aws:braket:us-east-1:000000000000:job"),
    (
        "an account id shorter than 12 digits",
        "arn:aws:braket:us-east-1:12345:job/j1",
    ),
    (
        "an account id that is not all digits",
        "arn:aws:braket:us-east-1:00000000000a:job/j1",
    ),
    ("an empty region", "arn:aws:braket::000000000000:job/j1"),
    (
        "a region in upper case",
        "arn:aws:braket:US-EAST-1:000000000000:job/j1",
    ),
];

/// Well-formed `JobArn`s this emulator holds no record for: the shape of the value
/// varies (partition, region, account, name form), the answer does not.
/// `(case, value, identifier)`, where the identifier is how the refusal names the job
/// it could not find: the account and the name the ARN itself carries.
const ABSENT_JOB_ARNS: &[(&str, &str, &str)] = &[
    // The class the recorded request belongs to: the form this emulator itself mints.
    (
        "the form this emulator mints",
        "arn:aws:braket:eu-west-1:000000000000:job/no-such-job",
        "000000000000#no-such-job",
    ),
    (
        "the China partition",
        "arn:aws-cn:braket:cn-north-1:123456789012:job/j1",
        "123456789012#j1",
    ),
    (
        "the GovCloud partition",
        "arn:aws-us-gov:braket:us-gov-west-1:210987654321:job/other-job",
        "210987654321#other-job",
    ),
    (
        "a name carrying slashes",
        "arn:aws:braket:us-east-1:000000000000:job/nested/name",
        "000000000000#nested/name",
    ),
    (
        "an empty name, which the pattern permits",
        "arn:aws:braket:us-east-1:000000000000:job/",
        "000000000000#",
    ),
];

/// The members `GetJob` must report exactly as the create supplied them. Each one is a
/// member both `CreateJobRequest` and `GetJobResponse` declare
/// (`smithy_query(query="operation",operation="CreateJob,GetJob")`).
const ECHOED_MEMBERS: &[&str] = &[
    "jobName",
    "roleArn",
    "algorithmSpecification",
    "inputDataConfig",
    "outputDataConfig",
    "checkpointConfig",
    "stoppingCondition",
    "instanceConfig",
    "hyperParameters",
    "deviceConfig",
    "tags",
    "associations",
];

/// A `CreateJob` body with only the required members filled in.
pub(crate) fn job_body(client_token: &str, job_name: &str, device_arn: &str) -> Value {
    json!({
        "clientToken": client_token,
        "jobName": job_name,
        "roleArn": "arn:aws:iam::123456789012:role/BraketJobsRole",
        "algorithmSpecification": {
            "containerImage": {
                "uri": "000000000000.dkr.ecr.eu-west-1.amazonaws.com/braket:latest"
            }
        },
        "outputDataConfig": { "s3Path": "s3://amazon-braket-emulator/jobs" },
        "instanceConfig": { "instanceType": "ml.m5.large", "volumeSizeInGb": 30 },
        "deviceConfig": { "device": device_arn },
    })
}

/// The ARN `CreateJob` mints for `job_name` in this harness's credential scope.
fn job_arn(job_name: &str) -> String {
    format!("arn:aws:braket:{REGION}:{ACCOUNT}:job/{job_name}")
}

impl Braket {
    pub(crate) async fn create_job(&self, body: Value) -> Reply {
        self.call("POST", "/job", Some(body)).await
    }

    pub(crate) async fn get_job(&self, arn: &str) -> Reply {
        self.call("GET", &format!("/job/{}", encode(arn)), None).await
    }

    async fn get_job_with_queue_info(&self, arn: &str) -> Reply {
        self.call(
            "GET",
            &format!("/job/{}?additionalAttributeNames=QueueInfo", encode(arn)),
            None,
        )
        .await
    }

    /// The ARN of a seeded device in `status`.
    async fn a_device_with_status(&self, status: &str) -> String {
        let inventory = self.inventory().await;
        let device = inventory
            .iter()
            .find(|d| member(d, "deviceStatus") == status)
            .unwrap_or_else(|| panic!("the seeded inventory holds a {status} device"));
        member(device, "deviceArn").to_string()
    }
}

/// The statuses the clock owes a job of measured age `age`. The age is measured BEFORE
/// the read, so the job's age at the handler's own clock read is `age` or a little more.
fn owed_statuses(age: Duration) -> Vec<&'static str> {
    use service_com_amazonaws_braket_braket::state::{JOB_RUNNING_SPAN, JOB_TERMINAL_SPAN};

    let mut owed: Vec<&str> = Vec::new();
    if age < JOB_RUNNING_SPAN {
        owed.push("QUEUED");
    }
    if age + CLOCK_GUARD >= JOB_RUNNING_SPAN && age < JOB_TERMINAL_SPAN {
        owed.push("RUNNING");
    }
    if age + CLOCK_GUARD >= JOB_TERMINAL_SPAN {
        owed.push("FAILED");
    }
    owed
}

/// Assert the job `reply` reports a status the model declares, and that it is one the
/// clock owes at `age`.
fn assert_status_owed(reply: &Reply, age: Duration, case: &str) {
    use service_com_amazonaws_braket_braket::ops::types::JOB_PRIMARY_STATUS_VALUES;

    let status = member(&reply.body, "status");
    assert!(
        JOB_PRIMARY_STATUS_VALUES.contains(&status),
        "case {case}: expected a modelled JobPrimaryStatus, got {status:?} \
         (the model declares {JOB_PRIMARY_STATUS_VALUES:?})"
    );
    let owed = owed_statuses(age);
    assert!(
        owed.contains(&status),
        "case {case} ({age:?} after the create returned): expected the status the clock \
         owes, one of {owed:?}, got {status}"
    );
}

/// `GetJob`: a `jobArn` outside the modelled pattern is invalid input, and only a
/// well-formed one naming no record is a missing resource.
#[tokio::test]
async fn get_job_separates_a_malformed_arn_from_an_absent_job() {
    let braket = Braket::new();

    for (case, value) in MALFORMED_JOB_ARNS {
        let reply = braket.get_job(value).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a value outside the JobArn pattern, \
             got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} ({})",
            reply.error,
            reply.body
        );
    }

    for (case, value, identifier) in ABSENT_JOB_ARNS {
        let reply = braket.get_job(value).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected HTTP 404 for a well-formed jobArn naming no job, \
             got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} ({})",
            reply.error,
            reply.body
        );
        // The refusal is built from the REQUEST: Braket names a hybrid job by the
        // account it belongs to and its name, both read out of the ARN sent.
        assert_eq!(
            reply.body.get("message").and_then(Value::as_str),
            Some(format!("Job {identifier} not found").as_str()),
            "case {case:?}: expected the message to name the job the request named, \
             got {}",
            reply.body
        );
    }
}

/// Every member a create supplies reads back through `GetJob`, over a table of job
/// shapes: the required members alone, a script-mode algorithm, every optional member
/// at once, and a multi-instance configuration on a QPU.
#[tokio::test]
async fn a_created_job_reads_back_through_get_job() {
    let braket = Braket::new();
    let simulator = braket.a_device_with_status("ONLINE").await;
    let qpu = {
        let inventory = braket.inventory().await;
        let device = inventory
            .iter()
            .find(|d| member(d, "deviceType") == "QPU" && member(d, "deviceStatus") == "ONLINE")
            .expect("the seeded inventory holds an ONLINE QPU");
        member(device, "deviceArn").to_string()
    };

    // (case, body) — rows differ in the algorithm form, which optional members are
    // present, the instance configuration and the device kind.
    let mut rows: Vec<(&str, Value)> = Vec::new();

    rows.push((
        "the required members only",
        job_body("tok-minimal", "job-minimal", &simulator),
    ));

    let mut body = job_body("tok-script-mode", "job-script-mode", &simulator);
    body["algorithmSpecification"] = json!({
        "scriptModeConfig": {
            "entryPoint": "my_module:start_here",
            "s3Uri": "s3://amazon-braket-emulator/source.tar.gz",
            "compressionType": "GZIP",
        }
    });
    rows.push(("a script-mode algorithm", body));

    let mut body = job_body("tok-everything", "job-everything-0", &simulator);
    body["hyperParameters"] = json!({ "shots": "100", "stepSize": "0.05" });
    body["inputDataConfig"] = json!([{
        "channelName": "training",
        "contentType": "application/json",
        "dataSource": { "s3DataSource": { "s3Uri": "s3://amazon-braket-emulator/input" } },
    }]);
    body["checkpointConfig"] = json!({
        "localPath": "/opt/braket/checkpoints",
        "s3Uri": "s3://amazon-braket-emulator/checkpoints",
    });
    body["stoppingCondition"] = json!({ "maxRuntimeInSeconds": 3_600 });
    body["outputDataConfig"] = json!({
        "s3Path": "s3://amazon-braket-emulator/jobs",
        "kmsKeyId": "arn:aws:kms:eu-west-1:000000000000:key/abcd",
    });
    body["tags"] = json!({ "team": "quantum", "env": "test" });
    body["associations"] = json!([{
        "arn": format!("arn:aws:braket:{REGION}:{ACCOUNT}:reservation/abc"),
        "type": "RESERVATION_TIME_WINDOW_ARN",
    }]);
    rows.push(("every optional member at once", body));

    let mut body = job_body("tok-qpu", "job-on-a-qpu", &qpu);
    body["instanceConfig"] = json!({
        "instanceType": "ml.c5.xlarge",
        "volumeSizeInGb": 100,
        "instanceCount": 4,
    });
    rows.push(("four instances of another type on a QPU", body));

    for (case, body) in rows {
        let created = braket.create_job(body.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201 Created, got {} {}",
            created.status, created.body
        );
        let arn = created.str_member("jobArn");
        let expected = job_arn(member(&body, "jobName"));
        assert_eq!(
            arn, expected,
            "case {case:?}: expected the minted ARN to carry the job name in this \
             request's credential scope, got {arn}"
        );

        let accepted_at = Instant::now();
        let read = braket.get_job(&arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 reading the job back, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.str_member("jobArn"),
            arn,
            "case {case:?}: expected the ARN to read back, got {}",
            read.body
        );
        for name in ECHOED_MEMBERS {
            assert_eq!(
                read.body.get(*name),
                body.get(*name),
                "case {case:?}: expected {name} to read back as the create supplied it, \
                 got {:?}",
                read.body.get(*name)
            );
        }
        assert_status_owed(&read, accepted_at.elapsed(), case);
        assert!(
            read.body.get("createdAt").is_some_and(|at| !at.is_null()),
            "case {case:?}: expected a createdAt timestamp, got {}",
            read.body
        );
        // Nothing has executed, so no artifact-bearing member is reported.
        assert!(
            read.body.get("billableDuration").is_none(),
            "case {case:?}: nothing ran this job, so it has no billable duration, got {}",
            read.body
        );
        // `queueInfo` is reported only when the caller asks for it.
        assert!(
            read.body.get("queueInfo").is_none(),
            "case {case:?}: queueInfo must be withheld unless QueueInfo is requested, \
             got {}",
            read.body
        );
    }
}

/// The statuses after `QUEUED` are owed by the CLOCK, not by any call: a job observed
/// immediately is queued, and later reads report the status the elapsed time owes, with
/// the timestamps and the event history that go with it.
#[tokio::test]
async fn a_job_reaches_the_statuses_the_clock_owes() {
    use service_com_amazonaws_braket_braket::ops::types::JOB_EVENT_TYPE_VALUES;
    use service_com_amazonaws_braket_braket::state::{JOB_RUNNING_SPAN, JOB_TERMINAL_SPAN};

    let braket = Braket::new();
    let device = braket.a_device_with_status("ONLINE").await;
    let created = braket
        .create_job(job_body("tok-clock", "job-clock", &device))
        .await;
    assert_eq!(
        created.status, 201,
        "case create: expected HTTP 201, got {} {}",
        created.status, created.body
    );
    // The job's age is measured from just after the create returned, so every
    // assertion below is a function of the elapsed time rather than of a fixed sleep.
    let accepted_at = Instant::now();
    let arn = created.str_member("jobArn");

    let queued = braket.get_job(&arn).await;
    assert_status_owed(&queued, accepted_at.elapsed(), "immediately after create");
    if member(&queued.body, "status") == "QUEUED" {
        assert!(
            queued.body.get("startedAt").is_none() && queued.body.get("endedAt").is_none(),
            "case immediately after create: a queued job has neither started nor ended, \
             got {}",
            queued.body
        );
    }

    while accepted_at.elapsed() < JOB_RUNNING_SPAN + CLOCK_GUARD {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let running = braket.get_job(&arn).await;
    let age = accepted_at.elapsed();
    assert_status_owed(&running, age, "past the running instant");
    if member(&running.body, "status") == "RUNNING" {
        assert!(
            running.body.get("startedAt").is_some_and(|at| !at.is_null()),
            "case past the running instant: a running job reports when it started, got {}",
            running.body
        );
        assert!(
            running.body.get("endedAt").is_none(),
            "case past the running instant: a running job has not ended, got {}",
            running.body
        );
    }

    while accepted_at.elapsed() < JOB_TERMINAL_SPAN + CLOCK_GUARD {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let ended = braket.get_job(&arn).await;
    assert_eq!(
        member(&ended.body, "status"),
        "FAILED",
        "case past the terminal instant: nothing ran the algorithm, so the job ends in \
         the modelled failure status, got {}",
        ended.body
    );
    let failure_reason = member(&ended.body, "failureReason");
    assert!(
        !failure_reason.is_empty() && failure_reason.len() <= 1_024,
        "case past the terminal instant: expected the modelled String1024 reason the job \
         failed, got {failure_reason:?}"
    );
    assert!(
        ended.body.get("endedAt").is_some_and(|at| !at.is_null())
            && ended.body.get("startedAt").is_some_and(|at| !at.is_null()),
        "case past the terminal instant: a finished job reports when it started and \
         ended, got {}",
        ended.body
    );

    // The events are the transitions that HAPPENED, in order, each of a modelled type.
    let events = ended
        .body
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let types: Vec<&str> = events.iter().map(|e| member(e, "eventType")).collect();
    assert_eq!(
        types,
        vec!["QUEUED_FOR_EXECUTION", "RUNNING", "FAILED"],
        "case past the terminal instant: expected one event per transition performed, \
         got {types:?}"
    );
    for event in &events {
        let event_type = member(event, "eventType");
        assert!(
            JOB_EVENT_TYPE_VALUES.contains(&event_type),
            "case event {event_type:?}: expected a modelled JobEventType, the model \
             declares {JOB_EVENT_TYPE_VALUES:?}"
        );
        assert!(
            event.get("timeOfEvent").is_some_and(|at| !at.is_null()),
            "case event {event_type:?}: expected the time the event occurred, got {event}"
        );
    }

    // The terminal status is stable: the clock owes nothing more, and reading again
    // changes nothing a caller can see.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let again = braket.get_job(&arn).await;
    assert_eq!(
        again.body, ended.body,
        "case a second read: a reporting operation must not change what a later caller \
         sees, got {}",
        again.body
    );
}

/// `clientToken` is an `@idempotencyToken`, and a job's ARN carries its NAME: the same
/// token with the same request replays one job, the same token with a different request
/// is refused, and a second job of an existing name is a conflict.
#[tokio::test]
async fn a_repeated_token_replays_one_job_and_a_repeated_name_conflicts() {
    let braket = Braket::new();
    let device = braket.a_device_with_status("ONLINE").await;

    let body = job_body("tok-replay", "job-replay", &device);
    let first = braket.create_job(body.clone()).await;
    assert_eq!(
        first.status, 201,
        "case first create: expected HTTP 201, got {} {}",
        first.status, first.body
    );
    let arn = first.str_member("jobArn");

    let replay = braket.create_job(body.clone()).await;
    assert_eq!(
        replay.status, 201,
        "case replayed token: expected HTTP 201, got {} {}",
        replay.status, replay.body
    );
    assert_eq!(
        replay.str_member("jobArn"),
        arn,
        "case replayed token: expected the first ARN back, got {}",
        replay.body
    );

    // One token, a different request: refused rather than answered with the first ARN.
    // Each row changes ONE member of the replayed body.
    let mut rows: Vec<(&str, Value)> = Vec::new();
    for (case, member_name, value) in [
        ("a different job name", "jobName", json!("job-replay-other")),
        (
            "a different role",
            "roleArn",
            json!("arn:aws:iam::123456789012:role/OtherRole"),
        ),
        (
            "a different instance configuration",
            "instanceConfig",
            json!({ "instanceType": "ml.m5.xlarge", "volumeSizeInGb": 30 }),
        ),
        (
            "added hyper parameters",
            "hyperParameters",
            json!({ "shots": "10" }),
        ),
    ] {
        let mut changed = body.clone();
        changed[member_name] = value;
        rows.push((case, changed));
    }
    for (case, changed) in rows {
        let reply = braket.create_job(changed).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a token reused for another request, \
             got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} ({})",
            reply.error,
            reply.body
        );
    }

    // A fresh token for a name that already exists names the SAME resource: a conflict.
    let mut same_name = body.clone();
    same_name["clientToken"] = json!("tok-replay-second");
    let conflict = braket.create_job(same_name).await;
    assert_eq!(
        conflict.status, 409,
        "case a second job of an existing name: expected HTTP 409, got {} {}",
        conflict.status, conflict.body
    );
    assert_eq!(
        conflict.error.as_deref(),
        Some("ConflictException"),
        "case a second job of an existing name: expected ConflictException, got {:?} ({})",
        conflict.error,
        conflict.body
    );
    // And the first job is untouched by either refusal.
    let read = braket.get_job(&arn).await;
    assert_eq!(
        read.status, 200,
        "case the first job after the refusals: expected HTTP 200, got {} {}",
        read.status, read.body
    );
}

/// `queueInfo` is reported only when `additionalAttributeNames` asks for it, and the
/// position it reports is counted over the jobs that are actually queued.
#[tokio::test]
async fn queue_info_is_reported_only_when_it_is_asked_for() {
    use service_com_amazonaws_braket_braket::ops::types::QUEUE_NAME_VALUES;
    use service_com_amazonaws_braket_braket::state::{JOB_TERMINAL_SPAN, QUEUE_POSITION_NOT_QUEUED};

    let braket = Braket::new();
    let device = braket.a_device_with_status("ONLINE").await;

    // Three jobs, created in order: the queue positions are 1, 2, 3 while they wait.
    let mut arns: Vec<String> = Vec::new();
    for index in 0..3 {
        let reply = braket
            .create_job(job_body(
                &format!("tok-queue-{index}"),
                &format!("job-queue-{index}"),
                &device,
            ))
            .await;
        assert_eq!(
            reply.status, 201,
            "case create {index}: expected HTTP 201, got {} {}",
            reply.status, reply.body
        );
        arns.push(reply.str_member("jobArn"));
    }

    for (index, arn) in arns.iter().enumerate() {
        let reply = braket.get_job_with_queue_info(arn).await;
        assert_eq!(
            reply.status, 200,
            "case queued job {index}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        let info = reply
            .body
            .get("queueInfo")
            .unwrap_or_else(|| panic!("case queued job {index}: expected queueInfo in {}", reply.body));
        let queue = member(info, "queue");
        assert!(
            QUEUE_NAME_VALUES.contains(&queue) && queue == "JOBS_QUEUE",
            "case queued job {index}: a hybrid job waits in the jobs queue, got {queue:?}"
        );
        let position = member(info, "position");
        if member(&reply.body, "status") == "QUEUED" {
            assert_eq!(
                position,
                (index + 1).to_string(),
                "case queued job {index}: expected its place among the queued jobs, got \
                 {position:?}"
            );
        }
    }

    // Once a job is no longer waiting, it has no position: the sentinel and a message
    // say so rather than a number claiming it still waits.
    tokio::time::sleep(JOB_TERMINAL_SPAN + CLOCK_GUARD).await;
    for (index, arn) in arns.iter().enumerate() {
        let reply = braket.get_job_with_queue_info(arn).await;
        let info = reply
            .body
            .get("queueInfo")
            .unwrap_or_else(|| panic!("case finished job {index}: expected queueInfo in {}", reply.body));
        assert_eq!(
            member(info, "position"),
            QUEUE_POSITION_NOT_QUEUED,
            "case finished job {index}: expected the not-queued sentinel, got {info}"
        );
        assert!(
            info.get("message").and_then(Value::as_str).is_some(),
            "case finished job {index}: expected a message saying why there is no \
             position, got {info}"
        );
    }
}

/// `deviceConfig.device` is held to the device inventory: a value that is not a device
/// ARN is invalid input, a device the inventory does not publish is too (this operation
/// models no 404), and a device that cannot run work is answered with the modelled
/// device error for its status.
#[tokio::test]
async fn create_job_holds_the_device_config_to_the_inventory() {
    let braket = Braket::new();

    // (case, value) — one row per part of the ARN grammar a value can miss.
    let malformed: &[(&str, &str)] = &[
        ("not an ARN at all", "not-an-arn"),
        ("a bare device name", "sv1"),
        ("another service's namespace", "arn:aws:s3:::device/quantum-simulator/amazon/sv1"),
        (
            "another Braket resource type",
            "arn:aws:braket:us-east-1:000000000000:quantum-task/t1",
        ),
        ("an empty resource id", "arn:aws:braket:::device/"),
        (
            "a partition outside the aws family",
            "arn:gcp:braket:::device/quantum-simulator/amazon/sv1",
        ),
    ];
    for (index, (case, value)) in malformed.iter().enumerate() {
        let reply = braket
            .create_job(job_body(
                &format!("tok-device-bad-{index}"),
                &format!("job-device-bad-{index}"),
                value,
            ))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400 for a value that is not a device ARN, \
             got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} ({})",
            reply.error,
            reply.body
        );
        let message = reply
            .body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains("deviceConfig.device"),
            "case {case:?}: expected the message to name the member, got {message:?}"
        );
    }

    // A well-formed ARN the inventory does not publish.
    let absent = braket
        .create_job(job_body(
            "tok-device-absent",
            "job-device-absent",
            "arn:aws:braket:us-east-1::device/qpu/nobody/Nothing",
        ))
        .await;
    assert_eq!(
        absent.status, 400,
        "case a device the inventory does not publish: expected HTTP 400, got {} {}",
        absent.status, absent.body
    );

    // (case, device status, expected status, expected error) — the modelled device
    // errors, one row per status a device can be in that cannot take work.
    let rows: &[(&str, &str, u16, &str)] = &[
        ("a retired device", "RETIRED", 410, "DeviceRetiredException"),
        ("an offline device", "OFFLINE", 424, "DeviceOfflineException"),
    ];
    for (index, (case, device_status, status, error)) in rows.iter().enumerate() {
        let device = braket.a_device_with_status(device_status).await;
        let reply = braket
            .create_job(job_body(
                &format!("tok-device-status-{index}"),
                &format!("job-device-status-{index}"),
                &device,
            ))
            .await;
        assert_eq!(
            reply.status, *status,
            "case {case:?} ({device}): expected HTTP {status}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some(*error),
            "case {case:?} ({device}): expected {error}, got {:?} ({})",
            reply.error,
            reply.body
        );
        // Nothing was created, so the ARN that create would have minted resolves to
        // nothing.
        let read = braket.get_job(&job_arn(&format!("job-device-status-{index}"))).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create writes no job, got {} {}",
            read.status, read.body
        );
    }
}

// ---------------------------------------------------------------------------
// CancelJob
// ---------------------------------------------------------------------------

impl Braket {
    async fn cancel_job(&self, arn: &str) -> Reply {
        self.call("PUT", &format!("/job/{}/cancel", encode(arn)), None)
            .await
    }
}

/// What may be cancelled depends on the status the job is in: a job still in flight is
/// accepted and moves to `CANCELLING`, the clock then owes `CANCELLED`, a job that
/// reached a terminal status on its own is refused with `ConflictException` (409), and a
/// well-formed ARN no job is held for is the modelled missing resource (404).
#[tokio::test]
async fn cancel_job_moves_the_record_and_conflicts_on_a_finished_job() {
    use service_com_amazonaws_braket_braket::state::{CANCELLATION_SPAN, JOB_TERMINAL_SPAN};

    let braket = Braket::new();
    let device = braket.a_device_with_status("ONLINE").await;
    let created = braket
        .create_job(job_body("tok-cancel", "job-cancel", &device))
        .await;
    assert_eq!(
        created.status, 201,
        "case create: expected HTTP 201, got {} {}",
        created.status, created.body
    );
    let arn = created.str_member("jobArn");

    let cancelled = braket.cancel_job(&arn).await;
    assert_eq!(
        cancelled.status, 200,
        "case cancel a queued job: expected 200, got {} {}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        member(&cancelled.body, "jobArn"),
        arn,
        "case cancel a queued job: the ARN must come back, got {}",
        cancelled.body
    );
    assert_eq!(
        member(&cancelled.body, "cancellationStatus"),
        "CANCELLING",
        "case cancel a queued job: expected CANCELLING, got {}",
        cancelled.body
    );
    assert_eq!(
        member(&braket.get_job(&arn).await.body, "status"),
        "CANCELLING",
        "case after cancel: GetJob must report the cancellation"
    );

    // `@idempotent`: cancelling again reports where the cancellation has got to.
    let again = braket.cancel_job(&arn).await;
    assert_eq!(
        again.status, 200,
        "case re-cancel: expected 200, got {} {}",
        again.status, again.body
    );
    assert!(
        matches!(
            member(&again.body, "cancellationStatus"),
            "CANCELLING" | "CANCELLED"
        ),
        "case re-cancel: expected a cancellation status, got {}",
        again.body
    );

    // The CANCELLING -> CANCELLED transition is owed by the clock.
    tokio::time::sleep(CANCELLATION_SPAN + CLOCK_GUARD).await;
    let terminal = braket.get_job(&arn).await;
    assert_eq!(
        member(&terminal.body, "status"),
        "CANCELLED",
        "case past the cancellation span: expected CANCELLED, got {}",
        terminal.body
    );
    assert!(
        terminal.body.get("endedAt").is_some_and(|at| !at.is_null()),
        "case a cancelled job: endedAt must be set, got {}",
        terminal.body
    );

    // A job that finished without being cancelled can no longer be cancelled.
    let other = braket
        .create_job(job_body("tok-finished", "job-finished", &device))
        .await;
    let finished_arn = other.str_member("jobArn");
    tokio::time::sleep(JOB_TERMINAL_SPAN + CLOCK_GUARD).await;
    let finished = braket.get_job(&finished_arn).await;
    assert_eq!(
        member(&finished.body, "status"),
        "FAILED",
        "case setup: the job must have reached a terminal status, got {}",
        finished.body
    );
    let conflict = braket.cancel_job(&finished_arn).await;
    assert_eq!(
        conflict.status, 409,
        "case cancel a finished job: expected 409, got {} {}",
        conflict.status, conflict.body
    );
    assert_eq!(
        conflict.error.as_deref(),
        Some("ConflictException"),
        "case cancel a finished job: expected ConflictException, got {:?}",
        conflict.error
    );
    assert_eq!(
        member(&braket.get_job(&finished_arn).await.body, "status"),
        "FAILED",
        "case after the refused cancel: the status must stand"
    );

    // A well-formed ARN no job is held for is a missing resource, over every form such
    // an ARN takes; a value that is not a `JobArn` at all is a 400 (asserted above).
    for (case, ghost, identifier) in ABSENT_JOB_ARNS {
        let reply = braket.cancel_job(ghost).await;
        assert_eq!(
            reply.status, 404,
            "case cancel {case}: expected 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error.as_deref(),
            Some("ResourceNotFoundException"),
            "case cancel {case}: expected ResourceNotFoundException, got {:?}",
            reply.error
        );
        assert_eq!(
            reply.message(),
            format!("Job {identifier} not found"),
            "case cancel {case}: expected the job named as the ARN carries it, got {:?}",
            reply.message()
        );
    }
}
