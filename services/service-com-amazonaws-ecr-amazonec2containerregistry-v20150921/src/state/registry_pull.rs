// SPDX-License-Identifier: CC0-1.0

//! Serving an image PULL this registry is asked for by a compute service, on a ROLE's
//! behalf, and the upstream import a pull through cache rule makes on the first one.
//!
//! A container runtime does not call this API: it logs in to the registry and fetches a
//! manifest. The emulated compute services are what stand in for that runtime — Amazon ECS
//! pulls the image its task definition names before it starts a container — and they reach
//! the registry in-process, through this module, so the registry answers one question
//! however it is asked:
//!
//! 1. **may this principal pull** — a pull takes the two calls the agent makes against the
//!    registry, the authorization token for the registry
//!    ([`GET_AUTHORIZATION_TOKEN_ACTION`], granted on nothing narrower than the registry)
//!    and the image manifest itself ([`BATCH_GET_IMAGE_ACTION`], granted on the repository)
//!    — decided over the documents in force on the role and the repository's own policy,
//!    through the same gate a signed request of this service goes through
//!    ([`super::authorization`]);
//! 2. **does this registry hold the image** — the repository the reference names and the
//!    tag or digest in it, resolved through the repository's own lookup
//!    ([`Repo::lookup_image`]), so an in-process pull sees exactly what `BatchGetImage`
//!    serves;
//! 3. **can it be imported** — "A pull through cache rule provides a way to cache images
//!    from an upstream registry source in your Amazon ECR private registry"
//!    (`CreatePullThroughCacheRule`): a reference under a rule's `ecrRepositoryPrefix` is
//!    served by caching the upstream image, so the FIRST pull creates the cache repository
//!    and stores the image under the tag that was asked for, and every later read of that
//!    repository — `DescribeImages`, `ListImages`, `BatchGetImage` — reports it.
//!
//! What an import stores is the registry's own record of the cached image: a manifest this
//! registry writes, describing one configuration blob it also stores, with the digest of
//! each computed over the bytes held ([`content_digest`]). No upstream registry is
//! contacted — this emulator makes no outbound connection — so no layer of the upstream
//! image is claimed to be here: the imported manifest references the configuration blob
//! alone, and the image's size is the size of what the repository really holds.
//!
//! Nothing is withheld on evidence this emulator does not have. An engine carrying no IAM
//! store, a role that store holds no record for, and a role carrying an attachment whose
//! document this account does not hold (an entry of AWS's own catalog) each show nothing
//! about the role, so the pull proceeds — the same rule every other gate over an execution
//! role's documents in this tree keeps. A pull made with no principal at all is not
//! decided either: a caller that names no role states nothing for a policy to be read
//! against.
//!
//! LOCK ORDER: the authorization decision is taken BEFORE this service's store is locked
//! (it reads the IAM store), and the resolution and the import hold this store's lock
//! alone. A caller reaches this with no lock of its own held.

use super::*;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// The registry-level call a pull begins with: "Retrieves an authorization token ... A
/// client can use the returned token to access any Amazon ECR registry that the IAM
/// principal has access to" (`GetAuthorizationToken`). It addresses the registry rather
/// than one repository, so it is decided on [`ANY_RESOURCE`].
pub const GET_AUTHORIZATION_TOKEN_ACTION: &str = "GetAuthorizationToken";

/// The call that fetches the image manifest a pull resolves: "Gets detailed information for
/// an image ... to pull an image" (`BatchGetImage`), decided on the repository the
/// reference names.
pub const BATCH_GET_IMAGE_ACTION: &str = "BatchGetImage";

/// The resource a registry-level action is decided on: a grant for it is written on
/// nothing narrower, which is how AWS's own task-execution policy states
/// `ecr:GetAuthorizationToken`.
const ANY_RESOURCE: &str = "*";

/// The tag a reference that names neither a tag nor a digest resolves to, which is the tag
/// a container runtime appends to a bare reference.
pub const DEFAULT_IMAGE_TAG: &str = "latest";

/// The media type of the configuration blob an imported manifest describes, in the OCI
/// spelling that goes with [`OCI_IMAGE_MANIFEST_MEDIA_TYPE`].
const OCI_IMAGE_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";

/// How many blobs a repository may hold before an IMPORT adds none.
///
/// See [`MAX_IMAGES_PER_REPOSITORY`]: the same reasoning for the blob map, which an import
/// grows by one entry per cached image, and which no other cap bounds the entry COUNT of.
/// It is set at the image cap because an import stores one configuration blob per image it
/// caches, so a repository at this bound is one whose images are capped too. Without it a
/// client pulling one tag after another under a cache rule would grow this process's heap
/// without end.
const MAX_IMPORTED_BLOBS_PER_REPOSITORY: usize = MAX_IMAGES_PER_REPOSITORY;

/// The separator a reference names a DIGEST after, as a registry client writes one
/// (`<repository>@sha256:<hex>`).
const DIGEST_SEPARATOR: char = '@';

/// An image reference that addresses THIS registry: which repository it names, in which
/// Region, and whether it asks for a tag or a digest.
///
/// Read from the reference alone ([`parse_reference`]), so the repository a pull resolves
/// is the one the reference spells rather than anything a caller passed beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryReference {
    /// The Region the registry host names, which the repository's ARN carries.
    pub region: String,
    /// The repository the reference names, as this registry keys one.
    pub repository_name: String,
    /// The tag the reference asks for, and `None` when it addresses a digest.
    pub tag: Option<String>,
    /// The digest the reference asks for, and `None` when it addresses a tag.
    pub digest: Option<String>,
}

impl RegistryReference {
    /// The `ImageIdentifier` this reference resolves through, which is the identifier
    /// `BatchGetImage` would carry for the same pull.
    fn identifier(&self) -> ImageIdentifier {
        ImageIdentifier {
            image_digest: self.digest.clone(),
            image_tag: self.tag.clone(),
        }
    }
}

/// Why this registry did not serve a pull.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullRefusal {
    /// No policy in force on the principal permits one of the calls the pull makes: the
    /// action that was withheld, and the sentence AWS refuses it with.
    NotAuthorized {
        /// The fully qualified IAM action (`ecr:<OperationName>`) that was withheld.
        action: String,
        /// The authorization refusal itself ([`super::authorization`]).
        message: String,
    },
    /// Nothing in this registry answers to the reference, and no pull through cache rule
    /// covers the repository it names: the message the repository's own lookup words.
    NotFound {
        /// Why no image was found, in this registry's own words.
        message: String,
    },
}

/// Serve the pull of `reference` made on behalf of `principal`, importing the upstream
/// image when a pull through cache rule covers the repository and the registry does not
/// hold it yet.
///
/// `principal` is the ARN of the ROLE the pull is made as — a task's execution role — and
/// `None` for a pull whose caller names none, which states nothing for a policy to be read
/// against. `now` is the instant the pull is made at, which is what an imported image
/// records as its push instant.
///
/// What the pull SERVED — the manifest and its digest — stays in this registry, where every
/// read of the repository reports it; the caller is told only whether it was served, because
/// that is what decides whether the container it is pulling for can start.
///
/// A WRITE, and only through the import: a pull of an image this registry already holds
/// changes nothing a later caller sees.
pub fn serve_pull<Eng: Send + Sync + 'static>(
    engine: &Eng,
    reference: &RegistryReference,
    principal: Option<&str>,
    now: SystemTime,
) -> Result<(), PullRefusal> {
    if let Some(refusal) = withheld_pull(engine, reference, principal) {
        return Err(refusal);
    }
    let Some(state) = service_state(engine) else {
        // No store of this service to ask: nothing is known about the image, so the pull
        // is not refused on it.
        return Ok(());
    };
    let mut store = state.lock();
    let identifier = reference.identifier();
    if let Some(repository) = store.repositories.get(&reference.repository_name) {
        if repository.lookup_image(&identifier).is_ok() {
            return Ok(());
        }
    }
    match import_through_cache_rule(&mut store, reference, now) {
        Some(_) => Ok(()),
        None => Err(PullRefusal::NotFound {
            message: not_found_message(&store, reference),
        }),
    }
}

/// The reference `image` names when it addresses a repository of THIS registry, and `None`
/// for every other reference — one in another account's registry, in another registry
/// altogether, or a bare name a container runtime resolves against a public registry.
///
/// The form is the one the Amazon ECR User Guide gives for a pull:
/// `<account>.dkr.ecr.<region>.amazonaws.com/<repository>[:<tag>|@<digest>]`, so the
/// Region and the repository are read out of the reference itself. A reference naming no
/// tag and no digest asks for [`DEFAULT_IMAGE_TAG`].
#[must_use]
pub fn parse_reference(image: &str) -> Option<RegistryReference> {
    let (host, path) = image.trim().split_once('/')?;
    let region = host_region(&host.to_ascii_lowercase())?;
    // A digest is written after `@`, a tag after the LAST `:` of the path — a repository
    // name carries neither character, so whichever is present decides which of the two the
    // reference asks for.
    if let Some((repository_name, digest)) = path.split_once(DIGEST_SEPARATOR) {
        if repository_name.is_empty() || digest.is_empty() {
            return None;
        }
        return Some(RegistryReference {
            region,
            repository_name: repository_name.to_string(),
            tag: None,
            digest: Some(digest.to_string()),
        });
    }
    let (repository_name, tag) = match path.rsplit_once(':') {
        Some((repository_name, tag)) if !repository_name.is_empty() && !tag.is_empty() => {
            (repository_name, tag.to_string())
        }
        Some(_) => return None,
        None => (path, DEFAULT_IMAGE_TAG.to_string()),
    };
    if repository_name.is_empty() {
        return None;
    }
    Some(RegistryReference {
        region,
        repository_name: repository_name.to_string(),
        tag: Some(tag),
        digest: None,
    })
}

/// The Region the registry host `host` names, when `host` is THIS registry's host: this
/// account's id, the two fixed labels, one Region label and the partition suffix. `host` is
/// already lowercased.
///
/// The host layout itself is read by the one walk of it this service states
/// ([`ecr_registry_host_parts`]); what this adds is that the account has to be the account
/// this registry serves — a repository of another account's registry is not ours to pull
/// from.
fn host_region(host: &str) -> Option<String> {
    let (account, region) = ecr_registry_host_parts(host)?;
    (account == registry_id()).then(|| region.to_string())
}

/// Why no image in this registry answers to `reference`: the repository is not here at all,
/// or it is and holds nothing the reference names — the two sentences this registry already
/// words for a request that addresses either ([`repository_not_found_message`],
/// [`Repo::resolve_image`]).
fn not_found_message(
    store: &ComAmazonawsEcrAmazonec2containerregistryV20150921State,
    reference: &RegistryReference,
) -> String {
    let identifier = reference.identifier();
    match store.repositories.get(&reference.repository_name) {
        None => repository_not_found_message(&reference.repository_name),
        // Only reached for a reference the resolution above did NOT answer, so the
        // resolution here is what words why. A repository that resolves it after all is
        // worded as this registry words an identifier that names no image of it, so no
        // branch reports anything but a sentence.
        Some(repository) => repository.resolve_image(&identifier).err().map_or_else(
            || not_within_repository("image", &repository.name, &repository.registry_id),
            |fault| fault.message().to_string(),
        ),
    }
}

/// The first of the calls a pull makes that no policy in force on `principal` permits, or
/// `None` when every one of them is permitted — or when nothing of the kind can be shown
/// (see the module docs).
fn withheld_pull<Eng: Send + Sync + 'static>(
    engine: &Eng,
    reference: &RegistryReference,
    principal: Option<&str>,
) -> Option<PullRefusal> {
    let role_arn = principal?;
    let iam = iam_store::service_state(engine)?;
    let (statements, boundary) = {
        let store = iam.lock();
        // A role this account's IAM holds no record for may belong to another account, so
        // its documents are not this store's to read.
        store.roles.values().find(|role| role.arn == role_arn)?;
        let policies = store.principal_policies(role_arn)?;
        // An attachment whose document this account does not hold — an entry of AWS's own
        // catalog — may be the very policy that carries the grant, so nothing is withheld
        // on it.
        if !policies.uncarried.is_empty() {
            return None;
        }
        (
            policies.documents,
            store.permissions_boundary_document(role_arn),
        )
    };
    let prepared = iam::prepare_documents(&statements);
    let capped = boundary.map(|document| iam::prepare_documents(::std::slice::from_ref(&document)));
    // The document the repository SHARES itself with, which is the other way a pull is
    // allowed: a repository policy naming the role grants it with no identity-based grant
    // at all.
    let shared = service_state(engine).and_then(|state| {
        let store = state.lock();
        store
            .repositories
            .get(&reference.repository_name)
            .and_then(|repository| repository.policy_text.clone())
    });
    // Every spelling a repository policy may name this role by: the role itself and the
    // account it belongs to, read the way every other service in this tree reads them.
    let mut principals = vec![role_arn.to_string()];
    principals.extend(iam::account_principals(role_arn));
    let repository = repository_arn(&reference.region, &reference.repository_name);
    // The registry-level call first, the way the agent makes them: a role that cannot
    // obtain a token never reaches a manifest.
    for (action, resource, repository_scoped) in [
        (GET_AUTHORIZATION_TOKEN_ACTION, ANY_RESOURCE, false),
        (BATCH_GET_IMAGE_ACTION, repository.as_str(), true),
    ] {
        let action = super::authorization::action_for(action);
        let request = iam::Request::new(&action, resource, &principals);
        let identity = iam::identity_decision_prepared(&prepared, &request);
        // A registry-level action is granted on the registry, which no repository policy
        // is the document for.
        let stated = match (repository_scoped, shared.as_deref()) {
            (true, Some(document)) => iam::resource_decision(document, &request),
            _ => Decision::NotStated,
        };
        let granted = iam::is_allowed(identity, stated);
        // A permissions boundary CAPS what the documents grant; it grants nothing itself,
        // and a call an explicit deny already refuses reads no ceiling.
        let refused_outright = identity == Decision::Denied || stated == Decision::Denied;
        let ceiling = if refused_outright {
            Decision::Allowed
        } else {
            capped.as_deref().map_or(Decision::Allowed, |boundary| {
                iam::identity_decision_prepared(boundary, &request)
            })
        };
        if granted && ceiling == Decision::Allowed {
            continue;
        }
        // Why the grant was withheld: the two documents' own clauses are this service's one
        // wording of them ([`super::authorization::withheld_because`]), and the ceiling's are
        // the shared evaluator's — so an explicit deny in the REPOSITORY's policy is reported
        // as that rather than as a missing identity grant.
        let reason = if ceiling == Decision::Denied {
            iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string()
        } else if !granted {
            super::authorization::withheld_because(identity, stated, &action)
        } else {
            iam::no_permissions_boundary_allows(&action)
        };
        // The refusal is the security-relevant event of this pull, so it is attributable in
        // the emulator's audit log the way this service's own gate logs a refused request:
        // who the pull was made as, what it asked for, and the outcome. The policy
        // documents themselves are not logged.
        ::tracing::info!(
            target: "emulator::audit",
            audit = true,
            operation = "ServeImagePull",
            outcome = "refused_access_denied",
            principal = %role_arn,
            action = %action,
            resource = %resource,
            explicit_deny = refused_outright || ceiling == Decision::Denied,
            "no policy in force permits this role to pull this image"
        );
        return Some(PullRefusal::NotAuthorized {
            message: iam::not_authorized_by_identity_policy(role_arn, &action, resource, &reason),
            action,
        });
    }
    None
}

/// Import the image `reference` names into the cache repository, when a pull through cache
/// rule of this registry covers that repository — answering the digest the import stored.
///
/// `None` when no rule covers the repository, so there is nothing to cache, and when the
/// registry is at a bound that a cached copy would push it past: an import stores a
/// repository, an image, a tag and the one blob that image references, so it is held to the
/// cap on each of them. EVERY bound is checked before ANYTHING is written, so a rejected
/// import leaves the store exactly as it found it — a client pulling tag after tag under a
/// cache rule is bounded by those caps rather than growing this process without end.
fn import_through_cache_rule(
    store: &mut ComAmazonawsEcrAmazonec2containerregistryV20150921State,
    reference: &RegistryReference,
    now: SystemTime,
) -> Option<String> {
    let rule = covering_cache_rule(store, &reference.repository_name)?;
    // A pull addressing a DIGEST names content, and an import has none to match: the
    // upstream manifest is not here to be digested, so only a tag is cached.
    let tag = reference.tag.clone()?;
    let upstream = upstream_reference(&rule, &reference.repository_name, &tag);
    match store.repositories.get(&reference.repository_name) {
        // A cached copy the registry has no room for is not cached at all.
        None if store.repositories.len() >= MAX_REPOSITORIES => return None,
        Some(repository)
            if repository.images.len() >= MAX_IMAGES_PER_REPOSITORY
                || repository.tag_index.len() >= MAX_TAGS_PER_REPOSITORY
                || repository.layer_blobs.len() >= MAX_IMPORTED_BLOBS_PER_REPOSITORY =>
        {
            return None
        }
        _ => {}
    }
    let (manifest, config) = imported_manifest(&upstream);
    let digest = manifest_digest(&manifest);
    let config_digest = content_digest(config.as_bytes());
    let repository = store
        .repositories
        .entry(reference.repository_name.clone())
        .or_insert_with(|| cache_repository(&reference.region, &reference.repository_name, now));
    // The configuration document the imported manifest describes is held as the blob it
    // describes, under the digest of its own bytes, so the manifest references nothing the
    // repository does not have.
    repository.layer_blobs.insert(
        config_digest.clone(),
        LayerBlob {
            digest: config_digest,
            size: i64::try_from(config.len()).ok(),
            media_type: Some(OCI_IMAGE_CONFIG_MEDIA_TYPE.to_string()),
            filesystem: LayerFilesystem::default(),
            content: config.into_bytes(),
        },
    );
    repository.images.entry(digest.clone()).or_insert_with(|| {
        ImageRecord::pushed(
            digest.clone(),
            manifest,
            Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE.to_string()),
            now,
        )
    });
    repository.move_tag(&tag, &digest);
    repository.refresh_image_sizes();
    Some(digest)
}

/// The pull through cache rule that covers `repository_name`: the one whose
/// `ecrRepositoryPrefix` is a leading NAMESPACE of the repository name — "the repository
/// name prefix to use when caching images from the source registry", with "always an
/// assumed `/` applied to the end of the prefix" — and the LONGEST such prefix when
/// several of them do.
fn covering_cache_rule(
    store: &ComAmazonawsEcrAmazonec2containerregistryV20150921State,
    repository_name: &str,
) -> Option<PullThroughCacheRule> {
    store
        .pull_through_cache_rules
        .iter()
        .filter(|(prefix, _)| {
            repository_name
                .strip_prefix(prefix.as_str())
                .is_some_and(|rest| rest.starts_with('/') && rest.len() > 1)
        })
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, rule)| rule.clone())
}

/// The UPSTREAM reference the cached image came from: the upstream registry's URL, the
/// upstream repository prefix the rule matches with ("when this field isn't specified,
/// Amazon ECR will use the `ROOT`", which prepends nothing), the repository path under the
/// rule's own prefix, and the tag that was asked for.
fn upstream_reference(rule: &PullThroughCacheRule, repository_name: &str, tag: &str) -> String {
    let prefix = rule
        .ecr_repository_prefix
        .as_deref()
        .map(normalize_prefix)
        .unwrap_or_default();
    let path = repository_name
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(repository_name);
    let upstream_prefix = rule
        .upstream_repository_prefix
        .as_deref()
        .filter(|value| *value != DEFAULT_UPSTREAM_REPOSITORY_PREFIX)
        .map(|value| format!("{}/", normalize_prefix(value)))
        .unwrap_or_default();
    let host = rule
        .upstream_registry_url
        .as_deref()
        .unwrap_or_default()
        .trim_end_matches('/');
    format!("{host}/{upstream_prefix}{path}:{tag}")
}

/// The repository a first pull through the cache creates, with the settings a repository
/// this registry creates without a request carries: tags mutable, nothing scanned on push,
/// and the service-default encryption.
fn cache_repository(region: &str, name: &str, now: SystemTime) -> Repo {
    Repo {
        arn: repository_arn(region, name),
        registry_id: registry_id().to_string(),
        name: name.to_string(),
        created_at: now,
        image_tag_mutability: DEFAULT_IMAGE_TAG_MUTABILITY.to_string(),
        image_tag_mutability_exclusion_filters: Vec::new(),
        image_scanning_config: ImageScanningConfiguration::default(),
        encryption_config: EncryptionConfiguration {
            encryption_type: DEFAULT_ENCRYPTION_TYPE.to_string(),
            kms_key: None,
        },
        tags: Vec::new(),
        policy_text: None,
        images: BTreeMap::new(),
        tag_index: BTreeMap::new(),
        layer_blobs: BTreeMap::new(),
        lifecycle_policy: None,
        lifecycle_preview: None,
    }
}

/// The manifest an import stores for the cached image, with the configuration document it
/// describes: `(manifest, configuration)`.
///
/// An OCI image manifest describing ONE configuration blob and no layer, because no layer
/// of the upstream image is here: this emulator makes no outbound connection, so what the
/// registry holds for a cached image is its own record of what was cached — the upstream
/// reference it came from, in the configuration document's annotation — and every digest
/// and size in the manifest is computed over the bytes beside it.
fn imported_manifest(upstream: &str) -> (String, String) {
    let config = serde_json::json!({
        "mediaType": OCI_IMAGE_CONFIG_MEDIA_TYPE,
        "annotations": { "org.opencontainers.image.ref.name": upstream },
    })
    .to_string();
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_IMAGE_MANIFEST_MEDIA_TYPE,
        "config": {
            "mediaType": OCI_IMAGE_CONFIG_MEDIA_TYPE,
            "digest": content_digest(config.as_bytes()),
            "size": config.len(),
        },
        "layers": [],
    })
    .to_string();
    (manifest, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reference to a repository of this registry in `us-east-1`, in the form the Amazon
    /// ECR User Guide gives for a pull — the form a task definition carries.
    fn reference(path: &str) -> String {
        format!(
            "{}.dkr.ecr.us-east-1.amazonaws.com/{path}",
            registry_id()
        )
    }

    /// What a row expects [`parse_reference`] to read out of its reference.
    #[derive(Debug, PartialEq, Eq)]
    struct Named {
        repository: &'static str,
        tag: Option<&'static str>,
        digest: Option<&'static str>,
    }

    /// One row of the reference table: the case, the reference, and what it names — `None`
    /// for a reference that names no repository of THIS registry.
    struct Row {
        case: &'static str,
        image: String,
        names: Option<Named>,
    }

    #[test]
    fn a_reference_of_this_registry_names_its_region_repository_and_tag() {
        let tagged = |repository, tag| Named {
            repository,
            tag: Some(tag),
            digest: None,
        };
        let rows = vec![
            Row {
                case: "a tagged reference",
                image: reference("app:1"),
                names: Some(tagged("app", "1")),
            },
            Row {
                case: "a reference with no tag takes the default one",
                image: reference("app"),
                names: Some(tagged("app", DEFAULT_IMAGE_TAG)),
            },
            Row {
                case: "a namespaced repository",
                image: reference("docker-hub/library/busybox:latest"),
                names: Some(tagged("docker-hub/library/busybox", "latest")),
            },
            Row {
                case: "a reference addressing a digest",
                image: reference("app@sha256:abc"),
                names: Some(Named {
                    repository: "app",
                    tag: None,
                    digest: Some("sha256:abc"),
                }),
            },
            Row {
                case: "another account's registry",
                image: "123456789012.dkr.ecr.us-east-1.amazonaws.com/app:1".to_string(),
                names: None,
            },
            Row {
                case: "a public registry",
                image: "public.ecr.aws/nginx/nginx:latest".to_string(),
                names: None,
            },
            Row {
                case: "a bare name",
                image: "nginx:1.27".to_string(),
                names: None,
            },
        ];
        for row in &rows {
            let case = row.case;
            let parsed = parse_reference(&row.image);
            match (&row.names, parsed.as_ref()) {
                (Some(expected), Some(parsed)) => {
                    assert_eq!(
                        parsed.repository_name, expected.repository,
                        "case {case:?}: expected repository {:?}, got {:?}",
                        expected.repository, parsed.repository_name
                    );
                    assert_eq!(
                        parsed.tag.as_deref(),
                        expected.tag,
                        "case {case:?}: expected tag {:?}, got {:?}",
                        expected.tag,
                        parsed.tag
                    );
                    assert_eq!(
                        parsed.digest.as_deref(),
                        expected.digest,
                        "case {case:?}: expected digest {:?}, got {:?}",
                        expected.digest,
                        parsed.digest
                    );
                    assert_eq!(
                        parsed.region, "us-east-1",
                        "case {case:?}: expected the host's Region, got {:?}",
                        parsed.region
                    );
                }
                (None, None) => {}
                (expected, parsed) => panic!(
                    "case {case:?}: expected {expected:?} to be named, got {parsed:?}"
                ),
            }
        }
    }

    #[test]
    fn an_imported_manifest_states_the_digest_and_size_of_the_blob_it_holds() {
        let (manifest, config) = imported_manifest("registry-1.docker.io/library/busybox:latest");
        let document: serde_json::Value =
            serde_json::from_str(&manifest).expect("the imported manifest is JSON");
        assert_eq!(
            document["config"]["digest"].as_str(),
            Some(content_digest(config.as_bytes()).as_str()),
            "case \"the configuration descriptor\": expected the digest of the stored bytes, \
             got {manifest}"
        );
        assert_eq!(
            document["config"]["size"].as_u64(),
            u64::try_from(config.len()).ok(),
            "case \"the configuration descriptor\": expected the length of the stored bytes, \
             got {manifest}"
        );
        assert!(
            config.contains("registry-1.docker.io/library/busybox:latest"),
            "case \"the configuration document\": expected it to name the upstream reference, \
             got {config}"
        );
    }

    #[test]
    fn the_upstream_reference_follows_the_rule_that_cached_the_image() {
        let rule = |upstream_prefix: Option<&str>| PullThroughCacheRule {
            ecr_repository_prefix: Some("hub".to_string()),
            upstream_registry_url: Some("registry-1.docker.io".to_string()),
            upstream_repository_prefix: upstream_prefix.map(ToString::to_string),
            ..PullThroughCacheRule::default()
        };
        // (case, the rule's upstreamRepositoryPrefix, the upstream reference)
        let rows: &[(&str, Option<&str>, &str)] = &[
            (
                "the documented default prepends nothing",
                Some(DEFAULT_UPSTREAM_REPOSITORY_PREFIX),
                "registry-1.docker.io/library/busybox:1",
            ),
            (
                "no prefix stated at all",
                None,
                "registry-1.docker.io/library/busybox:1",
            ),
            (
                "a stated prefix leads the upstream path",
                Some("mirror"),
                "registry-1.docker.io/mirror/library/busybox:1",
            ),
        ];
        for (case, upstream_prefix, expected) in rows {
            let actual = upstream_reference(&rule(*upstream_prefix), "hub/library/busybox", "1");
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn an_import_past_a_repository_cap_writes_nothing_at_all() {
        let repository_name = "hub/library/busybox";
        let mut store = ComAmazonawsEcrAmazonec2containerregistryV20150921State::default();
        store.pull_through_cache_rules.insert(
            "hub".to_string(),
            PullThroughCacheRule {
                ecr_repository_prefix: Some("hub".to_string()),
                upstream_registry_url: Some("registry-1.docker.io".to_string()),
                ..PullThroughCacheRule::default()
            },
        );
        let reference = |tag: &str| RegistryReference {
            region: "us-east-1".to_string(),
            repository_name: repository_name.to_string(),
            tag: Some(tag.to_string()),
            digest: None,
        };
        let now = SystemTime::UNIX_EPOCH;
        let case = "the first pull of a covered reference";
        assert!(
            import_through_cache_rule(&mut store, &reference("1"), now).is_some(),
            "case {case:?}: expected the upstream image to be cached"
        );

        // The repository at its blob cap: a further pull is the one this bound is about, a
        // client asking for tag after tag.
        let repository = store
            .repositories
            .get_mut(repository_name)
            .expect("the cached repository");
        for index in 0..MAX_IMPORTED_BLOBS_PER_REPOSITORY {
            let digest = format!("sha256:{index:064x}");
            repository.layer_blobs.insert(
                digest.clone(),
                LayerBlob {
                    digest,
                    size: None,
                    media_type: None,
                    filesystem: LayerFilesystem::default(),
                    content: Vec::new(),
                },
            );
        }
        let held = (
            repository.layer_blobs.len(),
            repository.images.len(),
            repository.tag_index.len(),
        );

        let case = "a pull past the blob cap";
        assert!(
            import_through_cache_rule(&mut store, &reference("2"), now).is_none(),
            "case {case:?}: expected no image to be cached"
        );
        let repository = store
            .repositories
            .get(repository_name)
            .expect("the cached repository");
        let after = (
            repository.layer_blobs.len(),
            repository.images.len(),
            repository.tag_index.len(),
        );
        assert_eq!(
            after, held,
            "case {case:?}: expected a refused import to write nothing, got {after:?} where \
             the repository held {held:?}"
        );
    }
}
