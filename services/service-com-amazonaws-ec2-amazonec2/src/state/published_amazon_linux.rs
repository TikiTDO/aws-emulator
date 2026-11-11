// SPDX-License-Identifier: CC0-1.0

//! The Amazon Linux images Amazon publishes, and the public Systems Manager parameter name
//! each one's id is published under.
//!
//! ONE ROW PER PUBLISHED ALIAS. Amazon publishes a parameter under
//! [`AMAZON_LINUX_ALIAS_NAMESPACE`] for every variant of every Amazon Linux family it
//! publishes an image of — the family, the `minimal` variant, the kernel version the name
//! pins, the architecture, and the root-device form the name ends in — and a client that
//! cannot hardcode a per-Region AMI id discovers an image by enumerating that hierarchy
//! (`Image$PublicSsmParameterName`: "the name of the public Systems Manager parameter that
//! resolves to this AMI, under the `aws/service/` namespace"). The alias namespace is
//! therefore the POPULATION of this table: a variant with no row here is a name a client
//! enumerating the hierarchy never sees, and a row with no image behind it is an alias that
//! resolves to nothing.
//!
//! No id is written here. Each row is an [`ImageSpec`] of the published catalog
//! ([`super::published_image_specs`]) carrying its alias in `Image$PublicSsmParameterName`,
//! so the ONE place this emulator records which AMI an alias names stays the image
//! catalogue itself
//! ([`super::ComAmazonawsEc2Amazonec2State::public_ssm_parameter_image_ids`]) and the id a
//! Parameter Store read reports is an AMI the account can really describe and launch. The
//! second name Amazon publishes an image under — its family's `kernel-default` alias — is
//! [`PUBLIC_SSM_PARAMETER_ALIASES`], which resolves to the same id rather than to a second
//! image.
//!
//! The name forms are Amazon's own (Amazon Linux 2023 User Guide, "Find an AMI"; Amazon EC2
//! User Guide, "Use a Systems Manager parameter instead of an AMI ID"):
//!
//! ```text
//! amzn-ami[-minimal]-{hvm,pv}-<version>-x86_64-{ebs,gp2,s3}
//! amzn2-ami[-minimal][-kernel-<kernel>]-hvm-<version>-<architecture>-{ebs,gp2}
//! al2023-ami[-minimal]-<version>-kernel-<kernel>-<architecture>
//! ```
//!
//! and an alias is that name with the release version dropped out of it. WHICH releases,
//! kernels and build dates the catalog holds is service-published data that no model
//! describes: it is recorded behaviour, so the tables below carry what AWS publishes rather
//! than anything derived.

use super::images_and_zone_facts::published_amazon_image;
use super::{
    ImageSpec, BOOT_MODE_LEGACY_BIOS, BOOT_MODE_UEFI, BOOT_MODE_UEFI_PREFERRED,
    IMAGE_ROOT_DEVICE_TYPE_EBS, IMAGE_ROOT_DEVICE_TYPE_INSTANCE_STORE,
    IMAGE_ROOT_VOLUME_TYPE_GP2, IMAGE_ROOT_VOLUME_TYPE_STANDARD,
    IMAGE_VIRTUALIZATION_TYPE_HVM, IMAGE_VIRTUALIZATION_TYPE_PARAVIRTUAL, IMDS_SUPPORT_V2,
    INSTANCE_STORE_ROOT_DEVICE_NAME, LINUX_ROOT_DEVICE_NAME,
};

/// The level of the public `aws/service/` namespace Amazon publishes its Amazon Linux AMI
/// aliases at.
pub const AMAZON_LINUX_ALIAS_NAMESPACE: &str = "/aws/service/ami-amazon-linux-latest";

/// The architectures Amazon publishes both of today's Amazon Linux families for, in the
/// order the catalog publishes a release's builds in.
const PUBLISHED_ARCHITECTURES: &[&str] = &["x86_64", "arm64"];

/// The 64-bit x86 architecture, which is the only one the retired families below were
/// published for.
const ARCHITECTURE_X86_64: &str = "x86_64";

/// The 64-bit Arm architecture, whose images boot UEFI.
const ARCHITECTURE_ARM64: &str = "arm64";

/// The published name of an image whose root volume is a `gp2` EBS volume.
const FORM_GP2: &str = "gp2";

/// The published name of an instance-store-backed image: its root file system is delivered
/// from Amazon S3 rather than restored from an EBS snapshot.
const FORM_S3: &str = "s3";

/// The `hvm` virtualization, as a published name spells it.
const VIRTUALIZATION_HVM: &str = "hvm";

/// The paravirtual virtualization, as a published name spells it.
const VIRTUALIZATION_PV: &str = "pv";

/// The public parameter name the alias `leaf` is published under.
fn alias(leaf: &str) -> String {
    format!("{AMAZON_LINUX_ALIAS_NAMESPACE}/{leaf}")
}

/// The `-minimal` the name of a minimal variant carries, and the ` Minimal` its description
/// ends with.
fn minimal_tokens(minimal: bool) -> (&'static str, &'static str) {
    if minimal {
        ("-minimal", " Minimal")
    } else {
        ("", "")
    }
}

/// The EBS volume type the published root-device form `form` names, and the root device
/// type it implies.
///
/// `gp2` names a general-purpose SSD root volume and `ebs` the magnetic `standard` volume
/// the form predates it with — both EBS-backed — while `s3` names an instance-store-backed
/// image, which restores no volume at all.
fn root_device(form: &str) -> (&'static str, &'static str) {
    match form {
        FORM_S3 => (
            IMAGE_ROOT_VOLUME_TYPE_STANDARD,
            IMAGE_ROOT_DEVICE_TYPE_INSTANCE_STORE,
        ),
        FORM_GP2 => (IMAGE_ROOT_VOLUME_TYPE_GP2, IMAGE_ROOT_DEVICE_TYPE_EBS),
        _ => (
            IMAGE_ROOT_VOLUME_TYPE_STANDARD,
            IMAGE_ROOT_DEVICE_TYPE_EBS,
        ),
    }
}

/// The root device name an image of root device type `root_device_type` carries.
fn root_device_name(root_device_type: &str) -> String {
    if root_device_type == IMAGE_ROOT_DEVICE_TYPE_INSTANCE_STORE {
        INSTANCE_STORE_ROOT_DEVICE_NAME.to_string()
    } else {
        LINUX_ROOT_DEVICE_NAME.to_string()
    }
}

/// The boot mode an HVM image of `architecture` declares: 64-bit Arm boots UEFI, and x86
/// boots whichever the instance type supports where the family declares a preference and
/// the legacy BIOS where it does not.
fn hvm_boot_mode(architecture: &str, uefi_preferred_on_x86: bool) -> String {
    if architecture == ARCHITECTURE_ARM64 {
        BOOT_MODE_UEFI.to_string()
    } else if uefi_preferred_on_x86 {
        BOOT_MODE_UEFI_PREFERRED.to_string()
    } else {
        BOOT_MODE_LEGACY_BIOS.to_string()
    }
}

// ---------------------------------------------------------------------------
// Amazon Linux: `amzn-ami-…`
// ---------------------------------------------------------------------------

/// The Amazon Linux release every `amzn-ami-*` row below is a build of.
const AL1_RELEASE: &str = "2018.03.0.20230719.0";

/// The `amzn-ami-*` variants Amazon publishes an alias for:
/// `(virtualization, root-device form, minimal, CreationDate)`.
///
/// The combinations are the published ones, not every product of the three: Amazon
/// publishes no `gp2` build of the minimal or the paravirtual variants. One release's
/// variants are built minutes apart, which is what their creation dates carry.
const AL1_VARIANTS: &[(&str, &str, bool, &str)] = &[
    (VIRTUALIZATION_HVM, "ebs", false, "2023-07-19T18:12:21.000Z"),
    (VIRTUALIZATION_HVM, FORM_GP2, false, "2023-07-19T18:14:08.000Z"),
    (VIRTUALIZATION_HVM, FORM_S3, false, "2023-07-19T18:16:55.000Z"),
    (VIRTUALIZATION_HVM, "ebs", true, "2023-07-19T18:19:31.000Z"),
    (VIRTUALIZATION_HVM, FORM_S3, true, "2023-07-19T18:21:07.000Z"),
    (VIRTUALIZATION_PV, "ebs", false, "2023-07-19T18:23:44.000Z"),
    (VIRTUALIZATION_PV, FORM_S3, false, "2023-07-19T18:25:12.000Z"),
    (VIRTUALIZATION_PV, "ebs", true, "2023-07-19T18:27:36.000Z"),
    (VIRTUALIZATION_PV, FORM_S3, true, "2023-07-19T18:29:50.000Z"),
];

/// The published Amazon Linux builds, one per [`AL1_VARIANTS`] row.
fn amazon_linux_specs() -> Vec<ImageSpec> {
    AL1_VARIANTS
        .iter()
        .map(|&(virtualization, form, minimal, creation_date)| {
            let (minimal_name, minimal_description) = minimal_tokens(minimal);
            let (root_volume_type, root_device_type) = root_device(form);
            let paravirtual = virtualization == VIRTUALIZATION_PV;
            let name = format!(
                "amzn-ami{minimal_name}-{virtualization}-{AL1_RELEASE}-{ARCHITECTURE_X86_64}-{form}"
            );
            ImageSpec {
                description: format!(
                    "Amazon Linux AMI {AL1_RELEASE} {ARCHITECTURE_X86_64} {} {form}{minimal_description}",
                    virtualization.to_uppercase()
                ),
                // A paravirtual image declares no boot mode at all: `Image$BootMode` is an
                // HVM boot facility.
                boot_mode: (!paravirtual)
                    .then(|| hvm_boot_mode(ARCHITECTURE_X86_64, false)),
                // Enhanced networking with ENA is an HVM facility too, so a paravirtual row
                // reports it as not enabled, which is what the `ena-support` filter of
                // `DescribeImages` selects those rows on.
                ena_support: Some(!paravirtual),
                virtualization_type: if paravirtual {
                    IMAGE_VIRTUALIZATION_TYPE_PARAVIRTUAL.to_string()
                } else {
                    IMAGE_VIRTUALIZATION_TYPE_HVM.to_string()
                },
                root_volume_type: root_volume_type.to_string(),
                root_device_type: root_device_type.to_string(),
                root_device_name: root_device_name(root_device_type),
                public_ssm_parameter_name: Some(alias(&format!(
                    "amzn-ami{minimal_name}-{virtualization}-{ARCHITECTURE_X86_64}-{form}"
                ))),
                ..published_amazon_image(&name, ARCHITECTURE_X86_64, creation_date)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Amazon Linux 2: `amzn2-ami-…`
// ---------------------------------------------------------------------------

/// The Amazon Linux 2 release every `amzn2-ami-*` row below is a build of.
const AL2_RELEASE: &str = "2.0.20241113.1";

/// The kernel version an Amazon Linux 2 name that pins none is built on \u2014 the family's
/// original kernel, which is why Amazon publishes that build under both the unqualified and
/// the `kernel-4.14` names.
const AL2_ORIGINAL_KERNEL: &str = "4.14";

/// The kernel version the family's `kernel-default` alias names today.
const AL2_DEFAULT_KERNEL: &str = "5.10";

/// The `amzn2-ami-*` variants Amazon publishes an alias for:
/// `(kernel the name pins, minimal, architecture, root-device form, CreationDate)`.
///
/// The combinations are the published ones rather than every product of the four. All of
/// them come out of one release, built minutes apart, so a client resolving the family by
/// name pattern picks the newest build of the variant it asked for.
const AL2_VARIANTS: &[(Option<&str>, bool, &str, &str, &str)] = &[
    (None, false, ARCHITECTURE_X86_64, FORM_GP2, "2024-11-13T18:17:12.000Z"),
    (None, false, ARCHITECTURE_ARM64, FORM_GP2, "2024-11-13T18:19:04.000Z"),
    (None, false, ARCHITECTURE_X86_64, "ebs", "2024-11-13T18:01:10.000Z"),
    (None, true, ARCHITECTURE_X86_64, "ebs", "2024-11-13T18:21:48.000Z"),
    (None, true, ARCHITECTURE_ARM64, "ebs", "2024-11-13T18:02:25.000Z"),
    (
        Some(AL2_ORIGINAL_KERNEL),
        false,
        ARCHITECTURE_X86_64,
        FORM_GP2,
        "2024-11-13T18:03:41.000Z",
    ),
    (
        Some(AL2_ORIGINAL_KERNEL),
        false,
        ARCHITECTURE_X86_64,
        "ebs",
        "2024-11-13T18:04:58.000Z",
    ),
    (
        Some(AL2_ORIGINAL_KERNEL),
        false,
        ARCHITECTURE_ARM64,
        FORM_GP2,
        "2024-11-13T18:06:14.000Z",
    ),
    (
        Some(AL2_ORIGINAL_KERNEL),
        true,
        ARCHITECTURE_X86_64,
        "ebs",
        "2024-11-13T18:07:30.000Z",
    ),
    (
        Some(AL2_ORIGINAL_KERNEL),
        true,
        ARCHITECTURE_ARM64,
        "ebs",
        "2024-11-13T18:08:47.000Z",
    ),
    (
        Some(AL2_DEFAULT_KERNEL),
        false,
        ARCHITECTURE_X86_64,
        FORM_GP2,
        "2024-11-13T18:21:48.000Z",
    ),
    (
        Some(AL2_DEFAULT_KERNEL),
        false,
        ARCHITECTURE_X86_64,
        "ebs",
        "2024-11-13T18:10:03.000Z",
    ),
    (
        Some(AL2_DEFAULT_KERNEL),
        false,
        ARCHITECTURE_ARM64,
        FORM_GP2,
        "2024-11-13T18:11:19.000Z",
    ),
    (
        Some(AL2_DEFAULT_KERNEL),
        true,
        ARCHITECTURE_X86_64,
        FORM_GP2,
        "2024-11-13T18:12:36.000Z",
    ),
    (
        Some(AL2_DEFAULT_KERNEL),
        true,
        ARCHITECTURE_ARM64,
        FORM_GP2,
        "2024-11-13T18:13:52.000Z",
    ),
];

/// The published Amazon Linux 2 builds, one per [`AL2_VARIANTS`] row.
fn amazon_linux_2_specs() -> Vec<ImageSpec> {
    AL2_VARIANTS
        .iter()
        .map(|&(kernel, minimal, architecture, form, creation_date)| {
            let (minimal_name, minimal_description) = minimal_tokens(minimal);
            let kernel_name = kernel.map(|kernel| format!("-kernel-{kernel}")).unwrap_or_default();
            let kernel_description =
                kernel.map(|kernel| format!("Kernel {kernel} ")).unwrap_or_default();
            let (root_volume_type, root_device_type) = root_device(form);
            let name = format!(
                "amzn2-ami{minimal_name}{kernel_name}-hvm-{AL2_RELEASE}-{architecture}-{form}"
            );
            ImageSpec {
                description: format!(
                    "Amazon Linux 2 {kernel_description}AMI {AL2_RELEASE} {architecture} HVM \
                     {form}{minimal_description}"
                ),
                boot_mode: Some(hvm_boot_mode(architecture, false)),
                root_volume_type: root_volume_type.to_string(),
                root_device_type: root_device_type.to_string(),
                root_device_name: root_device_name(root_device_type),
                public_ssm_parameter_name: Some(alias(&format!(
                    "amzn2-ami{minimal_name}{kernel_name}-hvm-{architecture}-{form}"
                ))),
                ..published_amazon_image(&name, architecture, creation_date)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The kernel-named families: `al2023-ami-…` and the Amazon Linux 2027 preview
// ---------------------------------------------------------------------------

/// The family prefix the Amazon Linux 2023 names carry.
const AL2023_FAMILY: &str = "al2023";

/// How the Amazon Linux 2023 descriptions name the distribution.
const AL2023_DISTRIBUTION: &str = "Amazon Linux 2023";

/// The Amazon Linux 2023 releases the catalog publishes, each with the kernel its names pin
/// and the build date its version carries: `(kernel, release, CreationDate)`.
///
/// A family publishes one release per kernel it supports, every variant of a release sharing
/// that release's build date — so "the newest `CreationDate` of the images matching a name
/// pattern" picks the same build on every run.
const AL2023_RELEASES: &[(&str, &str, &str)] = &[
    ("6.1", "2023.6.20241121.0", "2024-11-21T04:40:03.000Z"),
    ("6.12", "2023.7.20250603.0", "2025-06-03T05:18:27.000Z"),
    ("6.18", "2023.8.20251104.0", "2025-11-04T05:52:41.000Z"),
];

/// The family prefix the Amazon Linux 2027 preview names carry.
const AL2027_FAMILY: &str = "al2027-preview";

/// How the Amazon Linux 2027 preview descriptions name the distribution.
const AL2027_DISTRIBUTION: &str = "Amazon Linux 2027 Preview";

/// The Amazon Linux 2027 preview releases the catalog publishes (see [`AL2023_RELEASES`]).
const AL2027_RELEASES: &[(&str, &str, &str)] = &[
    ("7.1", "2027.0.20260615.0", "2026-06-15T06:22:14.000Z"),
    ("7.2", "2027.0.20261020.0", "2026-10-20T07:05:38.000Z"),
];

/// The published builds of a family whose names pin a kernel version: every variant of
/// every release in `releases`, in both [`PUBLISHED_ARCHITECTURES`].
fn kernel_named_specs(
    family: &str,
    distribution: &str,
    releases: &[(&str, &str, &str)],
) -> Vec<ImageSpec> {
    let mut specs = Vec::new();
    for &(kernel, release, creation_date) in releases {
        for minimal in [false, true] {
            for architecture in PUBLISHED_ARCHITECTURES.iter().copied() {
                let (minimal_name, minimal_description) = minimal_tokens(minimal);
                let name =
                    format!("{family}-ami{minimal_name}-{release}-kernel-{kernel}-{architecture}");
                specs.push(ImageSpec {
                    description: format!(
                        "{distribution} AMI {release} {architecture} HVM \
                         kernel-{kernel}{minimal_description}"
                    ),
                    boot_mode: Some(hvm_boot_mode(architecture, true)),
                    // These families require IMDSv2.
                    imds_support: Some(IMDS_SUPPORT_V2.to_string()),
                    public_ssm_parameter_name: Some(alias(&format!(
                        "{family}-ami{minimal_name}-kernel-{kernel}-{architecture}"
                    ))),
                    ..published_amazon_image(&name, architecture, creation_date)
                });
            }
        }
    }
    specs
}

/// Every Amazon Linux build of the published catalog: one per published alias, across every
/// family Amazon publishes an Amazon Linux AMI alias for.
pub fn published_amazon_linux_specs() -> Vec<ImageSpec> {
    let mut specs = amazon_linux_specs();
    specs.extend(amazon_linux_2_specs());
    specs.extend(kernel_named_specs(
        AL2023_FAMILY,
        AL2023_DISTRIBUTION,
        AL2023_RELEASES,
    ));
    specs.extend(kernel_named_specs(
        AL2027_FAMILY,
        AL2027_DISTRIBUTION,
        AL2027_RELEASES,
    ));
    specs
}

/// The public Systems Manager parameter names Amazon publishes as a SECOND name for an image
/// [`published_amazon_linux_specs`] already publishes under one: `(the other name, the
/// published name that names the same AMI)`.
///
/// Amazon publishes the current build of a family both under a KERNEL-VERSION-PINNED name
/// and under a `kernel-default` name, and while that kernel is the family's default the two
/// resolve to the same AMI (Amazon EC2 User Guide, "Use a Systems Manager parameter instead
/// of an AMI ID", which lists the `/aws/service/ami-amazon-linux-latest/` names). The
/// default is the newest kernel each family publishes
/// ([`AL2023_RELEASES`], [`AL2027_RELEASES`], [`AL2_DEFAULT_KERNEL`]).
/// `Image$PublicSsmParameterName` carries one name per image, so the second name of an image
/// is recorded here and resolved to the same id by
/// [`super::ComAmazonawsEc2Amazonec2State::public_ssm_parameter_image_ids`] — rather than as
/// a second image record, which would make one AMI resolve to two ids.
pub const PUBLIC_SSM_PARAMETER_ALIASES: &[(&str, &str)] = &[
    (
        "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64",
        "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-6.18-x86_64",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-arm64",
        "/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-6.18-arm64",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/al2023-ami-minimal-kernel-default-x86_64",
        "/aws/service/ami-amazon-linux-latest/al2023-ami-minimal-kernel-6.18-x86_64",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/al2023-ami-minimal-kernel-default-arm64",
        "/aws/service/ami-amazon-linux-latest/al2023-ami-minimal-kernel-6.18-arm64",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/al2027-preview-ami-kernel-default-x86_64",
        "/aws/service/ami-amazon-linux-latest/al2027-preview-ami-kernel-7.2-x86_64",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/al2027-preview-ami-kernel-default-arm64",
        "/aws/service/ami-amazon-linux-latest/al2027-preview-ami-kernel-7.2-arm64",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/al2027-preview-ami-minimal-kernel-default-x86_64",
        "/aws/service/ami-amazon-linux-latest/al2027-preview-ami-minimal-kernel-7.2-x86_64",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/al2027-preview-ami-minimal-kernel-default-arm64",
        "/aws/service/ami-amazon-linux-latest/al2027-preview-ami-minimal-kernel-7.2-arm64",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-kernel-default-hvm-x86_64-gp2",
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-kernel-5.10-hvm-x86_64-gp2",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-kernel-default-hvm-x86_64-ebs",
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-kernel-5.10-hvm-x86_64-ebs",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-kernel-default-hvm-arm64-gp2",
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-kernel-5.10-hvm-arm64-gp2",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-minimal-kernel-default-hvm-x86_64-gp2",
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-minimal-kernel-5.10-hvm-x86_64-gp2",
    ),
    (
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-minimal-kernel-default-hvm-arm64-gp2",
        "/aws/service/ami-amazon-linux-latest/amzn2-ami-minimal-kernel-5.10-hvm-arm64-gp2",
    ),
];
