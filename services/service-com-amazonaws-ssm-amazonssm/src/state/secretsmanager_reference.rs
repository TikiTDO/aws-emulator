// SPDX-License-Identifier: CC0-1.0

//! The reserved hierarchy a read of Parameter Store serves an Amazon Web Services Secrets
//! Manager secret through: `/aws/reference/secretsmanager/<secret-id>`.
//!
//! That hierarchy is inside the one AWS publishes its own parameters in
//! ([`super::public_parameters::PUBLISHED_ROOT`]), so no caller can write a parameter there
//! — `PutParameter` refuses a name prefixed with `aws` — and nothing of it is ever in this
//! service's store. A name under it names a SECRET instead, and resolving it is a read of
//! Secrets Manager: applications reach a secret through Parameter Store precisely so they
//! need not call the Secrets Manager API themselves.
//!
//! So the answer is computed where the secret lives — Secrets Manager's own store, its own
//! authorization gate and its own envelope — rather than from anything this service holds:
//!
//! * WHETHER THE CALLER MAY READ IT is Secrets Manager's decision, taken by that service's
//!   own gate on its own action for the secret this read names
//!   (`secretsmanager:GetSecretValue`, which is the permission a caller needs to reach a
//!   secret through this path), so a caller its policies refuse is refused here too and a
//!   secret's resource policy grants this read exactly as it grants a direct one;
//! * WHICH VERSION is selected by the `name:selector` suffix the read carried, against the
//!   STAGING LABELS Secrets Manager keeps on that secret's versions — `AWSCURRENT` when the
//!   read named no selector, which is the version Secrets Manager serves a caller that names
//!   none. So the demoted version's own bytes are what a read selecting `AWSPREVIOUS` is
//!   answered with, and a label nothing carries resolves to nothing;
//! * THE VALUE is the one Secrets Manager answers with, opened under the secret's own KMS
//!   key by that service's envelope ([`secrets::envelope::open`]) and as THIS request's
//!   caller, so the key's policy decides the read and no plaintext is reported that the
//!   caller could not have read through Secrets Manager itself.
//!
//! `WithDecryption` is what asks for those bytes: this service holds no ciphertext of its
//! own for a reference, so a read that asks for no decryption is served the reference's
//! identity without its value, and nothing of the source's answer either.
//!
//! What the read reports is therefore a parameter of the secret's identity rather than of
//! any parameter record: `SecureString`, the reference path as the request wrote it, the
//! SECRET's ARN, the instant the version it answers with was created, and the source's own
//! answer in `SourceResult` — "Applies to parameters that reference information in other
//! Amazon Web Services services. `SourceResult` is the raw result or response from the
//! source" (`Parameter$SourceResult`). Its `Version` is the member's own default, because a
//! reference has no parameter version of its own: version numbers belong to the parameters
//! `PutParameter` wrote.
//!
//! Nothing here writes: the resolution reads Secrets Manager's store under a short-lived
//! guard and the opening of the value reaches no state of this service at all, so a read of
//! a reference changes nothing a later caller — of either service — sees.

use std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_secretsmanager_secretsmanager::state as secrets;

use super::{canonical_name, reported_selector, DEFAULT_DATA_TYPE, TYPE_SECURE_STRING};
use crate::ops::types::Parameter;

/// The reserved hierarchy itself, canonical (no leading `/`), as a parameter name spells
/// it: `/aws/reference/secretsmanager`.
pub const REFERENCE_PATH: &str = "aws/reference/secretsmanager";

/// The Secrets Manager action a caller needs for a read through this path, which is the
/// action this resolution asks that service's own gate about.
const SOURCE_ACTION: &str = "GetSecretValue";

/// The `SecretId` the reserved path `name` references, or `None` when `name` is not a read
/// of that path. Takes either spelling of the name (with or without its leading `/`).
///
/// The hierarchy level BELOW the reserved path is the whole of the secret's id, separators
/// and all, because a secret's name may itself carry `/`. The reserved path on its own
/// references no secret: it is a hierarchy, not a name.
pub fn referenced_secret(name: &str) -> Option<&str> {
    let secret_id = canonical_name(name)
        .strip_prefix(REFERENCE_PATH)?
        .strip_prefix('/')?;
    (!secret_id.is_empty()).then_some(secret_id)
}

/// What a read through the reserved path is refused with when the service it depends on
/// would not serve the value: the ONE sentence Parameter Store words for a downstream call
/// that failed, whichever way it failed, because the failure belongs to the other service
/// and this one states nothing of it.
pub const DEPENDENCY_FAILURE_MESSAGE: &str =
    "An error occurred while calling one AWS dependency service.";

/// Why a read of the reserved path cannot report a value, named by what reports it.
#[derive(Debug)]
pub enum Refusal {
    /// There is nothing to report: this Region holds no such secret, it is scheduled for
    /// deletion — "If a secret is scheduled for deletion, then its details, including the
    /// encrypted secret value, is not accessible" — or the selector names no version of it.
    /// A read of a name nothing resolves is a miss.
    NotFound,
    /// The service this read depends on would not serve the value: it refused the caller the
    /// read, or the value could not be opened under the secret's own key. Both are a
    /// downstream call that failed, which is the one class [`DEPENDENCY_FAILURE_MESSAGE`]
    /// reports — never the other service's own refusal, which this service does not relay.
    Dependency,
    /// No Secrets Manager answered at all, so nothing resolved the reference: a server-side
    /// fault of how this emulator was assembled, never anything about the request.
    Unavailable(String),
}

/// The secret version a read of the reserved path resolved, carried out of Secrets
/// Manager's store guard so the KMS call that opens its value is made with no lock held.
struct Resolved {
    arn: String,
    name: String,
    version_id: String,
    stages: Option<Vec<String>>,
    created_date: SystemTime,
    /// The ARN the value was sealed for and the Region it was sealed in: a replica serves
    /// the primary's values, which open under the primary's ARN and against the key in the
    /// primary's Region.
    context_arn: String,
    sealed_region: String,
    value: secrets::envelope::StoredValue,
}

/// The `Parameter` a read of `path` — the reserved-path name as the request wrote it,
/// selector stripped — reports for the secret `secret_id` names in `region`.
///
/// `selector` is the `name:selector` suffix the read carried: a Secrets Manager staging
/// label, or the version id Secrets Manager identifies a version by, which is the same pair
/// `GetSecretValue` selects a version with. `None` selects the version that service serves
/// a caller naming neither ([`secrets::STAGE_CURRENT`]).
///
/// `with_decryption` is what asks for the secret's BYTES. Parameter Store holds no
/// ciphertext of its own for a reference — the value belongs to Secrets Manager, which
/// answers it only decrypted — so a read that asks for no decryption is served the
/// reference's identity without its value and the source is never asked to open one. The
/// secret is still resolved, and still only for a caller Secrets Manager permits the read,
/// so a reference reports nothing about a secret its caller may not read.
///
/// Reaches no state of THIS service, so a handler calls it with its own store guard
/// released.
pub async fn resolve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    path: &str,
    secret_id: &str,
    selector: Option<&str>,
    with_decryption: bool,
) -> Result<Parameter, Refusal> {
    // Whether this caller may read the secret is Secrets Manager's decision, taken on its
    // own action and against the policies in force on that secret — before anything about
    // the secret is read, so a caller no policy allows is told nothing about it.
    secrets::authorization::authorize_secret(engine, ctx, SOURCE_ACTION, Some(secret_id)).map_err(
        |denied| {
            // The caller is told which class of failure refused the read, not the other
            // service's sentence about its own policies; the sentence it did word is kept
            // for whoever reads this emulator's own trace.
            ::tracing::debug!(
                refusal = %denied.message,
                "Secrets Manager refused the read a Parameter Store reference depends on"
            );
            Refusal::Dependency
        },
    )?;

    let Some(state) = secrets::service_state(engine) else {
        return Err(Refusal::Unavailable(
            "no Secrets Manager service is mounted beside Systems Manager, so a secret \
             reference cannot be resolved"
                .to_string(),
        ));
    };
    let resolved = {
        let store = state.lock();
        let view = store.view(region, secret_id).ok_or(Refusal::NotFound)?;
        if view.secret.scheduled_for_deletion() {
            return Err(Refusal::NotFound);
        }
        // The versions are the CONTENT of the secret, which for a replica is the primary's.
        let content = view.content;
        let version = match selector {
            None => content.version_with_stage(secrets::STAGE_CURRENT),
            // A staging label is what a selector names; the same suffix naming a version
            // id selects that version outright, the way Secrets Manager resolves the pair.
            Some(selector) => content
                .version_with_stage(selector)
                .or_else(|| content.version(selector)),
        }
        .ok_or(Refusal::NotFound)?;
        Resolved {
            arn: view.secret.arn.clone(),
            name: view.secret.name.clone(),
            version_id: version.version_id.clone(),
            stages: version.stages_out(),
            created_date: version.created_date,
            context_arn: content.value_context_arn().to_string(),
            sealed_region: secrets::arn_region(content.value_context_arn())
                .unwrap_or(region)
                .to_string(),
            value: version.value.clone(),
        }
    };

    // A read that asks for no decryption is served the reference's identity and no value:
    // there is no form of it this service could report instead, because the value is the
    // source's and that service answers it only decrypted. So nothing is opened for such a
    // read, and no key is asked about it.
    if !with_decryption {
        return Ok(reported(&resolved, path, selector, None, None));
    }

    // The value is held under the secret's own key, so answering with it means opening it
    // under that key as this caller: a caller the key does not permit is refused rather
    // than answered with the plaintext.
    let context =
        secrets::envelope::ValueContext::new(&resolved.context_arn, &resolved.version_id);
    let opened = secrets::envelope::open(
        engine,
        ctx,
        &resolved.sealed_region,
        &context,
        &resolved.value,
    )
    .await
    .map_err(|refusal| {
        // However the open failed — a key that refused this caller, a value it could not
        // open — it is the downstream call this read depends on failing.
        ::tracing::debug!(
            refusal = %refusal.decryption_failure_message(),
            "the value a Parameter Store reference depends on could not be opened"
        );
        Refusal::Dependency
    })?;
    let (secret_string, secret_binary) = opened.into_members();

    Ok(reported(&resolved, path, selector, secret_string, secret_binary))
}

/// The `Parameter` the resolved version is reported as.
///
/// The parameter's identity is the SECRET's — its ARN, and the instant the version this read
/// answers with was created ("Date the parameter was last changed or updated and the
/// parameter version was created", `Parameter$LastModifiedDate`) — and its name is the
/// reserved path the read asked for, reported as the request wrote it. The type is
/// `SecureString`, because the value is one a key of this account protects, and the data
/// type is the default `text`.
///
/// A read the value was not opened for carries neither the value nor the source's answer:
/// that answer IS the value, so reporting it would report what the read did not ask to have
/// decrypted.
fn reported(
    resolved: &Resolved,
    path: &str,
    selector: Option<&str>,
    secret_string: Option<String>,
    secret_binary: Option<Vec<u8>>,
) -> Parameter {
    // "The decrypted secret value, if the secret value was originally provided as a string"
    // is the value a reference reports; a secret holding binary data instead has no string
    // value, and binary is reported the way this wire carries it — "When you retrieve a
    // SecretBinary using the HTTP API ... the value is Base64-encoded"
    // (`GetSecretValueResponse$SecretBinary`, Secrets Manager's own model).
    let value = secret_string.or_else(|| {
        secret_binary
            .as_deref()
            .map(::aws_smithy_types::base64::encode)
    });
    Parameter {
        name: Some(path.to_string()),
        r#type: Some(TYPE_SECURE_STRING.to_string()),
        value: value.clone(),
        // A reference carries no parameter version of its own: version numbers belong to
        // the parameters a put wrote, so this is the member's own modelled default
        // (`smithy_query(query="shape", shape="Parameter")`: `"Version": { "default": 0 }`).
        version: 0,
        // "Either the version number or the label used to retrieve the parameter value"
        // (`Parameter$Selector`): for a reference that is the suffix the read carried, colon
        // and all, as the service echoes it back.
        selector: selector.map(reported_selector),
        source_result: value
            .as_ref()
            .map(|_| source_result(resolved, value.clone(), secret_binary)),
        last_modified_date: Some(::aws_smithy_types::DateTime::from(resolved.created_date)),
        arn: Some(resolved.arn.clone()),
        data_type: Some(DEFAULT_DATA_TYPE.to_string()),
    }
}

/// The source's own answer, rendered as the source itself renders it.
///
/// The members are the ones `GetSecretValue` reports — the ARN, the secret's name, the
/// version id, the value under whichever member holds it, the version's staging labels and
/// the instant it was created, as epoch seconds — under the names the SOURCE spells them
/// with in this result, which are lower camel case: a client reads the resolved version of a
/// reference out of `versionId`.
fn source_result(
    resolved: &Resolved,
    secret_string: Option<String>,
    secret_binary: Option<Vec<u8>>,
) -> String {
    let mut answer = ::serde_json::Map::new();
    answer.insert("arn".to_string(), resolved.arn.clone().into());
    answer.insert("name".to_string(), resolved.name.clone().into());
    answer.insert("versionId".to_string(), resolved.version_id.clone().into());
    if let Some(binary) = secret_binary.as_deref() {
        answer.insert(
            "secretBinary".to_string(),
            ::aws_smithy_types::base64::encode(binary).into(),
        );
    } else if let Some(string) = secret_string {
        answer.insert("secretString".to_string(), string.into());
    }
    if let Some(stages) = &resolved.stages {
        answer.insert("versionStages".to_string(), stages.clone().into());
    }
    answer.insert(
        "createdDate".to_string(),
        epoch_seconds(resolved.created_date).into(),
    );
    ::serde_json::Value::Object(answer).to_string()
}

/// `at` as the seconds since the epoch an `awsJson1_1` timestamp carries, at the
/// millisecond resolution that protocol renders.
fn epoch_seconds(at: SystemTime) -> f64 {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis())
        .unwrap_or_default();
    millis as f64 / 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the reserved path lies inside the hierarchy AWS publishes its own parameters
    /// in, which is what keeps any caller from writing a parameter over it.
    #[test]
    fn reserved_path_is_inside_the_published_hierarchy() {
        assert!(
            crate::state::public_parameters::is_published_name(REFERENCE_PATH),
            "the reserved path {REFERENCE_PATH} must lie in AWS's own hierarchy"
        );
    }

    /// RULE: the level below the reserved path is the whole of the secret's id, whatever
    /// shape it has; the path itself, and every name outside it, references none.
    #[test]
    fn referenced_secret_is_the_level_below_the_reserved_path() {
        let cases: &[(&str, Option<&str>)] = &[
            ("/aws/reference/secretsmanager/demo", Some("demo")),
            ("aws/reference/secretsmanager/demo", Some("demo")),
            (
                "/aws/reference/secretsmanager/team/demo",
                Some("team/demo"),
            ),
            ("/aws/reference/secretsmanager/", None),
            ("/aws/reference/secretsmanager", None),
            ("/aws/reference/secretsmanagerish/demo", None),
            ("/aws/service/ami-amazon-linux-latest/x", None),
            ("/demo", None),
        ];
        for (name, expected) in cases {
            let actual = referenced_secret(name);
            assert_eq!(
                actual, *expected,
                "case {name:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
