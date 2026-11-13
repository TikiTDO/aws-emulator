// SPDX-License-Identifier: CC0-1.0

//! The sentence a `ResourceNotFoundException` carries, across the operations that refuse.
//!
//! RULE: when this service refuses a request because the resource the request named is not
//! held, it answers HTTP 404 `ResourceNotFoundException` whose `message` is one fixed
//! sentence — `Resource of type <resourceType> with id <resourceId> does not exist.`, the
//! trailing period included — with `<resourceType>` the kind the addressed path segment
//! spells and `<resourceId>` the id taken verbatim from the request. The sentence varies
//! with nothing else: not with the id's digits, not with which operation or HTTP method
//! addressed the missing resource, and not with whether the missing resource is the one the
//! path ends in or a parent level of it.
//!
//! The model documents the exception only as "The requested resource can't be found." and
//! enumerates no `resourceType` values (`smithy_query(query="shape",
//! shape="ResourceNotFoundException")` reports `httpError` 404 with `message`, `resourceId`
//! and `resourceType` all `@required`), so the wording recorded from the service is the
//! contract a client matches on, and a reworded sentence is a defect even when the status,
//! the code and the members are right.
//!
//! Where the sibling suite `resource_not_found_wording` spans the KINDS this service refuses
//! (one row per resource kind, read through its own path), this one spans the rest of what the
//! rule generalises over and shares that suite's expectation helper: several ids per kind
//! (all-zero, all-`f` and mixed hex suffixes), a resource read and the same kind of resource
//! deleted, a nested kind, and a list operation refused at each of its two parent levels. Every row asserts the whole refusal; then the suite blanks each row's own
//! type and id out of its own message and asserts every row left the SAME skeleton, which is
//! what makes "one fixed text, variable parts from the request" an assertion rather than a
//! handful of independent string comparisons.
//!
//! The paths are the modelled ones (`smithy_query(query="operation",
//! operation="GetFarm,DeleteFarm,GetQueue,DeleteQueue,GetJob,ListJobs,GetFleet,DeleteFleet,\
//! GetWorker,GetBudget,GetLimit")`), and the harness is the one the other suites drive this
//! service with, so every call goes through routing, deserialization, constraint validation,
//! the handler and serialization.

use std::collections::BTreeSet;

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::fleet::create_fleet;
use crate::resource_not_found_wording::not_found_message;

/// One row: the call that names a resource nothing holds, and the kind and id its 404 must
/// report.
struct Case {
    case: &'static str,
    method: &'static str,
    path: String,
    resource_type: &'static str,
    resource_id: String,
}

/// A 32-hex-character id suffix built by repeating one hex digit, so rows of the same kind
/// differ along the id's digits and nothing else.
fn repeated(digit: char) -> String {
    std::iter::repeat_n(digit, 32).collect()
}

/// The table: the refusals of this service, spanning the id values, the kinds, the HTTP
/// methods and the levels at which a path can miss.
async fn cases(emulator: &Emulator) -> Vec<Case> {
    let farm_id = emulator.farm("Template farm").await;
    let queue_id = emulator.queue(&farm_id, "Template queue").await;
    let fleet_id = create_fleet(emulator, &farm_id, "Template fleet").await;

    // Every id is well formed for its modelled `^<prefix>-[0-9a-f]{32}$` label pattern, so
    // the boundary validator passes the request and the handler is what decides the outcome.
    let farm_zeroes = absent_id("farm", &repeated('0'));
    let farm_effs = absent_id("farm", &repeated('f'));
    let farm_mixed = absent_id("farm", "0123456789abcdef0123456789abcdef");
    let farm_deleted_target = absent_id("farm", "fedcba98765432100123456789abcdef");
    let queue_zeroes = absent_id("queue", &repeated('0'));
    let queue_mixed = absent_id("queue", "00112233445566778899aabbccddeeff");
    let queue_delete_target = absent_id("queue", &repeated('e'));
    let job_zeroes = absent_id("job", &repeated('0'));
    let job_mixed = absent_id("job", "a0b1c2d3e4f50123456789abcdef0011");
    let fleet_mixed = absent_id("fleet", "1111222233334444555566667777888f");
    let fleet_delete_target = absent_id("fleet", &repeated('d'));
    let worker_mixed = absent_id("worker", "99887766554433221100ffeeddccbbaa");
    let budget_effs = absent_id("budget", &repeated('f'));
    let limit_zeroes = absent_id("limit", &repeated('0'));

    vec![
        // The same kind, three different ids: the id is echoed, the sentence around it does
        // not move.
        Case {
            case: "GetFarm, an all-zero id",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_zeroes}"),
            resource_type: "farm",
            resource_id: farm_zeroes,
        },
        Case {
            case: "GetFarm, an all-f id",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_effs}"),
            resource_type: "farm",
            resource_id: farm_effs,
        },
        Case {
            case: "GetFarm, a mixed-hex id",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_mixed}"),
            resource_type: "farm",
            resource_id: farm_mixed,
        },
        // A different method on the same path: a delete of something absent is refused with
        // the same sentence as a read of it.
        Case {
            case: "DeleteFarm, a mixed-hex id",
            method: "DELETE",
            path: format!("/2023-10-12/farms/{farm_deleted_target}"),
            resource_type: "farm",
            resource_id: farm_deleted_target,
        },
        Case {
            case: "GetQueue, an all-zero id under a farm that exists",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_id}/queues/{queue_zeroes}"),
            resource_type: "queue",
            resource_id: queue_zeroes,
        },
        Case {
            case: "GetQueue, a mixed-hex id under a farm that exists",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_id}/queues/{queue_mixed}"),
            resource_type: "queue",
            resource_id: queue_mixed,
        },
        Case {
            case: "DeleteQueue, an absent queue under a farm that exists",
            method: "DELETE",
            path: format!("/2023-10-12/farms/{farm_id}/queues/{queue_delete_target}"),
            resource_type: "queue",
            resource_id: queue_delete_target,
        },
        // Two levels down: the kind reported is the one the path ENDS in while its parents
        // are held.
        Case {
            case: "GetJob, an all-zero id under a queue that exists",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{job_zeroes}"),
            resource_type: "job",
            resource_id: job_zeroes,
        },
        Case {
            case: "GetJob, a mixed-hex id under a queue that exists",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{job_mixed}"),
            resource_type: "job",
            resource_id: job_mixed,
        },
        Case {
            case: "GetFleet, a mixed-hex id under a farm that exists",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_id}/fleets/{fleet_mixed}"),
            resource_type: "fleet",
            resource_id: fleet_mixed,
        },
        Case {
            case: "DeleteFleet, an absent fleet under a farm that exists",
            method: "DELETE",
            path: format!("/2023-10-12/farms/{farm_id}/fleets/{fleet_delete_target}"),
            resource_type: "fleet",
            resource_id: fleet_delete_target,
        },
        Case {
            case: "GetWorker, an absent worker in a fleet that exists",
            method: "GET",
            path: format!(
                "/2023-10-12/farms/{farm_id}/fleets/{fleet_id}/workers/{worker_mixed}"
            ),
            resource_type: "worker",
            resource_id: worker_mixed,
        },
        Case {
            case: "GetBudget, an all-f id under a farm that exists",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_id}/budgets/{budget_effs}"),
            resource_type: "budget",
            resource_id: budget_effs,
        },
        Case {
            case: "GetLimit, an all-zero id under a farm that exists",
            method: "GET",
            path: format!("/2023-10-12/farms/{farm_id}/limits/{limit_zeroes}"),
            resource_type: "limit",
            resource_id: limit_zeroes,
        },
        // A collection operation, refused at each of the two parent levels its path names:
        // the kind and id reported are the level that is missing, not the collection.
        Case {
            case: "ListJobs, whose queue is absent",
            method: "GET",
            path: format!(
                "/2023-10-12/farms/{farm_id}/queues/{queue}/jobs",
                queue = absent_id("queue", &repeated('a'))
            ),
            resource_type: "queue",
            resource_id: absent_id("queue", &repeated('a')),
        },
        Case {
            case: "ListJobs, whose farm is absent",
            method: "GET",
            path: format!(
                "/2023-10-12/farms/{farm}/queues/{queue_id}/jobs",
                farm = absent_id("farm", &repeated('b'))
            ),
            resource_type: "farm",
            resource_id: absent_id("farm", &repeated('b')),
        },
    ]
}

#[tokio::test]
async fn every_refusal_words_the_one_sentence_with_the_request_s_kind_and_id() {
    let emulator = Emulator::new();

    // The sentence with each row's own type and id blanked out. One distinct entry means
    // every row shares the same fixed text.
    let mut skeletons: BTreeSet<String> = BTreeSet::new();

    for case in cases(&emulator).await {
        let (status, body) = emulator.call(case.method, &case.path, None, None).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected 404, got {status} with {body}",
            case.case
        );
        assert_eq!(
            (
                error_code(&body),
                string_at(&body, "resourceType"),
                string_at(&body, "resourceId"),
                string_at(&body, "message"),
            ),
            (
                "ResourceNotFoundException".to_string(),
                case.resource_type.to_string(),
                case.resource_id.clone(),
                not_found_message(case.resource_type, &case.resource_id),
            ),
            "case {:?}: expected the 404 to name the missing kind and id in AWS's sentence, \
             got {body}",
            case.case
        );

        // The message is rendered from the very members the response reports, so the two
        // cannot drift apart: a client that reads `resourceType`/`resourceId` and a client
        // that matches the sentence see the same thing.
        let message = string_at(&body, "message");
        assert_eq!(
            message,
            not_found_message(
                &string_at(&body, "resourceType"),
                &string_at(&body, "resourceId")
            ),
            "case {:?}: expected the sentence to be rendered from the reported members, got \
             {message:?}",
            case.case
        );

        skeletons.insert(
            message
                .replace(&case.resource_id, "<resourceId>")
                .replace(case.resource_type, "<resourceType>"),
        );
    }

    assert_eq!(
        skeletons.iter().cloned().collect::<Vec<String>>(),
        vec!["Resource of type <resourceType> with id <resourceId> does not exist.".to_string()],
        "expected one fixed text across every refusal, got {skeletons:?}"
    );
}

/// The id is echoed VERBATIM: an id that is well formed for its label pattern but names
/// nothing comes back character for character, so the sentence a client matches carries the
/// id that client sent.
#[tokio::test]
async fn the_reported_id_is_the_one_the_request_named() {
    let emulator = Emulator::new();

    // Rows differ along the id's digits only; each is a valid `^farm-[0-9a-f]{32}$` label.
    let suffixes = [
        repeated('0'),
        repeated('f'),
        "0123456789abcdef0123456789abcdef".to_string(),
        "fedcba9876543210fedcba9876543210".to_string(),
        "00000000000000000000000000000001".to_string(),
    ];

    for suffix in &suffixes {
        let farm_id = absent_id("farm", suffix);
        let (status, body) = emulator.get(&format!("/2023-10-12/farms/{farm_id}")).await;
        assert_eq!(
            status, 404,
            "case {farm_id:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            string_at(&body, "resourceId"),
            farm_id,
            "case {farm_id:?}: expected the id echoed verbatim, got {body}"
        );
        assert!(
            string_at(&body, "message").contains(&farm_id),
            "case {farm_id:?}: expected the sentence to carry the id the request named, got \
             {body}"
        );
    }
}
