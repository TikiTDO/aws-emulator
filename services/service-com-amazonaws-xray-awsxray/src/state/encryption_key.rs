// SPDX-License-Identifier: CC0-1.0

//! Resolving the KMS key a `PutEncryptionConfig` selector names, through KMS.
//!
//! `EncryptionConfig$KeyId` is "the ID of the KMS key used for encryption"
//! (`smithy_query(query="shape",shape="EncryptionConfig")`), while
//! `PutEncryptionConfig$KeyId` admits three selectors for it — an alias, a key
//! id, or the full ARN of either. Those are not the same thing: an alias points
//! at WHATEVER key it currently targets, so a configuration that reported the
//! alias back would not say which key protects the account's trace data. The
//! configuration therefore reports the KEY the selector resolves to, which is a
//! fact only KMS holds.
//!
//! So the selector is RESOLVED — through the KMS endpoint of this same emulator,
//! the way every emulated service reaches another, over the in-process transport
//! the operation callback hands out
//! ([`smithy_server_runtime::OperationContext::callback`]). No socket is opened,
//! no name is resolved and nothing outside this process is reachable: the
//! endpoint URL below is ADDRESSING for that transport, which routes the call
//! back into this emulator's own KMS service. Its only variable part is the
//! Region the request was signed for — a KMS key is regional, so the key a
//! selector names is the one in that Region — and no caller-supplied value
//! becomes a destination. The caller's `Authorization` header is never
//! forwarded; the credentials are the emulator's own.
//!
//! `DescribeKey` is the read that resolves a key selector and nothing else: it
//! "provides detailed information about a KMS key" and is the call AWS documents
//! for a predefined Amazon Web Services alias, where it is also what ASSOCIATES
//! that alias with the account's Amazon Web Services managed key. A selector KMS
//! does not resolve, and an emulator assembled WITHOUT a KMS, answer nothing
//! about the key, so the configuration keeps the qualified form of the selector
//! the request submitted ([`super::kms_key_arn`]) rather than reporting a key
//! nothing stands behind.

/// The ARN of the KMS key `key_id` selects, as KMS reports it at `region`'s
/// endpoint, or `None` when no KMS answered for it.
///
/// The ARN is KMS's own `KeyMetadata$Arn` — the `key/<key id>` form, whichever
/// selector was submitted — so a caller comparing the configuration against
/// `kms:DescribeKey`, a key policy or a key's audit records is comparing the
/// same identifier.
pub async fn resolved_key_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
) -> Option<String> {
    match kms_client(ctx, region)
        .describe_key()
        .key_id(key_id)
        .send()
        .await
    {
        Ok(described) => described.key_metadata?.arn,
        Err(error) => {
            // A key KMS does not hold, a refusal of the selector, a KMS this
            // emulator was assembled without: none of them says what key the
            // selector names, which is what the caller is left with.
            ::tracing::debug!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "KMS did not resolve the key the encryption configuration names"
            );
            None
        }
    }
}

/// A KMS client over the in-process callback into this same emulator, addressed
/// to `region`'s endpoint.
///
/// The transport is in-process, so there is nothing to retry, time out or watch
/// for a stalled stream — each would need a sleep implementation this client is
/// not given — and the credentials the callback hands over are used directly
/// rather than cached.
fn kms_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}
