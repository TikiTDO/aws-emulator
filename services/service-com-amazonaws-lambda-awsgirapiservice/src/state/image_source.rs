// SPDX-License-Identifier: CC0-1.0

//! Resolving the CONTAINER IMAGE a function's package names, against the registry that
//! holds it, and whether that registry lets this service pull it.
//!
//! "For a container image, the code property must include the URI of a container image in
//! the Amazon ECR registry" (`CreateFunction`): a container-image package is a reference
//! into Amazon ECR, and Lambda follows that reference when the package is stored. Three
//! things come out of following it, and all of them are settled by the call that WRITES the
//! package rather than by an invocation:
//!
//! 1. whether the reference names an image at all — a URI that is no image of the
//!    function's own Region's registry is refused, and nothing is created, so a customer
//!    learns about a mistyped tag or Region from the refusal instead of from a function
//!    that can never start;
//! 2. whether the registry lets THIS SERVICE pull that image — Lambda fetches the image
//!    with its own service principal ([`crate::state::EXECUTION_ROLE_PRINCIPAL`]), so a
//!    repository whose own policy denies that principal the pull refuses the write as a
//!    permission failure ([`ImageSourceRefusal::NotPermitted`]) rather than as a bad
//!    reference;
//! 3. WHICH image bytes the function is frozen to — the manifest digest the registry holds
//!    for the reference, which is what `FunctionCodeLocation$ResolvedImageUri` pins the
//!    repository to, and so what a client audits the function against
//!    (`DescribeImages`/`BatchGetImage` report that same digest).
//!
//! The registry is the Amazon ECR emulator's OWN store, reached through the SAME shared
//! engine this service's store lives in, so the digest a function reports is the one
//! `PutImage` minted rather than a second copy of it, and the pull decision is the one that
//! registry takes for any other principal — the same seam
//! [`super::event_source_authorization`] reads IAM's roles through.
//!
//! A reference addresses that registry either at one of AWS's own registry hosts
//! (`<account-id>.dkr.ecr.<region>.<domain>`) or at the address the registry ANNOUNCES to a
//! client that pushed the image here: `Repository$repositoryUri` is "The URI for the
//! repository. You can use this URI for container image `push` and `pull` operations", and
//! on this emulator that URI is built on the endpoint the emulator answers the registry at,
//! because AWS's own registry host resolves nowhere on the machine it runs on. A reference
//! carrying the URI a client pushed to is therefore a reference to this registry, and is
//! followed exactly like the AWS-hosted spelling ([`emulator_registry_authorities`]).
//!
//! Nothing is refused on evidence this emulator does not have: an engine carrying no ECR
//! store, and a registry belonging to another account, show nothing about the image and
//! let the package through ([`ImageResolution::Unknown`]).

/// The registry store an image reference is resolved against: Amazon ECR's OWN state
/// struct, in the shared engine.
pub type EcrState =
    ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::ComAmazonawsEcrAmazonec2containerregistryV20150921State;

/// The ECR store in the SAME shared engine. `None` means the engine carries no such
/// store — a deployment with no registry to ask, which is evidence of nothing.
fn ecr_state<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<::engine::ServiceState<EcrState>> {
    ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::service_state(engine)
}

/// What following a container-image reference established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageResolution {
    /// The registry holds the image: the repository it is held in, as the registry keys
    /// one, and its manifest digest exactly as the registry minted it (`sha256:<hex>`).
    Resolved {
        /// The repository of this registry the reference named.
        repository: String,
        /// The manifest digest the registry holds for the reference.
        digest: String,
    },
    /// The reference is no image URI of this function's Region's registry: Lambda resolves
    /// a package in the function's own Region, so a URI naming another Region's registry,
    /// or no registry at all, is not a source image for it.
    NotValid,
    /// A reference into this Region's registry that the registry holds no image for — an
    /// unknown repository, or a tag or digest nothing was pushed under.
    Missing,
    /// Nothing of the kind can be shown: no registry is mounted to ask, or the reference
    /// names a registry of another account, which this emulator holds none of.
    Unknown,
}

/// Why a container-image package was not stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSourceRefusal {
    /// The reference names no image this registry can be pulled from, which is a bad
    /// parameter value: the sentence it is refused with ([`source_image_message`]).
    NotUsable(String),
    /// The image is there and the registry does not let this service pull it, which is a
    /// permission failure: the sentence it is refused with
    /// ([`IMAGE_ACCESS_DENIED_MESSAGE`]).
    NotPermitted(String),
}

/// The sentence Lambda refuses an image its own service principal may not pull with.
///
/// Wholly fixed text: what the caller has to fix is the repository's permissions, which the
/// sentence names as a class rather than by repeating the reference back.
pub const IMAGE_ACCESS_DENIED_MESSAGE: &str =
    "Lambda does not have permission to access the ECR image. Check the ECR permissions.";

/// The sentence Lambda refuses a source image it cannot use with.
///
/// Fixed text around the URI the request named, which is the one part of it that varies:
/// a reference that is no image URI of the function's Region is "not valid", and one that
/// is but names nothing pushed "does not exist".
#[must_use]
pub fn source_image_message(image_uri: &str, resolution: &ImageResolution) -> Option<String> {
    match resolution {
        ImageResolution::NotValid => Some(format!(
            "Source image {image_uri} is not valid. Provide a valid source image."
        )),
        ImageResolution::Missing => Some(format!(
            "Source image {image_uri} does not exist. Provide a valid source image."
        )),
        ImageResolution::Resolved { .. } | ImageResolution::Unknown => None,
    }
}

/// Follow `image_uri` into the registry of a function being written in `region`, and check
/// the pull against the repository it names: the manifest digest the function is frozen to,
/// `None` where the registry shows nothing about the image, and the refusal otherwise.
///
/// The ONE reading both writes that store a container-image package take, so a create and a
/// code update resolve a reference and decide a pull alike.
///
/// A READ of the registry: it decides and pins, and changes nothing there.
pub fn image_package_digest<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    image_uri: &str,
) -> Result<Option<String>, ImageSourceRefusal> {
    let resolution =
        resolve_image_source(engine, &emulator_registry_authorities(ctx), region, image_uri);
    if let Some(message) = source_image_message(image_uri, &resolution) {
        return Err(ImageSourceRefusal::NotUsable(message));
    }
    let ImageResolution::Resolved { repository, digest } = resolution else {
        return Ok(None);
    };
    // "Lambda needs to pull the image" with its own service principal, which the registry
    // decides over the repository's own policy: an explicit deny there is this service
    // being refused the image rather than the image being wrong.
    if ::service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::registry_pull::service_pull_denied_by_repository_policy(
        engine,
        region,
        &repository,
        super::EXECUTION_ROLE_PRINCIPAL,
    )
    .is_some()
    {
        return Err(ImageSourceRefusal::NotPermitted(
            IMAGE_ACCESS_DENIED_MESSAGE.to_string(),
        ));
    }
    Ok(Some(digest))
}

/// The `host[:port]` authorities an image URI addresses THIS emulator's registry by, beside
/// AWS's own registry hosts: the endpoint the request reached this emulator on, and the
/// address this emulator is served at.
///
/// Those are the two addresses the registry builds a `repositoryUri` on, read through the
/// tree's own readings of them (`service_common::data_plane_addressing`), so a reference
/// carrying the URI a client was handed for a push is followed here. One endpoint serves
/// every Region, so such a reference names no Region of its own and is resolved in the
/// function's.
#[must_use]
pub fn emulator_registry_authorities(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Vec<String> {
    let parts = ctx.parts();
    let reached = ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    );
    let served = ::service_common::data_plane_addressing::emulator_endpoint_authority(ctx);
    let mut authorities = vec![served];
    if let Some(reached) = reached.filter(|reached| !authorities.contains(reached)) {
        authorities.push(reached);
    }
    authorities
}

/// Follow `image_uri` into the registry of a function being written in `region`, where
/// `registry_authorities` are the addresses that registry is also reached at
/// ([`emulator_registry_authorities`]).
///
/// A READ of the registry: it decides and pins, and changes nothing there.
#[must_use]
pub fn resolve_image_source<Eng: Send + Sync + 'static>(
    engine: &Eng,
    registry_authorities: &[String],
    region: &str,
    image_uri: &str,
) -> ImageResolution {
    let Some(reference) = EcrImageRef::parse(image_uri, registry_authorities) else {
        return ImageResolution::NotValid;
    };
    if let Some(registry) = reference.registry {
        // "the URI of a container image in the Amazon ECR registry" is followed in the
        // function's OWN Region: a registry of another Region holds images this function
        // could never be given, so such a URI is no source image for it.
        if registry.region != region {
            return ImageResolution::NotValid;
        }
        // A registry of another account is one this emulator holds nothing for, so it shows
        // nothing about the image.
        if registry.registry_id != ::engine::emulator_account_id() {
            return ImageResolution::Unknown;
        }
    }
    let Some(ecr) = ecr_state(engine) else {
        return ImageResolution::Unknown;
    };
    let registry = ecr.lock();
    let Some(repository) = registry.repositories.get(reference.repository) else {
        return ImageResolution::Missing;
    };
    // The digest is the registry's own, for whichever way the reference addressed the
    // image: a tag is followed through the repository's tag index, and a digest has to name
    // an image the repository really holds.
    let digest = match reference.selector {
        ImageSelector::Tag(tag) => repository.tag_index.get(tag).cloned(),
        ImageSelector::Digest(digest) => repository
            .images
            .get(digest)
            .map(|image| image.digest.clone()),
    };
    match digest {
        Some(digest) => ImageResolution::Resolved {
            repository: reference.repository.to_string(),
            digest,
        },
        None => ImageResolution::Missing,
    }
}

/// Which image of a repository a reference addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageSelector<'a> {
    /// `<repository>:<tag>`, or a reference carrying neither tag nor digest, which names
    /// the registry's default tag ([`DEFAULT_IMAGE_TAG`]).
    Tag(&'a str),
    /// `<repository>@<digest>`.
    Digest(&'a str),
}

/// The tag a reference that names none addresses, which is the registry convention a
/// client's `docker push` of a bare repository writes.
const DEFAULT_IMAGE_TAG: &str = "latest";

/// The host labels an Amazon ECR registry's URI carries between the account id and the
/// Region: `<account-id>.dkr.ecr.<region>.<domain>`.
const ECR_HOST_LABELS: [&str; 2] = ["dkr", "ecr"];

/// How many characters an account id is rendered in, which is the width an ECR registry
/// URI's first host label has.
const ACCOUNT_ID_LEN: usize = 12;

/// The registry an AWS registry host names: the account whose registry it is, and the
/// Region that registry serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AwsRegistry<'a> {
    /// The account whose registry holds the image — the URI's first host label.
    registry_id: &'a str,
    /// The Region of that registry.
    region: &'a str,
}

/// An image URI broken into the parts the registry is addressed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EcrImageRef<'a> {
    /// The registry the reference addresses, when its host names one of AWS's own —
    /// `None` for a reference addressing the endpoint this emulator answers the registry
    /// at, which names neither an account nor a Region.
    registry: Option<AwsRegistry<'a>>,
    /// The repository inside it, which may itself be namespaced (`team/app`).
    repository: &'a str,
    /// Which image of that repository the reference addresses.
    selector: ImageSelector<'a>,
}

impl<'a> EcrImageRef<'a> {
    /// `image_uri` broken up, or `None` when it addresses no registry this emulator
    /// serves: neither one of AWS's own registry hosts nor an address in
    /// `registry_authorities`.
    fn parse(image_uri: &'a str, registry_authorities: &[String]) -> Option<Self> {
        let (authority, path) = image_uri.split_once('/')?;
        // The AWS-hosted spelling first, so a registry host this emulator is also reached
        // at keeps naming the account and the Region it spells out.
        let registry = match aws_registry(authority) {
            Some(registry) => Some(registry),
            // A host is case-insensitive, so the address a client was handed is matched
            // however it was written back.
            None
                if registry_authorities
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(authority)) =>
            {
                None
            }
            None => return None,
        };
        let (repository, selector) = split_reference(path)?;
        if repository.is_empty() {
            return None;
        }
        Some(Self {
            registry,
            repository,
            selector,
        })
    }
}

/// The AWS registry `authority` names (`<account-id>.dkr.ecr.<region>.<domain>`), or `None`
/// when it is no such host.
fn aws_registry(authority: &str) -> Option<AwsRegistry<'_>> {
    let mut labels = authority.split('.');
    let registry_id = labels.next()?;
    if registry_id.len() != ACCOUNT_ID_LEN || !registry_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    for expected in ECR_HOST_LABELS {
        if labels.next()? != expected {
            return None;
        }
    }
    let region = labels.next()?;
    if region.is_empty() {
        return None;
    }
    // Whatever domain the partition's registries are served under follows; what makes
    // this a registry URI is the labels above, not the domain.
    labels.next()?;
    Some(AwsRegistry {
        registry_id,
        region,
    })
}

/// The repository and the image it addresses, out of the path part of an image URI.
///
/// A digest is separated by `@`, and a tag by the LAST `:` of the final path segment, so a
/// namespaced repository is not read as a tag.
fn split_reference(path: &str) -> Option<(&str, ImageSelector<'_>)> {
    if let Some((repository, digest)) = path.split_once('@') {
        if digest.is_empty() {
            return None;
        }
        return Some((repository, ImageSelector::Digest(digest)));
    }
    match path.rsplit_once(':') {
        Some((repository, tag)) if !tag.contains('/') => {
            if tag.is_empty() {
                return None;
            }
            Some((repository, ImageSelector::Tag(tag)))
        }
        _ => Some((path, ImageSelector::Tag(DEFAULT_IMAGE_TAG))),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        source_image_message, split_reference, EcrImageRef, ImageResolution, ImageSelector,
    };

    /// The address this emulator's registry announces itself at in these rows, which is the
    /// one a client that pushed an image here was handed.
    const ANNOUNCED: &str = "127.0.0.1:9297";

    /// The parts a registry URI breaks into: the account and the Region of the registry
    /// (none for the announced address), the repository, and the image it addresses.
    type Parts<'a> = (Option<(&'a str, &'a str)>, &'a str, ImageSelector<'a>);

    /// A registry URI breaks into the registry, the repository and the image it addresses;
    /// anything that is not a URI of a registry this emulator serves breaks into nothing.
    #[test]
    fn a_registry_uri_names_a_registry_a_repository_and_an_image() {
        let announced = [ANNOUNCED.to_string()];
        // (case, the URI, the parsed parts)
        let rows: [(&str, &str, Option<Parts<'_>>); 12] = [
            (
                "a tag",
                "000000000000.dkr.ecr.us-west-2.amazonaws.com/app:v1",
                Some((
                    Some(("000000000000", "us-west-2")),
                    "app",
                    ImageSelector::Tag("v1"),
                )),
            ),
            (
                "a namespaced repository",
                "000000000000.dkr.ecr.eu-west-1.amazonaws.com/team/app:latest",
                Some((
                    Some(("000000000000", "eu-west-1")),
                    "team/app",
                    ImageSelector::Tag("latest"),
                )),
            ),
            (
                "no tag at all, which names the default one",
                "000000000000.dkr.ecr.us-east-1.amazonaws.com/app",
                Some((
                    Some(("000000000000", "us-east-1")),
                    "app",
                    ImageSelector::Tag("latest"),
                )),
            ),
            (
                "a digest",
                "000000000000.dkr.ecr.us-east-2.amazonaws.com/app@sha256:abc",
                Some((
                    Some(("000000000000", "us-east-2")),
                    "app",
                    ImageSelector::Digest("sha256:abc"),
                )),
            ),
            (
                "another partition's domain",
                "000000000000.dkr.ecr.cn-north-1.amazonaws.com.cn/app:v1",
                Some((
                    Some(("000000000000", "cn-north-1")),
                    "app",
                    ImageSelector::Tag("v1"),
                )),
            ),
            (
                "the address the registry announced, with a digest",
                "127.0.0.1:9297/app@sha256:abc",
                Some((None, "app", ImageSelector::Digest("sha256:abc"))),
            ),
            (
                "the announced address, with a tag",
                "127.0.0.1:9297/team/app:v1",
                Some((None, "team/app", ImageSelector::Tag("v1"))),
            ),
            ("another endpoint entirely", "127.0.0.1:1/app:v1", None),
            ("no registry host", "app:v1", None),
            (
                "a public registry, which is not this one",
                "public.ecr.aws/nginx/nginx:latest",
                None,
            ),
            (
                "a registry id that is no account id",
                "registry.dkr.ecr.us-west-2.amazonaws.com/app:v1",
                None,
            ),
            (
                "a host short of the registry labels",
                "000000000000.ecr.us-west-2.amazonaws.com/app:v1",
                None,
            ),
        ];
        for (case, uri, expected) in rows {
            let parsed = EcrImageRef::parse(uri, &announced).map(|reference| {
                (
                    reference
                        .registry
                        .map(|registry| (registry.registry_id, registry.region)),
                    reference.repository,
                    reference.selector,
                )
            });
            assert_eq!(
                parsed, expected,
                "case {case:?}: expected {expected:?}, got {parsed:?}"
            );
        }
    }

    /// A reference with an empty tag or digest addresses no image, so it parses as none.
    #[test]
    fn an_empty_reference_addresses_no_image() {
        for path in ["app:", "app@"] {
            let split = split_reference(path);
            assert!(
                split.is_none(),
                "case {path:?}: expected no image to be addressed, got {split:?}"
            );
        }
    }

    /// The two refusals are fixed text around the URI the request named, and a resolution
    /// that refuses nothing words nothing.
    #[test]
    fn the_refusal_is_fixed_text_around_the_requested_uri() {
        let uri = "000000000000.dkr.ecr.us-west-2.amazonaws.com/app:v1";
        // (case, the resolution, the sentence it is refused with)
        let rows: [(&str, ImageResolution, Option<String>); 4] = [
            (
                "no image URI of this Region's registry",
                ImageResolution::NotValid,
                Some(format!(
                    "Source image {uri} is not valid. Provide a valid source image."
                )),
            ),
            (
                "nothing pushed under the reference",
                ImageResolution::Missing,
                Some(format!(
                    "Source image {uri} does not exist. Provide a valid source image."
                )),
            ),
            (
                "an image the registry holds",
                ImageResolution::Resolved {
                    repository: "app".to_string(),
                    digest: "sha256:abc".to_string(),
                },
                None,
            ),
            ("no registry to ask", ImageResolution::Unknown, None),
        ];
        for (case, resolution, expected) in rows {
            let worded = source_image_message(uri, &resolution);
            assert_eq!(
                worded, expected,
                "case {case:?}: expected {expected:?}, got {worded:?}"
            );
        }
    }
}
