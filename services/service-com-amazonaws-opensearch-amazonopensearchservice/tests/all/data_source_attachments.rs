// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an OpenSearch application's data source attachments:
//! `AttachDataSource`, `ListDataSourceAttachments`, `DescribeDataSourceAttachment` and
//! `DetachDataSource`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`];
//! this suite adds no second copy of it.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * the listing reports exactly the attachment records the named application holds —
//!   every state included — each carrying the identifier, data source and status the
//!   attach persisted, over a table of applications differing in how many data sources
//!   they attach and in which optional members the attach carries;
//! * the same record is reported by the listing and by the single-attachment read, so
//!   the two cannot disagree;
//! * the listing pages the set: over a table of page sizes, the pages concatenate to the
//!   whole set in the listing's own order, with no record repeated or skipped, and the
//!   last page carries no token;
//! * that order is the order the attaches were made in, oldest first, whatever the data
//!   sources' ARNs sort as;
//! * a page size that is not one, and a token this service never issued, are refused
//!   with the modelled `ValidationException`;
//! * an attachment the clock owes a move settles without any call asking for it: PENDING
//!   while either the application or the data source is still being provisioned, then
//!   ATTACHED — or FAILED, where the data source is gone before it could complete;
//! * an attach is asynchronous, so an ARN that resolves to no data source of this
//!   account is accepted and recorded as PENDING and the clock settles it as FAILED,
//!   over a table of ARNs differing in why they resolve to nothing;
//! * a detached data source leaves the set, and a detach of a PENDING attachment or of a
//!   data source the application holds no record for is the modelled refusal;
//! * an attach is idempotent by data source and by a client token naming the same
//!   parameters, while a token reused for a different data source is the modelled
//!   conflict;
//! * an id naming no application is refused with the modelled
//!   `ResourceNotFoundException` by every one of the four operations.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    ATTACHMENT_PAGE, ATTACHMENT_QUOTA, ATTACHMENT_STATUS_ATTACHED, ATTACHMENT_STATUS_FAILED,
    ATTACHMENT_STATUS_PENDING,
};

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// `AttachDataSource`: `POST /2021-01-01/opensearch/application/{id}/attachDataSource`.
pub(crate) async fn attach(emulator: &Emulator, id: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application/{id}/attachDataSource"),
            Some(body),
        )
        .await
}

/// `DetachDataSource`: `POST /2021-01-01/opensearch/application/{id}/detachDataSource`.
async fn detach(emulator: &Emulator, id: &str, data_source_arn: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application/{id}/detachDataSource"),
            Some(json!({ "dataSourceArn": data_source_arn })),
        )
        .await
}

/// `DescribeDataSourceAttachment`:
/// `POST /2021-01-01/opensearch/application/{id}/describeDataSourceAttachment`.
pub(crate) async fn describe_attachment(
    emulator: &Emulator,
    id: &str,
    data_source_arn: &str,
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application/{id}/describeDataSourceAttachment"),
            Some(json!({ "dataSourceArn": data_source_arn })),
        )
        .await
}

/// `ListDataSourceAttachments`:
/// `POST /2021-01-01/opensearch/application/{id}/listDataSourceAttachments`.
pub(crate) async fn list_attachments(emulator: &Emulator, id: &str, body: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application/{id}/listDataSourceAttachments"),
            Some(body),
        )
        .await
}

/// An application of this account, answering with the id it was minted under.
pub(crate) async fn application(emulator: &Emulator, name: &str) -> String {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/application"),
            Some(json!({ "name": name })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case CreateApplication({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", name)
}

/// A domain of this account — the data source an attachment links an application to —
/// answering with its ARN.
async fn domain(emulator: &Emulator, name: &str) -> String {
    let answer = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        answer.status, 200,
        "case CreateDomain({name:?}): expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"]["ARN"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case CreateDomain({name:?}): expected the domain's ARN, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

/// `DeleteDomain`: the data source an attachment waits for goes away.
async fn delete_domain(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/{name}"), None)
        .await
}

/// One attachment as a listing or a read reports it: the three members both paths
/// carry, so a case compares the RECORD rather than one path's shape of it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Reported {
    attachment_id: String,
    data_source_arn: String,
    status: String,
}

/// What `entry` reports, with the case named on a member that is missing.
fn reported(entry: &Value, case: &str) -> Reported {
    Reported {
        attachment_id: string_member(entry, "attachmentId", case),
        data_source_arn: string_member(entry, "dataSourceArn", case),
        status: string_member(entry, "status", case),
    }
}

/// The attachments one answer of the listing reports, in the order it reported them.
fn page(answer: &Answer, case: &str) -> Vec<Reported> {
    answer.body["attachments"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected the listing to report an attachments list, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|entry| reported(entry, case))
        .collect()
}

/// Every attachment of `id`, read in ONE page large enough for the whole set.
async fn whole_set(emulator: &Emulator, id: &str, case: &str) -> Vec<Reported> {
    let answer = list_attachments(emulator, id, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200 from ListDataSourceAttachments, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.body["nextToken"],
        Value::Null,
        "case {case}: a page that ended the set carries no token, got {:?}",
        answer.body
    );
    page(&answer, case)
}

/// One row of [`the_listing_reports_every_attachment_of_the_application`]: an
/// application, how many data sources it attaches, and the optional members each attach
/// carries.
struct Row {
    case: &'static str,
    /// The application's name, which is also the stem of its data sources' names.
    name: &'static str,
    /// How many data sources this row attaches.
    sources: usize,
    /// The members the attach carries beyond `dataSourceArn`.
    extra: Value,
}

/// The table the round-trip rule is asserted over: one row per way an attach can differ
/// — how many data sources are attached, and which of the optional members
/// (`workspaceId`, `workspaceConfiguration`, `clientToken`) the call carries.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an application with no data source attached",
            name: "empty-ui",
            sources: 0,
            extra: json!({}),
        },
        Row {
            case: "one data source, no optional member",
            name: "single-ui",
            sources: 1,
            extra: json!({}),
        },
        Row {
            case: "three data sources",
            name: "triple-ui",
            sources: 3,
            extra: json!({}),
        },
        Row {
            case: "an attach naming an existing workspace",
            name: "workspace-id-ui",
            sources: 2,
            extra: json!({ "workspaceId": "ws-0123456789" }),
        },
        Row {
            case: "an attach naming a workspace to create",
            // `WorkspaceConfigurationInput$workspaceType`: "valid values are
            // OBSERVABILITY, SECURITY_ANALYTICS and SEARCH"
            // (`smithy_query(query="shape",shape="WorkspaceConfigurationInput")`), and
            // this row names one the issue did not cite.
            name: "workspace-new-ui",
            sources: 1,
            extra: json!({
                "workspaceConfiguration": { "name": "Security (prod)", "workspaceType": "SECURITY_ANALYTICS" },
            }),
        },
        Row {
            case: "an attach carrying a client token",
            name: "token-ui",
            sources: 2,
            extra: json!({ "clientToken": "attach-token-1" }),
        },
        Row {
            case: "five data sources, more than one page of two",
            name: "paged-ui",
            sources: 5,
            extra: json!({}),
        },
    ]
}

#[tokio::test]
async fn the_listing_reports_every_attachment_of_the_application() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let id = application(&emulator, row.name).await;
        let mut arns = Vec::new();
        for index in 0..row.sources {
            arns.push(domain(&emulator, &format!("{}-src{index}", row.name)).await);
        }
        // Both the application and every data source are up before the attaches, so
        // each attachment completes immediately: what this row asserts is the record,
        // and the clock moves have a case of their own below.
        tokio::time::sleep(SETTLE_WAIT).await;

        let mut expected: Vec<Reported> = Vec::new();
        for (index, arn) in arns.iter().enumerate() {
            let mut body = json!({ "dataSourceArn": arn });
            if let (Some(target), Some(extra)) = (body.as_object_mut(), row.extra.as_object()) {
                for (member, value) in extra {
                    // A client token identifies ONE attach, so a row attaching several
                    // data sources carries a token of its own per call.
                    let value = match value.as_str() {
                        Some(token) if member == "clientToken" => json!(format!("{token}-{index}")),
                        _ => value.clone(),
                    };
                    target.insert(member.clone(), value);
                }
            }
            let answer = attach(&emulator, &id, body.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 from AttachDataSource({body}), got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["id"],
                json!(id),
                "case {case}: the attach reports the application it was asked for, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.body["arn"],
                json!(format!(
                    "arn:aws:opensearch:us-east-1:{}:application/{id}",
                    engine::emulator_account_id()
                )),
                "case {case}: the attach reports the application's own ARN, got {:?}",
                answer.body
            );
            let minted = reported(&answer.body, case);
            assert_eq!(
                minted.data_source_arn, *arn,
                "case {case}: the attach reports the data source it was given, got {:?}",
                answer.body
            );
            assert_eq!(
                minted.status, ATTACHMENT_STATUS_ATTACHED,
                "case {case}: an attach of an active data source to an active application \
                 completes at once, got {:?}",
                answer.body
            );
            expected.push(minted);
        }

        // The listing reports exactly the records the attaches persisted — no more, no
        // fewer — each with the identifier, data source and status its attach answered
        // with.
        let listed = whole_set(&emulator, &id, case).await;
        let mut sorted_listed = listed.clone();
        sorted_listed.sort();
        let mut sorted_expected = expected.clone();
        sorted_expected.sort();
        assert_eq!(
            sorted_listed, sorted_expected,
            "case {case}: the listing reports the attachments the attaches persisted, got \
             {listed:?}"
        );

        // A read changes nothing a later caller sees: the second listing reports the
        // same records in the same order.
        let again = whole_set(&emulator, &id, case).await;
        assert_eq!(
            again, listed,
            "case {case}: two listings of one application report the same set in the same \
             order, got {again:?}"
        );

        // The single-attachment read reports the SAME record the listing does, so the
        // two paths cannot disagree about one attachment.
        for entry in &expected {
            let read = describe_attachment(&emulator, &id, &entry.data_source_arn).await;
            assert_eq!(
                read.status, 200,
                "case {case}: expected 200 from DescribeDataSourceAttachment({:?}), got {} with \
                 {:?}",
                entry.data_source_arn, read.status, read.body
            );
            assert_eq!(
                reported(&read.body, case),
                *entry,
                "case {case}: the read reports the attachment the listing does, got {:?}",
                read.body
            );
            assert_eq!(
                read.body["id"],
                json!(id),
                "case {case}: the read reports the application the attachment belongs to, got \
                 {:?}",
                read.body
            );
        }
    }
}

#[tokio::test]
async fn the_listing_pages_the_set_without_repeating_or_skipping_a_record() {
    let emulator = Emulator::new();
    let id = application(&emulator, "paging-ui").await;
    let mut arns = Vec::new();
    for index in 0..7 {
        arns.push(domain(&emulator, &format!("paging-src{index}")).await);
    }
    tokio::time::sleep(SETTLE_WAIT).await;
    for arn in &arns {
        let answer = attach(&emulator, &id, json!({ "dataSourceArn": arn })).await;
        assert_eq!(
            answer.status, 200,
            "case paging: expected 200 from AttachDataSource({arn:?}), got {} with {:?}",
            answer.status, answer.body
        );
    }

    let whole = whole_set(&emulator, &id, "paging").await;
    assert_eq!(
        whole.len(),
        arns.len(),
        "case paging: the set holds one attachment per attached data source, got {whole:?}"
    );

    // Every page size walks the SAME set in the same order: the one the request asks
    // for, the default a request that names none gets, and sizes at and past the size of
    // the set.
    for max_results in [1, 2, 3, 7, 8, 100] {
        let case = format!("paging with maxResults {max_results}");
        let mut walked: Vec<Reported> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "maxResults": max_results });
            if let (Some(target), Some(cursor)) = (body.as_object_mut(), token.as_deref()) {
                target.insert("nextToken".to_string(), json!(cursor));
            }
            let answer = list_attachments(&emulator, &id, body).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200 from ListDataSourceAttachments, got {} with {:?}",
                answer.status, answer.body
            );
            let reported = page(&answer, &case);
            assert!(
                reported.len() <= usize::try_from(max_results).unwrap_or(ATTACHMENT_PAGE),
                "case {case}: a page holds no more records than the request asked for, got {}",
                reported.len()
            );
            walked.extend(reported);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case}: paging ends rather than handing out tokens for ever, got {pages} \
                 pages"
            );
            token = answer.body["nextToken"].as_str().map(str::to_owned);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {case}: the pages concatenate to the whole set in the listing's order, got \
             {walked:?}"
        );
    }

    // A request that names no page size gets the documented default, which is larger
    // than this set: the whole set on one page.
    assert!(
        ATTACHMENT_PAGE >= whole.len(),
        "case paging: this set is smaller than the default page, got {}",
        whole.len()
    );
    let defaulted = list_attachments(&emulator, &id, json!({})).await;
    assert_eq!(
        page(&defaulted, "paging with no maxResults"),
        whole,
        "case paging with no maxResults: the default page reports the whole set, got {:?}",
        defaulted.body
    );

    // A token this operation never issued names no place in the set, so it is a
    // malformed request rather than an empty page. Each row is a token of a different
    // shape: one from another resource, one that was never minted, and an empty one
    // (which asks for the first page).
    for (token, expected_status) in [
        ("dsa-notatokenthisserviceissued", 400),
        ("arn:aws:es:us-east-1:123456789012:domain/paging-src0", 400),
        ("", 200),
    ] {
        let case = format!("paging from token {token:?}");
        let answer = list_attachments(&emulator, &id, json!({ "nextToken": token })).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case}: expected {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: a token this operation never issued is a malformed request, got {:?}",
                answer.body
            );
        }
    }

    // A negative page size is not a page size at all, so it is refused rather than
    // reinterpreted, while zero is the request that asks for no particular size.
    for max_results in [-1, -50] {
        let case = format!("paging with maxResults {max_results}");
        let answer = list_attachments(&emulator, &id, json!({ "maxResults": max_results })).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: a negative page size is a malformed request, got {:?}",
            answer.body
        );
    }
    let zero = list_attachments(&emulator, &id, json!({ "maxResults": 0 })).await;
    assert_eq!(
        page(&zero, "paging with maxResults 0"),
        whole,
        "case paging with maxResults 0: the default page reports the whole set, got {:?}",
        zero.body
    );
}

#[tokio::test]
async fn the_listing_reports_the_set_in_the_order_the_attaches_were_made() {
    let emulator = Emulator::new();
    let id = application(&emulator, "ordering-ui").await;
    // The data sources are attached in an order their ARNs do NOT sort in, so a listing
    // that reported them keyed by ARN would disagree with the order they were attached
    // in.
    let names = ["zz-order-src", "mm-order-src", "aa-order-src"];
    let mut arns = Vec::new();
    for name in names {
        arns.push(domain(&emulator, name).await);
    }
    tokio::time::sleep(SETTLE_WAIT).await;
    for arn in &arns {
        let answer = attach(&emulator, &id, json!({ "dataSourceArn": arn })).await;
        assert_eq!(
            answer.status, 200,
            "case ordering: expected 200 from AttachDataSource({arn:?}), got {} with {:?}",
            answer.status, answer.body
        );
    }

    let listed: Vec<String> = whole_set(&emulator, &id, "ordering")
        .await
        .into_iter()
        .map(|entry| entry.data_source_arn)
        .collect();
    assert_eq!(
        listed, arns,
        "case ordering: the listing reports the attachments in the order they were attached"
    );

    // Detaching one and attaching it again makes a NEW attachment, so it is reported
    // last — and the records around it keep their places.
    let detached = detach(&emulator, &id, &arns[0]).await;
    assert_eq!(
        detached.status, 200,
        "case ordering: expected 200 from DetachDataSource, got {} with {:?}",
        detached.status, detached.body
    );
    let reattached = attach(&emulator, &id, json!({ "dataSourceArn": arns[0] })).await;
    assert_eq!(
        reattached.status, 200,
        "case ordering: expected 200 from the second AttachDataSource, got {} with {:?}",
        reattached.status, reattached.body
    );
    let listed: Vec<String> = whole_set(&emulator, &id, "ordering after a reattach")
        .await
        .into_iter()
        .map(|entry| entry.data_source_arn)
        .collect();
    assert_eq!(
        listed,
        vec![arns[1].clone(), arns[2].clone(), arns[0].clone()],
        "case ordering after a reattach: the new attachment is the newest of the set"
    );
}

#[tokio::test]
async fn an_attachment_settles_on_the_clock_without_a_call_asking_for_it() {
    let emulator = Emulator::new();

    // Both the application and the data source are being provisioned, so neither is
    // active yet and the attachment cannot complete at once.
    let id = application(&emulator, "pending-ui").await;
    let provisioning = domain(&emulator, "pending-src").await;
    let answer = attach(&emulator, &id, json!({ "dataSourceArn": provisioning })).await;
    assert_eq!(
        answer.status, 200,
        "case an attach while both are provisioning: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let pending = reported(&answer.body, "an attach while both are provisioning");
    assert_eq!(
        pending.status, ATTACHMENT_STATUS_PENDING,
        "case an attach while both are provisioning: the attachment waits for them, got {:?}",
        answer.body
    );
    assert_eq!(
        whole_set(&emulator, &id, "the pending attachment").await,
        vec![pending.clone()],
        "case the pending attachment: the listing reports attachments in every state"
    );

    // Nothing asks for the move: the span the clock owed it passes, and the next read
    // reports the attachment it left.
    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = whole_set(&emulator, &id, "the settled attachment").await;
    assert_eq!(
        settled,
        vec![Reported {
            status: ATTACHMENT_STATUS_ATTACHED.to_string(),
            ..pending.clone()
        }],
        "case the settled attachment: the attachment the clock owed a move has landed"
    );
    let read = describe_attachment(&emulator, &id, &pending.data_source_arn).await;
    assert_eq!(
        reported(&read.body, "the settled attachment"),
        settled[0],
        "case the settled attachment: the read reports the status the listing does, got {:?}",
        read.body
    );

    // An attach made once both are active completes immediately instead.
    let active = domain(&emulator, "active-src").await;
    tokio::time::sleep(SETTLE_WAIT).await;
    let answer = attach(&emulator, &id, json!({ "dataSourceArn": active })).await;
    assert_eq!(
        reported(&answer.body, "an attach while both are active").status,
        ATTACHMENT_STATUS_ATTACHED,
        "case an attach while both are active: the attachment completes at once, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn an_attachment_whose_data_source_is_gone_fails_rather_than_completing() {
    let emulator = Emulator::new();
    let id = application(&emulator, "failing-ui").await;
    let arn = domain(&emulator, "failing-src").await;

    // The data source is still being provisioned, so the attachment is owed a move
    // rather than complete.
    let answer = attach(&emulator, &id, json!({ "dataSourceArn": arn })).await;
    let pending = reported(&answer.body, "an attach whose data source is deleted");
    assert_eq!(
        pending.status, ATTACHMENT_STATUS_PENDING,
        "case an attach whose data source is deleted: the attachment waits, got {:?}",
        answer.body
    );

    // The data source goes away before the attachment could complete, so it can never
    // complete: the non-retryable outcome.
    let deleted = delete_domain(&emulator, "failing-src").await;
    assert_eq!(
        deleted.status, 200,
        "case an attach whose data source is deleted: expected 200 from DeleteDomain, got {} \
         with {:?}",
        deleted.status, deleted.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    assert_eq!(
        whole_set(&emulator, &id, "an attach whose data source is deleted").await,
        vec![Reported {
            status: ATTACHMENT_STATUS_FAILED.to_string(),
            ..pending
        }],
        "case an attach whose data source is deleted: the attachment failed rather than landing"
    );
}

#[tokio::test]
async fn an_attach_of_a_data_source_this_account_does_not_hold_is_accepted_then_fails() {
    let emulator = Emulator::new();
    let id = application(&emulator, "absent-src-ui").await;
    let held = domain(&emulator, "absent-src-held").await;
    tokio::time::sleep(SETTLE_WAIT).await;

    // Every row has ARN syntax and satisfies `ARN`'s `@length(20..2048)`, and resolves
    // to no data source this account holds. They differ in WHY it resolves to nothing —
    // the resource name, the Region, the account, the service — and one sits at the
    // published upper length bound, so the acceptance is not a property of one value.
    let stem = "arn:aws:es:us-east-1:123456789012:domain/";
    let absent: Vec<(&str, String)> = vec![
        (
            "a resource name this account holds no domain under",
            format!("{stem}no-domain-of-this-name"),
        ),
        (
            "a domain of this name in another Region",
            "arn:aws:es:ap-southeast-2:123456789012:domain/absent-src-held".to_string(),
        ),
        (
            "a domain of this name in another account",
            "arn:aws:es:us-east-1:210987654321:domain/absent-src-held".to_string(),
        ),
        (
            "another service's resource",
            "arn:aws:s3:::a-bucket-that-is-no-domain".to_string(),
        ),
        (
            "the longest value the length constraint allows",
            format!("{stem}{}", "d".repeat(2048 - stem.len())),
        ),
    ];

    let mut recorded = Vec::new();
    for (case, data_source_arn) in &absent {
        let answer = attach(&emulator, &id, json!({ "dataSourceArn": data_source_arn })).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: an attach is asynchronous, so expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        let record = reported(&answer.body, case);
        assert_eq!(
            record.status, ATTACHMENT_STATUS_PENDING,
            "case {case}: expected {ATTACHMENT_STATUS_PENDING} while the data source is not \
             active, got {record:?}"
        );
        assert_eq!(
            &record.data_source_arn, data_source_arn,
            "case {case}: the attachment records the data source it was asked for, got {record:?}"
        );
        // The record is the store's, not the answer's: the single-attachment read
        // reports the same one straight away.
        let read = describe_attachment(&emulator, &id, data_source_arn).await;
        assert_eq!(
            reported(&read.body, case),
            record,
            "case {case}: the read reports the record the attach stored, got {:?}",
            read.body
        );
        recorded.push(record);
    }

    // An attach of a data source this account DOES hold, and which is active, completes
    // at once: what the rule turns on is the data source's state, not the call.
    let answer = attach(&emulator, &id, json!({ "dataSourceArn": held })).await;
    assert_eq!(
        reported(&answer.body, "a held, active data source").status,
        ATTACHMENT_STATUS_ATTACHED,
        "case a held, active data source: the attachment completes at once, got {:?}",
        answer.body
    );

    // Nothing asks for the move: the span the clock owed each attachment passes, and
    // every attachment whose data source this account does not hold has failed —
    // "attachment timed out or encountered a non-retryable error" — while the one that
    // resolved stays attached.
    tokio::time::sleep(SETTLE_WAIT).await;
    let settled = whole_set(&emulator, &id, "an attach that resolves to nothing").await;
    assert_eq!(
        settled.len(),
        absent.len() + 1,
        "case an attach that resolves to nothing: every accepted attach is recorded, got \
         {settled:?}"
    );
    for ((case, data_source_arn), record) in absent.iter().zip(&recorded) {
        let found = settled
            .iter()
            .find(|entry| &entry.data_source_arn == data_source_arn)
            .unwrap_or_else(|| {
                panic!("case {case}: expected the listing to report {record:?}, got {settled:?}")
            });
        assert_eq!(
            found,
            &Reported {
                status: ATTACHMENT_STATUS_FAILED.to_string(),
                ..record.clone()
            },
            "case {case}: expected the attachment to fail rather than land, got {found:?}"
        );
        let read = describe_attachment(&emulator, &id, data_source_arn).await;
        assert_eq!(
            &reported(&read.body, case),
            found,
            "case {case}: the read and the listing report one record, got {:?}",
            read.body
        );
    }
    let resolved = settled
        .iter()
        .find(|entry| entry.data_source_arn == held)
        .unwrap_or_else(|| {
            panic!("case a held, active data source: expected its attachment, got {settled:?}")
        });
    assert_eq!(
        resolved.status, ATTACHMENT_STATUS_ATTACHED,
        "case a held, active data source: expected {ATTACHMENT_STATUS_ATTACHED}, got {resolved:?}"
    );
}

#[tokio::test]
async fn a_detached_data_source_leaves_the_set() {
    let emulator = Emulator::new();
    let id = application(&emulator, "detach-ui").await;
    let first = domain(&emulator, "detach-src0").await;
    let second = domain(&emulator, "detach-src1").await;
    tokio::time::sleep(SETTLE_WAIT).await;
    for arn in [&first, &second] {
        let answer = attach(&emulator, &id, json!({ "dataSourceArn": arn })).await;
        assert_eq!(
            answer.status, 200,
            "case detach: expected 200 from AttachDataSource({arn:?}), got {} with {:?}",
            answer.status, answer.body
        );
    }

    let answer = detach(&emulator, &id, &first).await;
    assert_eq!(
        answer.status, 200,
        "case detach: expected 200 from DetachDataSource, got {} with {:?}",
        answer.status, answer.body
    );
    for (member, expected) in [("id", &id), ("dataSourceArn", &first)] {
        assert_eq!(
            answer.body[member],
            json!(expected),
            "case detach: the detach reports the {member} it was given, got {:?}",
            answer.body
        );
    }

    // The attachment record is deleted, so the set no longer holds it and the read of it
    // is the missing-resource refusal — while the application's OTHER attachment is
    // untouched.
    let remaining = whole_set(&emulator, &id, "detach").await;
    assert_eq!(
        remaining
            .iter()
            .map(|entry| entry.data_source_arn.clone())
            .collect::<Vec<String>>(),
        vec![second.clone()],
        "case detach: only the detached data source left the set, got {remaining:?}"
    );
    for (case, answer) in [
        (
            "a read of a detached data source",
            describe_attachment(&emulator, &id, &first).await,
        ),
        (
            "a second detach of one data source",
            detach(&emulator, &id, &first).await,
        ),
    ] {
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {case}: expected {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: a data source the application holds no record for is not found, got \
             {:?}",
            answer.body
        );
    }

    // A PENDING attachment is one still being made, so a detach of it is refused rather
    // than removing a record the clock is still owed a move on.
    let provisioning = domain(&emulator, "detach-pending-src").await;
    let answer = attach(&emulator, &id, json!({ "dataSourceArn": provisioning })).await;
    assert_eq!(
        reported(&answer.body, "a detach of a pending attachment").status,
        ATTACHMENT_STATUS_PENDING,
        "case a detach of a pending attachment: the attachment waits, got {:?}",
        answer.body
    );
    let answer = detach(&emulator, &id, &provisioning).await;
    assert_eq!(
        answer.status, 409,
        "case a detach of a pending attachment: expected 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ConflictException",
        "case a detach of a pending attachment: the attachment is in use, got {:?}",
        answer.body
    );

    // "The application must be in the ACTIVE state": one still being provisioned is
    // refused the detach.
    let provisioning_application = application(&emulator, "detach-creating-ui").await;
    let answer = detach(&emulator, &provisioning_application, &second).await;
    assert_eq!(
        answer.status, 409,
        "case a detach from a provisioning application: expected 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ConflictException",
        "case a detach from a provisioning application: the application is not active yet, got \
         {:?}",
        answer.body
    );
}

#[tokio::test]
async fn an_attach_is_idempotent_by_data_source_and_by_client_token() {
    let emulator = Emulator::new();
    let id = application(&emulator, "idempotent-ui").await;
    let arn = domain(&emulator, "idempotent-src").await;
    let other = domain(&emulator, "idempotent-src-other").await;
    tokio::time::sleep(SETTLE_WAIT).await;

    let first = attach(
        &emulator,
        &id,
        json!({ "dataSourceArn": arn, "clientToken": "idempotent-1" }),
    )
    .await;
    let minted = reported(&first.body, "the first attach");

    // "If the data source is already attached or pending, the operation returns the
    // existing attachment", and a retry "with the same client token and the same
    // parameters" is answered with the record the first call made: either way ONE
    // attachment, not one per call.
    for (case, body) in [
        ("the same data source again", json!({ "dataSourceArn": arn })),
        (
            "the same client token again",
            json!({ "dataSourceArn": arn, "clientToken": "idempotent-1" }),
        ),
    ] {
        let answer = attach(&emulator, &id, body.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 from AttachDataSource({body}), got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported(&answer.body, case),
            minted,
            "case {case}: the repeat is answered with the attachment the first call made, got \
             {:?}",
            answer.body
        );
    }

    // The same token for DIFFERENT parameters is not a retry of that request at all: it
    // is one token for two attaches, so it is the modelled conflict rather than the
    // first data source's attachment reported back for the second.
    let answer = attach(
        &emulator,
        &id,
        json!({ "dataSourceArn": other, "clientToken": "idempotent-1" }),
    )
    .await;
    assert_eq!(
        answer.status, 409,
        "case the same client token for another data source: expected 409, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ConflictException",
        "case the same client token for another data source: one token cannot identify two \
         attaches, got {:?}",
        answer.body
    );

    // ...and that refusal attached nothing: the application still holds the one
    // attachment its served attach made.
    let set = whole_set(&emulator, &id, "an idempotent attach").await;
    assert_eq!(
        set,
        vec![minted],
        "case an idempotent attach: the application holds one attachment, not one per call"
    );

    // The same data source under a FRESH token is still the one attachment it already
    // has, and another data source under a fresh token is an attachment of its own.
    for (case, data_source_arn, expected) in [
        ("the same data source under a fresh token", &arn, 1),
        ("another data source under a fresh token", &other, 2),
    ] {
        let answer = attach(
            &emulator,
            &id,
            json!({ "dataSourceArn": data_source_arn, "clientToken": format!("fresh-{case}") }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            whole_set(&emulator, &id, case).await.len(),
            expected,
            "case {case}: the application holds one attachment per attached data source"
        );
    }
}

#[tokio::test]
async fn an_attach_the_store_cannot_serve_is_refused() {
    let emulator = Emulator::new();
    let id = application(&emulator, "refusal-ui").await;
    let arn = domain(&emulator, "refusal-src").await;
    tokio::time::sleep(SETTLE_WAIT).await;

    // A value that is no ARN at all is the malformed input it is — the syntax of the
    // member is this call's to check, while resolving an ARN to a data source is the
    // attach's own asynchronous work (see
    // [`an_attach_of_a_data_source_this_account_does_not_hold_is_accepted_then_fails`]).
    // Each row is long enough to satisfy `ARN`'s own `@length(min: 20)`, so the refusal
    // comes from the rule under test rather than from the request boundary.
    for (case, data_source_arn, status, code) in [
        (
            "a value that is not an ARN at all",
            "not-an-arn-but-long-enough",
            400,
            "ValidationException",
        ),
        (
            "a value whose resource field is empty",
            "arn:aws:es:us-east-1:123456789012:",
            400,
            "ValidationException",
        ),
    ] {
        let answer = attach(&emulator, &id, json!({ "dataSourceArn": data_source_arn })).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected {status}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            code,
            "case {case}: expected {code}, got {:?}",
            answer.body
        );
    }

    // `workspaceId` and `workspaceConfiguration` are mutually exclusive, so a request
    // carrying both names two workspaces for one attachment.
    let answer = attach(
        &emulator,
        &id,
        json!({
            "dataSourceArn": arn,
            "workspaceId": "ws-0123456789",
            "workspaceConfiguration": { "name": "Observability", "workspaceType": "OBSERVABILITY" },
        }),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case an attach naming two workspaces: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case an attach naming two workspaces: the two members are mutually exclusive, got {:?}",
        answer.body
    );

    // Nothing was attached by any refused call.
    assert!(
        whole_set(&emulator, &id, "a refused attach").await.is_empty(),
        "case a refused attach: a refusal attaches nothing"
    );
}

#[tokio::test]
async fn every_attachment_operation_refuses_an_id_that_names_no_application() {
    let emulator = Emulator::new();
    let arn = domain(&emulator, "unknown-app-src").await;
    tokio::time::sleep(SETTLE_WAIT).await;

    // Each id satisfies `Id`'s own pattern `^[a-z0-9]{3,30}$` — including one at each
    // published length bound — so the refusal comes from the store rather than from the
    // request boundary.
    for id in ["abc", "noapplicationhere", "a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5"] {
        for (operation, answer) in [
            (
                "ListDataSourceAttachments",
                list_attachments(&emulator, id, json!({})).await,
            ),
            (
                "DescribeDataSourceAttachment",
                describe_attachment(&emulator, id, &arn).await,
            ),
            (
                "AttachDataSource",
                attach(&emulator, id, json!({ "dataSourceArn": arn })).await,
            ),
            ("DetachDataSource", detach(&emulator, id, &arn).await),
        ] {
            let case = format!("{operation} for the unknown application {id:?}");
            assert_eq!(
                answer.status, RECORDED_NOT_FOUND_STATUS,
                "case {case}: expected {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case}: an id naming no application is not found, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn the_attachment_set_of_one_application_is_bounded() {
    let emulator = Emulator::new();
    let id = application(&emulator, "bounded-ui").await;
    let mut arns = Vec::new();
    for index in 0..=ATTACHMENT_QUOTA {
        arns.push(domain(&emulator, &format!("bounded-src{index}")).await);
    }
    tokio::time::sleep(SETTLE_WAIT).await;

    // Every attach up to the bound is served...
    for arn in arns.iter().take(ATTACHMENT_QUOTA) {
        let answer = attach(&emulator, &id, json!({ "dataSourceArn": arn })).await;
        assert_eq!(
            answer.status, 200,
            "case an attach at the bound: expected 200 for {arn:?}, got {} with {:?}",
            answer.status, answer.body
        );
    }
    // ...and the one past it is refused rather than stored.
    let past = arns.last().expect("the table holds one data source past the bound");
    let answer = attach(&emulator, &id, json!({ "dataSourceArn": past })).await;
    assert_eq!(
        answer.status, 400,
        "case an attach past the bound: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case an attach past the bound: the set is bounded, got {:?}",
        answer.body
    );
    assert_eq!(
        whole_set(&emulator, &id, "an attach past the bound").await.len(),
        ATTACHMENT_QUOTA,
        "case an attach past the bound: the refused attach stored nothing"
    );
}
