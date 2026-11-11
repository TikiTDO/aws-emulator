// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RegisterImage`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rule under test is that A REGISTER MINTS ONE AMI OF THE CALLING ACCOUNT FROM THE
//! REQUEST'S OWN DESCRIPTION AND PERSISTS IT: the response carries the new `ImageId`, and a
//! `DescribeImages` of that id reports exactly what was registered — the name, description,
//! architecture, virtualization, root device, boot and metadata support, kernel and RAM disk,
//! tags, and the block device mappings with the CALLER's snapshots in them — with the
//! documented defaults for the members the request leaves out, and the service's own facts
//! (the id, the owner, the location, the creation date) derived rather than taken. A request
//! whose description this account cannot back is refused and stores nothing.
//!
//! Three suites:
//!
//! 1. the registration table — rows differing along what the rule generalises over (which
//!    members the request sets, the architecture, the virtualization type, how many devices
//!    the AMI maps, whether a mapping names its volume size or takes the snapshot's, which
//!    device is the root, and whether the AMI is Amazon EBS-backed or instance store-backed),
//!    each read back through `DescribeImages`;
//! 2. the refusal table — every way a register names something this account cannot back, with
//!    the code and the message EC2 answers, and the account's images unchanged afterwards;
//! 3. the state-is-real suite — the snapshot a registered AMI restores its root device from
//!    cannot be deleted while that AMI is registered, and the UEFI variable store a register
//!    supplied is the one `DescribeImageAttribute` reports.

use ec2::state::{
    DEFAULT_VOLUME_TYPE, DRY_RUN_OPERATION, IMAGE_DEREGISTRATION_PROTECTION_DISABLED,
    IMAGE_HYPERVISOR_XEN, IMAGE_ROOT_DEVICE_TYPE_EBS, IMAGE_ROOT_DEVICE_TYPE_INSTANCE_STORE,
    IMAGE_SRIOV_NET_SUPPORT_SIMPLE, IMAGE_STATE_AVAILABLE, IMAGE_TYPE_MACHINE,
    IMAGE_VIRTUALIZATION_TYPE_HVM, IMDS_SUPPORT_V2, INACCESSIBLE_STORAGE_LOCATION,
    INSTANCE_STORE_ROOT_DEVICE_NAME, INVALID_AMI_NAME_DUPLICATE, INVALID_AMI_NAME_MALFORMED,
    INVALID_SNAPSHOT_ID_MALFORMED, INVALID_SNAPSHOT_IN_USE, LINUX_ROOT_DEVICE_NAME,
    REGISTERED_IMAGE_DEFAULT_ARCHITECTURE, REGISTERED_IMAGE_DEFAULT_VIRTUALIZATION_TYPE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message};
use super::volume_lifecycle::{REGION, p, zone_name};
use super::wire::{child, child_text, document, set_items};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The HTTP status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The HTTP status `DryRunOperation` carries: the request was refused because a precondition
/// the CALLER set holds, not because anything in it was wrong.
const DRY_RUN_STATUS: u16 = 412;

/// The code `RegisterImage`'s block-device-mapping validator answers for a well-formed
/// snapshot id no snapshot of the account has — the id-parser family's not-found code, which
/// is the one this operation's own recorded refusal carries (it is a different code from the
/// `InvalidSnapshot.NotFound` the describes and the delete of a snapshot answer, so the wire
/// string is pinned here rather than read from the crate).
const INVALID_SNAPSHOT_ID_NOT_FOUND: &str = "InvalidSnapshotID.NotFound";

/// The code EC2 answers for a block device mapping it cannot use (EC2 API "Error codes",
/// `InvalidBlockDeviceMapping`).
const INVALID_BLOCK_DEVICE_MAPPING: &str = "InvalidBlockDeviceMapping";

/// EC2's code for a request the caller may not make.
const AUTH_FAILURE: &str = "AuthFailure";

/// EC2's code for a value outside what a parameter accepts.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's code for members that cannot hold together.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// One `action` call signed for the Region this suite works in.
async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Mint one snapshot of `size` GiB of this account, through the operations that create one,
/// and answer its id — the snapshot a register's block device mapping restores from.
async fn snapshot_of(router: &MultiServiceRouter, case: &str, size: i32) -> String {
    let (status, body) = op(
        router,
        "CreateVolume",
        &[p("AvailabilityZone", zone_name(0)), p("Size", size.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVolume, got {status} with body {body}"
    );
    let volume_id = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: no volumeId in {body}"));
    let (status, body) = op(router, "CreateSnapshot", &[p("VolumeId", &volume_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateSnapshot, got {status} with body {body}"
    );
    element(&body, "snapshotId")
        .unwrap_or_else(|| panic!("case {case:?}: no snapshotId in {body}"))
}

/// The form fields of one block device mapping at `index` (counting from 1).
fn mapping(index: usize, device_name: &str, members: &[(&str, String)]) -> Params {
    let mut fields = vec![p(
        &format!("BlockDeviceMapping.{index}.DeviceName"),
        device_name,
    )];
    for (member, value) in members {
        fields.push(p(
            &format!("BlockDeviceMapping.{index}.{member}"),
            value.clone(),
        ));
    }
    fields
}

/// `BlockDeviceMapping.N.Ebs.SnapshotId` as [`mapping`] takes it.
fn ebs_snapshot(snapshot_id: &str) -> (&'static str, String) {
    ("Ebs.SnapshotId", snapshot_id.to_string())
}

/// One `RegisterImage` call carrying `params`.
async fn register(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    op(router, "RegisterImage", params).await
}

/// The id a successful register answered with, asserting it succeeded and that the id is one
/// this service minted.
async fn registered(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = register(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from RegisterImage, got {status} with body {body}"
    );
    let image_id = element(&body, "imageId")
        .unwrap_or_else(|| panic!("case {case:?}: no imageId in {body}"));
    assert!(
        super::wire::is_long_id("ami", &image_id),
        "case {case:?}: expected an AMI id this service minted, got {image_id:?}"
    );
    image_id
}

/// What `DescribeImages` reports for `image_id`: the (member, text) pairs of the one image it
/// answers with, the sets left out — a member is read off the stored image, so this is what
/// the register persisted.
async fn described(router: &MultiServiceRouter, case: &str, image_id: &str) -> Vec<(String, String)> {
    let body = describe_body(router, case, image_id).await;
    let parsed = document(case, &body);
    let item = image_item(case, &parsed, &body);
    item.children()
        .filter(|node| node.is_element())
        .map(|node| {
            (
                node.tag_name().name().to_string(),
                node.text().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `DescribeImages` body for the one AMI `image_id`, asserting the read succeeded.
async fn describe_body(router: &MultiServiceRouter, case: &str, image_id: &str) -> String {
    let (status, body) = op(router, "DescribeImages", &[p("ImageId.1", image_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImages of {image_id}, got {status} with \
         body {body}"
    );
    body
}

/// The one `<item>` of a single-image `DescribeImages` body.
fn image_item<'a, 'i>(
    case: &str,
    parsed: &'a roxmltree::Document<'i>,
    body: &str,
) -> roxmltree::Node<'a, 'i> {
    parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "imageId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no image (no <imageId>) in {body}"))
}

/// The block device mappings `DescribeImages` reports for `image_id`, each as its device name
/// and the (member, text) pairs of its Amazon EBS half.
async fn described_mappings(
    router: &MultiServiceRouter,
    case: &str,
    image_id: &str,
) -> Vec<(String, Vec<(String, String)>)> {
    let body = describe_body(router, case, image_id).await;
    let parsed = document(case, &body);
    let item = image_item(case, &parsed, &body);
    set_items(item, "blockDeviceMapping")
        .into_iter()
        .map(|mapping| {
            let device_name = child_text(mapping, "deviceName").unwrap_or_default();
            let ebs = child(mapping, "ebs")
                .map(|ebs| {
                    ebs.children()
                        .filter(|node| node.is_element())
                        .map(|node| {
                            (
                                node.tag_name().name().to_string(),
                                node.text().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            (device_name, ebs)
        })
        .collect()
}

/// Every image id the account's own `DescribeImages` reports for the images it OWNS — the set
/// a register adds exactly one id to and a refused register leaves alone.
async fn own_image_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = op(router, "DescribeImages", &[p("Owner.1", "self")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImages, got {status} with body {body}"
    );
    elements(&body, "imageId")
}

/// Assert that `image_id` reports `value` for `member`, naming the case.
fn reports(case: &str, reported: &[(String, String)], member: &str, value: &str) {
    let found = reported
        .iter()
        .find(|(name, _)| name == member)
        .map(|(_, text)| text.clone());
    assert_eq!(
        found.as_deref(),
        Some(value),
        "case {case:?}: expected <{member}> {value:?}, got {found:?} in {reported:?}"
    );
}

/// Assert that the image reports no `member` at all — the answer for a facility nothing
/// enabled, which is a different answer from reporting it as off.
fn reports_nothing(case: &str, reported: &[(String, String)], member: &str) {
    let found = reported.iter().find(|(name, _)| name == member);
    assert!(
        found.is_none(),
        "case {case:?}: expected no <{member}> at all, got {found:?}"
    );
}

/// One row of the registration table.
struct Registration {
    /// What makes this row a different input.
    case: &'static str,
    /// The parameters of the register under test, beside its `Name`.
    params: Params,
    /// The members `DescribeImages` must report for the AMI, as (member, text).
    reported: Vec<(&'static str, String)>,
    /// The members it must report NOTHING for.
    absent: Vec<&'static str>,
    /// The mappings it must report: each device name with the (member, text) pairs of its
    /// Amazon EBS half.
    mappings: Vec<(String, Vec<(&'static str, String)>)>,
}

/// RULE: a register mints the AMI the request describes and `DescribeImages` reports it back.
///
/// The rows differ along which members the request sets, the architecture, the virtualization
/// type, how many devices the AMI maps, whether a mapping names its own volume size or takes
/// the snapshot's, which device is the root, and whether the AMI is Amazon EBS-backed or
/// instance store-backed — so no one of them is the rule.
#[tokio::test]
async fn a_register_mints_the_ami_the_request_describes() {
    let (router, _engine) = super::ec2_query::emulator();
    let account = engine::emulator_account_id();

    // Two snapshots of this account, of different sizes, so a mapping that takes the
    // snapshot's size and one that names a larger size of its own are different rows.
    let root_snapshot = snapshot_of(&router, "the table's root snapshot", 8).await;
    let data_snapshot = snapshot_of(&router, "the table's data snapshot", 20).await;

    let rows: Vec<Registration> = vec![
        Registration {
            case: "an Amazon EBS-backed AMI from a root snapshot, with every documented default",
            params: mapping(1, LINUX_ROOT_DEVICE_NAME, &[ebs_snapshot(&root_snapshot)])
                .into_iter()
                .chain([p("RootDeviceName", LINUX_ROOT_DEVICE_NAME)])
                .collect(),
            reported: vec![
                ("imageState", IMAGE_STATE_AVAILABLE.to_string()),
                ("imageType", IMAGE_TYPE_MACHINE.to_string()),
                ("imageOwnerId", account.to_string()),
                ("isPublic", "false".to_string()),
                ("hypervisor", IMAGE_HYPERVISOR_XEN.to_string()),
                ("rootDeviceType", IMAGE_ROOT_DEVICE_TYPE_EBS.to_string()),
                ("rootDeviceName", LINUX_ROOT_DEVICE_NAME.to_string()),
                // "Default: For Amazon EBS-backed AMIs, i386" and "Default: paravirtual".
                (
                    "architecture",
                    REGISTERED_IMAGE_DEFAULT_ARCHITECTURE.to_string(),
                ),
                (
                    "virtualizationType",
                    REGISTERED_IMAGE_DEFAULT_VIRTUALIZATION_TYPE.to_string(),
                ),
                (
                    "deregistrationProtection",
                    IMAGE_DEREGISTRATION_PROTECTION_DISABLED.to_string(),
                ),
            ],
            // Nothing enabled enhanced networking, NitroTPM, a boot mode, IMDSv2, a kernel
            // or a RAM disk for this AMI, and no instance was created from one.
            absent: vec![
                "enaSupport",
                "sriovNetSupport",
                "tpmSupport",
                "bootMode",
                "imdsSupport",
                "kernelId",
                "ramdiskId",
                "instanceId",
            ],
            mappings: vec![(
                LINUX_ROOT_DEVICE_NAME.to_string(),
                vec![
                    ("snapshotId", root_snapshot.clone()),
                    // "If you specify a snapshot, the default is the snapshot size."
                    ("volumeSize", "8".to_string()),
                    ("deleteOnTermination", "true".to_string()),
                    ("volumeType", DEFAULT_VOLUME_TYPE.to_string()),
                ],
            )],
        },
        Registration {
            case: "an HVM AMI that sets every member a register can set, over two devices",
            params: mapping(1, "/dev/sda1", &[ebs_snapshot(&root_snapshot)])
                .into_iter()
                .chain(mapping(
                    2,
                    "/dev/sdb",
                    &[
                        ebs_snapshot(&data_snapshot),
                        // "You can specify a volume size that is equal to or larger than
                        // the snapshot size."
                        ("Ebs.VolumeSize", "40".to_string()),
                        ("Ebs.VolumeType", "gp3".to_string()),
                        ("Ebs.DeleteOnTermination", "false".to_string()),
                    ],
                ))
                .chain([
                    p("RootDeviceName", "/dev/sda1"),
                    p("Architecture", "arm64"),
                    p("VirtualizationType", IMAGE_VIRTUALIZATION_TYPE_HVM),
                    p("Description", "A golden image of the fleet"),
                    p("BootMode", "uefi"),
                    p("ImdsSupport", IMDS_SUPPORT_V2),
                    p("TpmSupport", "v2.0"),
                    p("SriovNetSupport", IMAGE_SRIOV_NET_SUPPORT_SIMPLE),
                    p("EnaSupport", "true"),
                    p("KernelId", "aki-0a1b2c3d4e5f60001"),
                    p("RamdiskId", "ari-0a1b2c3d4e5f60002"),
                    p("TagSpecification.1.ResourceType", "image"),
                    p("TagSpecification.1.Tag.1.Key", "role"),
                    p("TagSpecification.1.Tag.1.Value", "golden"),
                ])
                .collect(),
            reported: vec![
                ("architecture", "arm64".to_string()),
                (
                    "virtualizationType",
                    IMAGE_VIRTUALIZATION_TYPE_HVM.to_string(),
                ),
                ("description", "A golden image of the fleet".to_string()),
                ("bootMode", "uefi".to_string()),
                ("imdsSupport", IMDS_SUPPORT_V2.to_string()),
                ("tpmSupport", "v2.0".to_string()),
                (
                    "sriovNetSupport",
                    IMAGE_SRIOV_NET_SUPPORT_SIMPLE.to_string(),
                ),
                ("enaSupport", "true".to_string()),
                ("kernelId", "aki-0a1b2c3d4e5f60001".to_string()),
                ("ramdiskId", "ari-0a1b2c3d4e5f60002".to_string()),
                ("rootDeviceName", "/dev/sda1".to_string()),
                ("rootDeviceType", IMAGE_ROOT_DEVICE_TYPE_EBS.to_string()),
                ("imageState", IMAGE_STATE_AVAILABLE.to_string()),
            ],
            absent: vec!["instanceId"],
            mappings: vec![
                (
                    "/dev/sda1".to_string(),
                    vec![
                        ("snapshotId", root_snapshot.clone()),
                        ("volumeSize", "8".to_string()),
                        ("deleteOnTermination", "true".to_string()),
                    ],
                ),
                (
                    "/dev/sdb".to_string(),
                    vec![
                        ("snapshotId", data_snapshot.clone()),
                        ("volumeSize", "40".to_string()),
                        ("volumeType", "gp3".to_string()),
                        ("deleteOnTermination", "false".to_string()),
                    ],
                ),
            ],
        },
        Registration {
            case: "a second AMI of the SAME snapshot, under a name of its own",
            params: mapping(1, "/dev/xvdh", &[ebs_snapshot(&root_snapshot)])
                .into_iter()
                .chain([p("RootDeviceName", "/dev/xvdh")])
                .collect(),
            reported: vec![
                ("rootDeviceName", "/dev/xvdh".to_string()),
                ("rootDeviceType", IMAGE_ROOT_DEVICE_TYPE_EBS.to_string()),
            ],
            absent: vec!["enaSupport"],
            mappings: vec![(
                "/dev/xvdh".to_string(),
                vec![
                    ("snapshotId", root_snapshot.clone()),
                    ("volumeSize", "8".to_string()),
                ],
            )],
        },
        Registration {
            case: "an instance store-backed AMI, whose root device no mapping restores",
            params: vec![p("RootDeviceName", INSTANCE_STORE_ROOT_DEVICE_NAME)],
            reported: vec![
                (
                    "rootDeviceType",
                    IMAGE_ROOT_DEVICE_TYPE_INSTANCE_STORE.to_string(),
                ),
                (
                    "rootDeviceName",
                    INSTANCE_STORE_ROOT_DEVICE_NAME.to_string(),
                ),
                ("imageState", IMAGE_STATE_AVAILABLE.to_string()),
            ],
            absent: vec!["enaSupport", "sriovNetSupport"],
            mappings: Vec::new(),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        // A name of this row's own: "the name must be unique for AMIs in the Region for this
        // account", which the refusal table below pins.
        let name = format!("register-image-row-{index}");
        let before = own_image_ids(&router, case).await;
        let mut params = vec![p("Name", &name)];
        params.extend(row.params.iter().cloned());
        let image_id = registered(&router, case, &params).await;

        let reported = described(&router, case, &image_id).await;
        reports(case, &reported, "imageId", &image_id);
        reports(case, &reported, "name", &name);
        // "The location of the AMI": `<owner>/<name>`, derived by the service rather than
        // taken from the request.
        reports(
            case,
            &reported,
            "imageLocation",
            &format!("{account}/{name}"),
        );
        for (member, value) in &row.reported {
            reports(case, &reported, member, value);
        }
        for member in &row.absent {
            reports_nothing(case, &reported, member);
        }
        // "The date and time the image was created": an instant the service wrote, not a
        // value the request could set.
        let creation_date = reported
            .iter()
            .find(|(member, _)| member == "creationDate")
            .map(|(_, text)| text.clone())
            .unwrap_or_default();
        assert!(
            super::ec2_query::as_timestamp(&creation_date).is_some(),
            "case {case:?}: expected a creationDate that reads as an instant, got \
             {creation_date:?}"
        );

        let described = described_mappings(&router, case, &image_id).await;
        assert_eq!(
            described.len(),
            row.mappings.len(),
            "case {case:?}: expected {} block device mapping(s), got {described:?}",
            row.mappings.len()
        );
        for (expected, (device_name, ebs)) in row.mappings.iter().zip(described.iter()) {
            assert_eq!(
                device_name, &expected.0,
                "case {case:?}: expected the mapping at device {:?}, got {device_name:?}",
                expected.0
            );
            for (member, value) in &expected.1 {
                reports(case, ebs, member, value);
            }
        }

        // The register added exactly its own AMI to the images this account owns.
        let after = own_image_ids(&router, case).await;
        let added: Vec<&String> = after.iter().filter(|id| !before.contains(id)).collect();
        assert_eq!(
            added,
            vec![&image_id],
            "case {case:?}: expected the register to add exactly {image_id}, got {added:?}"
        );
    }
}

/// One row of the refusal table: `(case, the parameters beside Name, the status, the code,
/// the message)`.
type RefusalRow = (&'static str, Params, u16, &'static str, String);

/// RULE: a register whose description this account cannot back is refused with EC2's own code
/// and message, and nothing is stored.
///
/// The rows differ along WHICH part of the description cannot be backed — the name, the
/// snapshot a mapping names (and whether that value is a snapshot id at all), the size of a
/// volume against its snapshot, the encryption state a mapping may not set, a device with no
/// name, the billing codes the account may not claim, the two free-string members' values,
/// the form of the AMI itself, and the caller's own `DryRun` precondition.
#[tokio::test]
async fn a_register_refuses_a_description_this_account_cannot_back() {
    let (router, _engine) = super::ec2_query::emulator();

    let snapshot = snapshot_of(&router, "the refusal table's snapshot", 8).await;
    // One AMI of the account, so the duplicate-name row has a name to clash with.
    let taken_name = "register-image-taken-name";
    let existing = registered(
        &router,
        "the AMI whose name is taken",
        &mapping(1, LINUX_ROOT_DEVICE_NAME, &[ebs_snapshot(&snapshot)])
            .into_iter()
            .chain([
                p("Name", taken_name),
                p("RootDeviceName", LINUX_ROOT_DEVICE_NAME),
            ])
            .collect::<Params>(),
    )
    .await;

    // A root mapping that resolves, for the rows whose fault is elsewhere.
    let good_mapping = || mapping(1, LINUX_ROOT_DEVICE_NAME, &[ebs_snapshot(&snapshot)]);
    let root_device = || p("RootDeviceName", LINUX_ROOT_DEVICE_NAME);
    // A snapshot id EC2 could have issued that no snapshot of this account has: the long id
    // form, and the all-zero placeholder a client reaches for.
    let absent_long = "snap-0a1b2c3d4e5f60001";
    let absent_null = "snap-00000000000000000";
    let absent_short = "snap-12345678";

    let rows: Vec<RefusalRow> = vec![
        (
            "a name outside the characters an AMI name may hold",
            good_mapping()
                .into_iter()
                .chain([root_device(), p("Name", "register-image*star")])
                .collect(),
            REFUSAL_STATUS,
            INVALID_AMI_NAME_MALFORMED,
            "AMI names must be between 3 and 128 characters long, and may contain letters, \
             digits, '(', ')', '.', '-', '/' and '_'"
                .to_string(),
        ),
        (
            "a name another AMI of this account already carries",
            good_mapping()
                .into_iter()
                .chain([root_device(), p("Name", taken_name)])
                .collect(),
            REFUSAL_STATUS,
            INVALID_AMI_NAME_DUPLICATE,
            format!("AMI name {taken_name} is already in use by AMI {existing}"),
        ),
        (
            "a long-form snapshot id no snapshot of this account has",
            mapping(1, LINUX_ROOT_DEVICE_NAME, &[ebs_snapshot(absent_long)])
                .into_iter()
                .chain([root_device(), p("Name", "register-image-absent-long")])
                .collect(),
            REFUSAL_STATUS,
            INVALID_SNAPSHOT_ID_NOT_FOUND,
            format!("The snapshot ID '{absent_long}' does not exist"),
        ),
        (
            "the all-zero snapshot id, which the mint can never have issued",
            mapping(1, LINUX_ROOT_DEVICE_NAME, &[ebs_snapshot(absent_null)])
                .into_iter()
                .chain([root_device(), p("Name", "register-image-absent-null")])
                .collect(),
            REFUSAL_STATUS,
            INVALID_SNAPSHOT_ID_NOT_FOUND,
            format!("The snapshot ID '{absent_null}' does not exist"),
        ),
        (
            "a short-form snapshot id no snapshot of this account has",
            mapping(1, LINUX_ROOT_DEVICE_NAME, &[ebs_snapshot(absent_short)])
                .into_iter()
                .chain([root_device(), p("Name", "register-image-absent-short")])
                .collect(),
            REFUSAL_STATUS,
            INVALID_SNAPSHOT_ID_NOT_FOUND,
            format!("The snapshot ID '{absent_short}' does not exist"),
        ),
        (
            "a value that is no snapshot id at all",
            mapping(1, LINUX_ROOT_DEVICE_NAME, &[ebs_snapshot("snap-not-a-snapshot")])
                .into_iter()
                .chain([root_device(), p("Name", "register-image-malformed-snap")])
                .collect(),
            REFUSAL_STATUS,
            INVALID_SNAPSHOT_ID_MALFORMED,
            "Value (snap-not-a-snapshot) for snapshot ID is invalid.".to_string(),
        ),
        (
            "a volume smaller than the snapshot it restores",
            mapping(
                1,
                LINUX_ROOT_DEVICE_NAME,
                &[ebs_snapshot(&snapshot), ("Ebs.VolumeSize", "2".to_string())],
            )
            .into_iter()
            .chain([root_device(), p("Name", "register-image-undersized")])
            .collect(),
            REFUSAL_STATUS,
            INVALID_BLOCK_DEVICE_MAPPING,
            format!("Volume of size 2GB is smaller than snapshot '{snapshot}', expect size >= 8GB"),
        ),
        (
            "a device with neither a snapshot nor a volume size",
            mapping(1, LINUX_ROOT_DEVICE_NAME, &[("Ebs.VolumeType", "gp3".to_string())])
                .into_iter()
                .chain([root_device(), p("Name", "register-image-no-source")])
                .collect(),
            REFUSAL_STATUS,
            INVALID_BLOCK_DEVICE_MAPPING,
            format!(
                "The blockDeviceMapping for device {LINUX_ROOT_DEVICE_NAME} must specify either \
                 a snapshot ID or a volume size"
            ),
        ),
        (
            "a mapping that names no device",
            vec![
                p("BlockDeviceMapping.1.Ebs.VolumeSize", "8"),
                root_device(),
                p("Name", "register-image-no-device"),
            ],
            REFUSAL_STATUS,
            INVALID_BLOCK_DEVICE_MAPPING,
            "Each blockDeviceMapping must name a device".to_string(),
        ),
        (
            "the encryption state of a volume restored from a snapshot",
            mapping(
                1,
                LINUX_ROOT_DEVICE_NAME,
                &[ebs_snapshot(&snapshot), ("Ebs.Encrypted", "true".to_string())],
            )
            .into_iter()
            .chain([root_device(), p("Name", "register-image-encrypted")])
            .collect(),
            REFUSAL_STATUS,
            INVALID_PARAMETER_VALUE,
            format!(
                "Value (true) for parameter encrypted is invalid. You can't set the encryption \
                 state of the volume of device {LINUX_ROOT_DEVICE_NAME} using the block device \
                 mapping"
            ),
        ),
        (
            "billing product codes this account is not authorized to specify",
            good_mapping()
                .into_iter()
                .chain([
                    root_device(),
                    p("Name", "register-image-billing"),
                    p("BillingProduct.1", "bp-0123456789abcdef0"),
                ])
                .collect(),
            REFUSAL_STATUS,
            AUTH_FAILURE,
            "Your account is not authorized to specify billing product codes".to_string(),
        ),
        (
            "a virtualization type that is neither of the two",
            good_mapping()
                .into_iter()
                .chain([
                    root_device(),
                    p("Name", "register-image-virtualization"),
                    p("VirtualizationType", "nitro"),
                ])
                .collect(),
            REFUSAL_STATUS,
            INVALID_PARAMETER_VALUE,
            "Value (nitro) for parameter virtualizationType is invalid. Valid values: hvm | \
             paravirtual"
                .to_string(),
        ),
        (
            "an enhanced-networking flag that is not the one value it takes",
            good_mapping()
                .into_iter()
                .chain([
                    root_device(),
                    p("Name", "register-image-sriov"),
                    p("SriovNetSupport", "complex"),
                ])
                .collect(),
            REFUSAL_STATUS,
            INVALID_PARAMETER_VALUE,
            format!(
                "Value (complex) for parameter sriovNetSupport is invalid. Valid value: \
                 {IMAGE_SRIOV_NET_SUPPORT_SIMPLE}"
            ),
        ),
        (
            "tags for a resource type this register does not create",
            good_mapping()
                .into_iter()
                .chain([
                    root_device(),
                    p("Name", "register-image-tag-type"),
                    p("TagSpecification.1.ResourceType", "snapshot"),
                    p("TagSpecification.1.Tag.1.Key", "role"),
                    p("TagSpecification.1.Tag.1.Value", "golden"),
                ])
                .collect(),
            REFUSAL_STATUS,
            INVALID_PARAMETER_VALUE,
            "'snapshot' is not a valid resource type for tagging on RegisterImage".to_string(),
        ),
        (
            "neither the manifest of an instance store-backed AMI nor a root device",
            good_mapping()
                .into_iter()
                .chain([p("Name", "register-image-no-form")])
                .collect(),
            REFUSAL_STATUS,
            INVALID_PARAMETER_COMBINATION,
            "Either imageLocation or rootDeviceName must be specified".to_string(),
        ),
        (
            "the Amazon S3 manifest of an instance store-backed AMI, which nothing here holds",
            vec![
                p("Name", "register-image-manifest"),
                p("ImageLocation", "amzn-image-bucket/probe/image.manifest.xml"),
            ],
            REFUSAL_STATUS,
            INACCESSIBLE_STORAGE_LOCATION,
            "Error accessing resource in S3. Check permissions, naming, and ensure the bucket \
             is within the same region as the API endpoint. Bucket: amzn-image-bucket, Key: \
             probe/image.manifest.xml."
                .to_string(),
        ),
        (
            "a register the caller only asked whether it may make",
            good_mapping()
                .into_iter()
                .chain([
                    root_device(),
                    p("Name", "register-image-dry-run"),
                    p("DryRun", "true"),
                ])
                .collect(),
            DRY_RUN_STATUS,
            DRY_RUN_OPERATION,
            "Request would have succeeded, but DryRun flag is set.".to_string(),
        ),
    ];

    for (case, params, status, code, message) in rows {
        let before = own_image_ids(&router, case).await;
        let (answered, body) = register(&router, &params).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the refusal wording for this cause, got {body}"
        );
        let after = own_image_ids(&router, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused register stores nothing, so the account's images must be \
             unchanged, got {after:?}"
        );
    }
}

/// RULE, on the state a register writes rather than on its answer: the snapshot a registered
/// AMI restores a device from is one the account cannot delete while that AMI is registered,
/// and the UEFI variable store the register supplied is the one that is read back.
///
/// The rows differ along which device of the AMI the snapshot backs (its root device and a
/// data device), so the dependency is the AMI's rather than one device's.
#[tokio::test]
async fn a_registered_amis_snapshots_are_in_use_and_its_uefi_data_reads_back() {
    let (router, _engine) = super::ec2_query::emulator();

    // "Base64 representation of the non-volatile UEFI variable store" — a value only a
    // register can give an AMI, read back through the attribute that reports it.
    let uefi_data = "dGhlIHZhcmlhYmxlIHN0b3Jl";

    let rows: Vec<(&str, &str)> = vec![
        ("the root device of the AMI", LINUX_ROOT_DEVICE_NAME),
        ("a data device of the AMI", "/dev/sdf"),
    ];

    for (case, device_name) in rows {
        let snapshot = snapshot_of(&router, case, 8).await;
        let name = format!("register-image-in-use-{}", device_name.replace('/', "-"));
        let mut params = mapping(1, LINUX_ROOT_DEVICE_NAME, &[ebs_snapshot(&snapshot)]);
        if device_name != LINUX_ROOT_DEVICE_NAME {
            // A second device restoring the same snapshot: the root device is mapped from a
            // snapshot of its own, so the AMI depends on both.
            let root_snapshot = snapshot_of(&router, case, 8).await;
            params = mapping(1, LINUX_ROOT_DEVICE_NAME, &[ebs_snapshot(&root_snapshot)])
                .into_iter()
                .chain(mapping(2, device_name, &[ebs_snapshot(&snapshot)]))
                .collect();
        }
        params.extend([
            p("Name", &name),
            p("RootDeviceName", LINUX_ROOT_DEVICE_NAME),
            p("UefiData", uefi_data),
        ]);
        let image_id = registered(&router, case, &params).await;

        // "You cannot delete a snapshot of the root device of an EBS volume used by a
        // registered AMI. You must first deregister the AMI before you can delete the
        // snapshot": the AMI the register wrote is what makes the snapshot undeletable.
        let (status, body) = op(&router, "DeleteSnapshot", &[p("SnapshotId", &snapshot)]).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} deleting the in-use {snapshot}, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_SNAPSHOT_IN_USE,
            "case {case:?}: expected {INVALID_SNAPSHOT_IN_USE} deleting {snapshot}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The snapshot {snapshot} is currently in use by {image_id}"),
            "case {case:?}: expected the refusal to name the snapshot and the AMI, got {body}"
        );

        // The UEFI variable store the register supplied, read back through the attribute that
        // reports it — the `Image` shape carries no member for it.
        let (status, body) = op(
            &router,
            "DescribeImageAttribute",
            &[p("ImageId", &image_id), p("Attribute", "uefiData")],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeImageAttribute, got {status} with \
             body {body}"
        );
        assert_eq!(
            element(&body, "value").as_deref(),
            Some(uefi_data),
            "case {case:?}: expected the registered UEFI data back, got {body}"
        );
    }
}
