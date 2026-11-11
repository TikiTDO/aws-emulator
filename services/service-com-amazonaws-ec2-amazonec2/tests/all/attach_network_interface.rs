// SPDX-License-Identifier: CC0-1.0

//! The ONE attachment record an `AttachNetworkInterface` writes, read back through every
//! reader that reports it — driven over the wire the service speaks (`ec2Query`, an
//! `application/x-www-form-urlencoded` POST answered with XML; README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! Two rules are under test.
//!
//! 1. AN ATTACH IS ONE WRITE EVERY READER REPORTS THE SAME WAY: "Attaches a network interface
//!    to an instance", so the attachment lands on the interface record and in the instance's
//!    index of the interfaces it holds — one record, so `DescribeNetworkInterfaces` and
//!    `DescribeInstances` cannot disagree about the attachment's id, its status, the device
//!    index and network card it sits on, or whether it goes with its instance. Everything the
//!    answer carries is read out of that record: the id is one the store minted (never a value
//!    from the request), the attachment is answered in the non-terminal `attaching` state with
//!    `attached` owed one [`ec2::state::TRANSITION_DELAY`] later, and the interface is in use
//!    from the call on. The rows span the device index, how many interfaces the instance
//!    already holds, and whether the request named the network card index or left it to its
//!    documented default.
//! 2. AN INTERFACE ONE INSTANCE HOLDS IS NOT GIVEN TO ANOTHER: an attach of an interface
//!    something already holds is refused `InvalidNetworkInterface.InUse` and writes nothing —
//!    neither instance's index moves — while an interface the clock has finished detaching
//!    holds nothing and so can be given to a different instance, at which point the index of
//!    the instance that held it no longer reports it and the new one does.

use std::time::Duration;

use ec2::state::TRANSITION_DELAY;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, error_code, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for: the emulator's own default Region, whose default
/// VPC and subnets a fresh account already holds.
const REGION: &str = "us-east-1";

/// The image every launch names. `RunInstances` only has to accept the id; which image an
/// instance ran from has no bearing on the interfaces it holds.
const AMI: &str = "ami-0123456789abcdef0";

/// The instance type every launch names. How many network interfaces an instance can hold is a
/// fact of its TYPE (`DescribeInstanceTypes`' `NetworkInfo$MaximumNetworkInterfaces`), so the
/// type is named rather than left to the launch default and every device index below stays
/// inside what it admits.
const INSTANCE_TYPE: &str = "m5.4xlarge";

/// The id prefix every network interface attachment is minted with.
const ATTACHMENT_ID_PREFIX: &str = "eni-attach";

/// The error code EC2 answers for an action asked of a network interface something still
/// holds. EC2's model declares no error shapes at all, so the code is matched as the wire
/// string the service answers with rather than through the service's own constant.
const INVALID_NETWORK_INTERFACE_IN_USE: &str = "InvalidNetworkInterface.InUse";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due, with a
/// margin so a slow machine fails nothing.
async fn wait_for_the_clock() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// One call that must succeed, as its body.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One `AttachNetworkInterface` call, with `extra` form fields beyond the three required
/// members.
async fn attach(
    router: &MultiServiceRouter,
    network_interface_id: &str,
    instance_id: &str,
    device_index: i32,
    extra: &[Param],
) -> (u16, String) {
    let mut params = vec![
        param("NetworkInterfaceId", network_interface_id),
        param("InstanceId", instance_id),
        param("DeviceIndex", &device_index.to_string()),
    ];
    params.extend_from_slice(extra);
    call(router, REGION, "AttachNetworkInterface", &params).await
}

/// The account's first default subnet, over the wire.
async fn default_subnet(router: &MultiServiceRouter) -> String {
    let body = ok(
        "the default network",
        router,
        "DescribeSubnets",
        &super::ec2_query::filters(&[("default-for-az", &["true"])]),
    )
    .await;
    element(&body, "subnetId").expect("a fresh account has a default subnet per zone")
}

/// One instance launched into `subnet_id`, as its id.
async fn launch(case: &str, router: &MultiServiceRouter, subnet_id: &str) -> String {
    let body = ok(
        case,
        router,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("InstanceType", INSTANCE_TYPE),
            param("MinCount", "1"),
            param("MaxCount", "1"),
            param("SubnetId", subnet_id),
        ],
    )
    .await;
    element(&body, "instanceId").expect("a launch reports the instance it minted")
}

/// One `CreateNetworkInterface` in `subnet_id`, as the interface id it minted.
async fn create_interface(case: &str, router: &MultiServiceRouter, subnet_id: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateNetworkInterface",
        &[param("SubnetId", subnet_id)],
    )
    .await;
    element(&body, "networkInterfaceId")
        .unwrap_or_else(|| panic!("case {case:?}: no interface in {body}"))
}

/// The `DescribeNetworkInterfaces` body for the one interface `network_interface_id` names.
async fn describe_interface(
    case: &str,
    router: &MultiServiceRouter,
    network_interface_id: &str,
) -> String {
    ok(
        case,
        router,
        "DescribeNetworkInterfaces",
        &indexed("NetworkInterfaceId", &[network_interface_id]),
    )
    .await
}

/// The `DescribeInstances` body for the one instance `instance_id` names.
async fn describe_instance(case: &str, router: &MultiServiceRouter, instance_id: &str) -> String {
    ok(
        case,
        router,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await
}

/// The members of one attachment that BOTH readers report, so the two reports can be compared
/// member for member rather than one reader being trusted about what the other says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Attachment {
    attachment_id: Option<String>,
    status: Option<String>,
    device_index: Option<String>,
    network_card_index: Option<String>,
    delete_on_termination: Option<String>,
}

/// The element of `body` that reports the interface `network_interface_id`: an entry of
/// `DescribeNetworkInterfaces`' set, or of the `networkInterfaceSet` an instance carries —
/// both are the element whose own `networkInterfaceId` child is that id.
fn interface_item<'a, 'i>(
    parsed: &'a roxmltree::Document<'i>,
    network_interface_id: &str,
) -> Option<roxmltree::Node<'a, 'i>> {
    parsed.descendants().find(|node| {
        node.is_element()
            && child_text(*node, "networkInterfaceId").as_deref() == Some(network_interface_id)
    })
}

/// The attachment members `body` reports for the interface `network_interface_id`, or `None`
/// when it reports that interface holding no attachment at all.
fn attachment(case: &str, body: &str, network_interface_id: &str) -> Option<Attachment> {
    let parsed = document(case, body);
    let item = interface_item(&parsed, network_interface_id).unwrap_or_else(|| {
        panic!("case {case:?}: no interface {network_interface_id} reported in {body}")
    });
    let attachment = child(item, "attachment")?;
    Some(Attachment {
        attachment_id: child_text(attachment, "attachmentId"),
        status: child_text(attachment, "status"),
        device_index: child_text(attachment, "deviceIndex"),
        network_card_index: child_text(attachment, "networkCardIndex"),
        delete_on_termination: child_text(attachment, "deleteOnTermination"),
    })
}

/// The text of the member `name` of the attachment `body` reports for the interface
/// `network_interface_id` — the members only the interface's own report carries (who holds the
/// attachment, and when it was made).
fn attachment_member(
    case: &str,
    body: &str,
    network_interface_id: &str,
    name: &str,
) -> Option<String> {
    let parsed = document(case, body);
    let item = interface_item(&parsed, network_interface_id)?;
    child_text(child(item, "attachment")?, name)
}

/// The INTERFACE's own status (`available` / `in-use`), read through the document so it is
/// never confused with its attachment's.
fn interface_status(case: &str, body: &str, network_interface_id: &str) -> Option<String> {
    let parsed = document(case, body);
    child_text(interface_item(&parsed, network_interface_id)?, "status")
}

/// The network interface ids an instance report holds, sorted: the entries of the
/// `networkInterfaceSet` the instance carries, so an id is read only where the INSTANCE says
/// it holds one.
///
/// Sorted but NOT deduplicated: an index entry written twice is exactly the defect the counting
/// assertion is for, so a repeated id has to reach the caller.
fn interfaces_the_instance_holds(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    let mut ids: Vec<String> = parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "networkInterfaceSet")
        .flat_map(|set| {
            set.children()
                .filter(roxmltree::Node::is_element)
                .filter_map(|item| child_text(item, "networkInterfaceId"))
                .collect::<Vec<String>>()
        })
        .collect();
    ids.sort();
    ids
}

/// One row of the attach table: what the request names beyond the interface the row creates,
/// and the network card index the attachment must then report.
///
/// A struct rather than a tuple so each column is named at the row that fills it.
struct Row {
    case: &'static str,
    device_index: i32,
    extra: Vec<Param>,
    /// The `networkCardIndex` both readers must report. "The default is network card index 0"
    /// (`AttachNetworkInterfaceRequest$NetworkCardIndex`), so a row that names none expects
    /// that default rather than an absent member.
    network_card_index: &'static str,
}

/// One attachment this suite made, as the row that asked for it and the ids the store
/// answered with — the columns the assertions after the clock has settled compare against.
struct Made {
    case: &'static str,
    device_index: i32,
    network_card_index: &'static str,
    network_interface_id: String,
    attachment_id: String,
}

/// Rule 1: the attach writes one attachment record, answers it out of the store, and both
/// readers report it identically — through the transient state the clock owes `attached`.
#[tokio::test]
async fn an_attach_writes_one_attachment_every_reader_reports_the_same_way() {
    let (router, _engine) = super::ec2_query::emulator();
    let subnet_id = default_subnet(&router).await;
    let instance_id = launch("the instance", &router, &subnet_id).await;

    // The rows differ in the device index the attachment sits at, in whether the request named
    // the network card index or left it to its documented default, and — because each row adds
    // an interface the instance keeps — in how many interfaces the instance already holds.
    let rows = vec![
        Row {
            case: "the first attach, naming only the three required members",
            device_index: 1,
            extra: Vec::new(),
            network_card_index: "0",
        },
        Row {
            case: "a second attach, naming the network card index explicitly",
            device_index: 2,
            extra: vec![param("NetworkCardIndex", "0")],
            network_card_index: "0",
        },
        Row {
            case: "a third attach, at a device index two above the last one taken",
            device_index: 4,
            extra: Vec::new(),
            network_card_index: "0",
        },
    ];

    let mut made: Vec<Made> = Vec::new();
    for Row {
        case,
        device_index,
        extra,
        network_card_index,
    } in rows
    {
        let network_interface_id = create_interface(case, &router, &subnet_id).await;
        let before = describe_interface(case, &router, &network_interface_id).await;
        assert_eq!(
            interface_status(case, &before, &network_interface_id).as_deref(),
            Some("available"),
            "case {case:?}: a created interface holds nothing, got {before}"
        );

        let (status, body) = attach(
            &router,
            &network_interface_id,
            &instance_id,
            device_index,
            &extra,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the attach, got {status} with body {body}"
        );
        let attachment_id = element(&body, "attachmentId")
            .unwrap_or_else(|| panic!("case {case:?}: the attach must answer an id: {body}"));
        assert!(
            is_long_id(ATTACHMENT_ID_PREFIX, &attachment_id),
            "case {case:?}: expected an attachment id the store minted, got {attachment_id:?}"
        );
        assert!(
            made.iter()
                .all(|earlier| earlier.attachment_id != attachment_id),
            "case {case:?}: every attachment gets an id of its own, got {attachment_id:?} again"
        );
        assert_eq!(
            element(&body, "networkCardIndex").as_deref(),
            Some(network_card_index),
            "case {case:?}: the attach reports the network card the attachment sits on, \
             got {body}"
        );

        // Straight after the call: the move is NOT instantaneous, so the caller still sees the
        // transient state, and the interface is in use from this call on.
        let described = describe_interface(case, &router, &network_interface_id).await;
        let reported = attachment(case, &described, &network_interface_id);
        assert_eq!(
            reported,
            Some(Attachment {
                attachment_id: Some(attachment_id.clone()),
                status: Some("attaching".to_string()),
                device_index: Some(device_index.to_string()),
                network_card_index: Some(network_card_index.to_string()),
                // "If you attach a network interface to an existing instance, the
                // DeleteOnTermination attribute is set to false by default" (the EC2 User
                // Guide's "Elastic network interfaces").
                delete_on_termination: Some("false".to_string()),
            }),
            "case {case:?}: expected the attachment the store holds, got {described}"
        );
        assert_eq!(
            interface_status(case, &described, &network_interface_id).as_deref(),
            Some("in-use"),
            "case {case:?}: an interface something holds is in use, got {described}"
        );
        assert_eq!(
            attachment_member(case, &described, &network_interface_id, "instanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: the attachment names the instance it was made for, got {described}"
        );
        assert_eq!(
            attachment_member(case, &described, &network_interface_id, "instanceOwnerId")
                .as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: the attachment names the account that owns the instance, \
             got {described}"
        );
        let attach_time = attachment_member(case, &described, &network_interface_id, "attachTime")
            .unwrap_or_else(|| panic!("case {case:?}: the attachment reports when it was made"));
        assert!(
            as_timestamp(&attach_time).is_some(),
            "case {case:?}: expected a timestamp for the attach time, got {attach_time:?}"
        );

        // The instance's own index of the interfaces it holds was written with the attachment,
        // so its report names the interface straight away. The two reports are compared member
        // for member once the clock has settled the attachment, below: inside the transient
        // window the two reads would be of two different instants, which says nothing about
        // whether they are one record.
        let instance_body = describe_instance(case, &router, &instance_id).await;
        assert!(
            interfaces_the_instance_holds(case, &instance_body).contains(&network_interface_id),
            "case {case:?}: the instance must report the interface it was given, \
             got {instance_body}"
        );

        made.push(Made {
            case,
            device_index,
            network_card_index,
            network_interface_id,
            attachment_id,
        });
    }

    wait_for_the_clock().await;

    for Made {
        case,
        device_index,
        network_card_index,
        network_interface_id,
        attachment_id,
    } in &made
    {
        let described = describe_interface(case, &router, network_interface_id).await;
        let settled = attachment(case, &described, network_interface_id);
        assert_eq!(
            settled,
            Some(Attachment {
                attachment_id: Some(attachment_id.clone()),
                status: Some("attached".to_string()),
                device_index: Some(device_index.to_string()),
                network_card_index: Some((*network_card_index).to_string()),
                delete_on_termination: Some("false".to_string()),
            }),
            "case {case:?}: one TRANSITION_DELAY after the attach the attachment must be \
             attached, and nothing else about it may change, got {described}"
        );
        let instance_body = describe_instance(case, &router, &instance_id).await;
        assert_eq!(
            attachment(case, &instance_body, network_interface_id),
            settled,
            "case {case:?}: the instance report and the interface report are one record, so \
             once the attachment has settled the two must agree member for member, \
             got {instance_body}"
        );
    }

    // Every interface the attaches made, beside the primary one the launch created, is held by
    // the one instance — one attach, one interface, no record written twice.
    let instance_body = describe_instance("the instance", &router, &instance_id).await;
    let held = interfaces_the_instance_holds("the instance", &instance_body);
    assert_eq!(
        held.len(),
        made.len() + 1,
        "the instance must hold its primary interface and each attached one exactly once, \
         got {held:?}"
    );
}

/// Rule 2: an interface one instance holds is not given to another, and one the clock has
/// finished detaching is.
#[tokio::test]
async fn an_interface_one_instance_holds_is_not_given_to_another() {
    let (router, _engine) = super::ec2_query::emulator();
    let subnet_id = default_subnet(&router).await;
    // Both instances in ONE subnet, so they share an Availability Zone and the only thing that
    // can refuse the second attach is the interface already being held.
    let first = launch("the instance that holds it first", &router, &subnet_id).await;
    let second = launch("the instance it moves to", &router, &subnet_id).await;
    let network_interface_id = create_interface("the interface", &router, &subnet_id).await;

    let (status, body) = attach(&router, &network_interface_id, &first, 1, &[]).await;
    assert_eq!(
        status, 200,
        "the first attach must succeed, got {status} with body {body}"
    );
    let attachment_id = element(&body, "attachmentId").expect("the attach answers an id");
    wait_for_the_clock().await;

    // The interface is held, so the second instance cannot be given it — at the device index
    // the first attach used or at any other.
    for device_index in [1, 3] {
        let case = format!("an attach to another instance at device index {device_index}");
        let (status, body) =
            attach(&router, &network_interface_id, &second, device_index, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_NETWORK_INTERFACE_IN_USE,
            "case {case:?}: expected {INVALID_NETWORK_INTERFACE_IN_USE} for an interface \
             something already holds, got {body}"
        );
    }

    // And the refusals wrote nothing: the first instance still holds it, the second does not.
    let described = describe_interface("the refusals", &router, &network_interface_id).await;
    assert_eq!(
        attachment_member("the refusals", &described, &network_interface_id, "instanceId")
            .as_deref(),
        Some(first.as_str()),
        "a refused attach must leave the attachment where it was, got {described}"
    );
    let second_body = describe_instance("the refusals", &router, &second).await;
    assert!(
        !interfaces_the_instance_holds("the refusals", &second_body)
            .contains(&network_interface_id),
        "a refused attach must not write the interface into the other instance's index, \
         got {second_body}"
    );

    // An interface the clock has finished detaching holds nothing, so the other instance can be
    // given it — and the index of the instance that held it no longer reports it.
    let (status, body) = call(
        &router,
        REGION,
        "DetachNetworkInterface",
        &[param("AttachmentId", &attachment_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the detach of what the attach made must succeed, got {status} with body {body}"
    );
    wait_for_the_clock().await;

    // The detach has settled, so the interface really holds nothing — read off the store
    // rather than inferred from the attach below being served.
    let detached = describe_interface("the detach", &router, &network_interface_id).await;
    assert_eq!(
        attachment("the detach", &detached, &network_interface_id),
        None,
        "one TRANSITION_DELAY after the detach the interface must hold no attachment, \
         got {detached}"
    );
    assert_eq!(
        interface_status("the detach", &detached, &network_interface_id).as_deref(),
        Some("available"),
        "an interface nothing holds is available again, got {detached}"
    );

    let (status, body) = attach(&router, &network_interface_id, &second, 1, &[]).await;
    assert_eq!(
        status, 200,
        "an interface that holds nothing can be given to another instance, got {status} with \
         body {body}"
    );
    let moved_attachment_id = element(&body, "attachmentId").expect("the attach answers an id");
    assert_ne!(
        moved_attachment_id, attachment_id,
        "a new attachment is a new record, so it gets an id of its own, got \
         {moved_attachment_id:?}"
    );

    let described = describe_interface("the move", &router, &network_interface_id).await;
    assert_eq!(
        attachment_member("the move", &described, &network_interface_id, "instanceId").as_deref(),
        Some(second.as_str()),
        "the attachment must now name the instance that was given the interface, got {described}"
    );
    let first_body = describe_instance("the move", &router, &first).await;
    assert!(
        !interfaces_the_instance_holds("the move", &first_body).contains(&network_interface_id),
        "the instance that held the interface must no longer report it, got {first_body}"
    );
    let second_body = describe_instance("the move", &router, &second).await;
    assert!(
        interfaces_the_instance_holds("the move", &second_body).contains(&network_interface_id),
        "the instance that was given the interface must report it, got {second_body}"
    );
}
