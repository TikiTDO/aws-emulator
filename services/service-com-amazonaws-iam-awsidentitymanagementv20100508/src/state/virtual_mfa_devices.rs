// SPDX-License-Identifier: CC0-1.0

//! The virtual MFA devices the account holds.
//!
//! One concern, held in one place: the record the account keeps of a device
//! ([`VirtualMfaDeviceRecord`]), the serial number that identifies it
//! ([`virtual_mfa_device_arn`]), the seed and the QR code a create mints for it
//! ([`mint_virtual_mfa_device_seed`], [`virtual_mfa_device_qr_code`]), the one-time pass codes
//! a device provisioned with that seed emits ([`virtual_mfa_device_answers`],
//! [`virtual_mfa_device_answers_in_sequence`]) and the report every reader of a device makes
//! ([`ComAmazonawsIamAwsidentitymanagementv20100508State::virtual_mfa_device_report`]).

#[allow(unused_imports)]
use super::*;

use crate::ops::types::VirtualMFADevice;

/// The resource type a virtual MFA device's ARN carries, between the account segment and
/// the device's path and name: `arn:aws:iam::<account>:mfa<path><name>`.
///
/// `VirtualMFADevice$SerialNumber` is "the serial number associated with
/// `VirtualMFADevice`", and for a virtual device that serial IS this ARN — the value
/// `EnableMFADevice` and `DeleteVirtualMFADevice` address the device by.
const VIRTUAL_MFA_DEVICE_RESOURCE: &str = "mfa";

/// The `AssignmentStatus` value that selects BOTH kinds of device: "if you do not specify
/// an `AssignmentStatus`, the operation defaults to `Any`, which lists both assigned and
/// unassigned virtual MFA devices" (`ListVirtualMFADevices$AssignmentStatus`). One of the
/// three values the enum declares, which `ops::types::ASSIGNMENT_STATUS_TYPE_VALUES`
/// records.
pub const ASSIGNMENT_STATUS_ANY: &str = "Any";

/// The `AssignmentStatus` value that selects the devices assigned to a user: "the status
/// (`Unassigned` or `Assigned`) of the devices to list"
/// (`ListVirtualMFADevices$AssignmentStatus`). See [`ASSIGNMENT_STATUS_ANY`].
pub const ASSIGNMENT_STATUS_ASSIGNED: &str = "Assigned";

/// The `AssignmentStatus` value that selects the devices no user holds. See
/// [`ASSIGNMENT_STATUS_ANY`].
pub const ASSIGNMENT_STATUS_UNASSIGNED: &str = "Unassigned";

/// The marker scope of a page over the account's virtual MFA devices
/// ([`MARKER_SCOPE_USERS`] states why each listing has its own scope).
pub const MARKER_SCOPE_VIRTUAL_MFA_DEVICES: &str = "virtual-mfa-devices";

/// How many virtual MFA devices one account may hold.
///
/// AWS publishes no number for this, so the bound is the emulator's own: it is what stops
/// the device map growing without end on repeated creates, and it sits far above any set
/// of devices an account under emulation uses. A create that would carry the account past
/// it is refused with the modelled `LimitExceededException` — "it attempted to create
/// resources beyond the current Amazon Web Services account limits" — and stores nothing.
pub const MAX_VIRTUAL_MFA_DEVICES_PER_ACCOUNT: usize = 1_000;

/// How many bytes of seed material a minted `Base32StringSeed` carries: 20 bytes taken from
/// the credential store's own minted secret, spelled as 32 base32 characters
/// ([`BASE32_ALPHABET`] carries 5 bits per character) — the shared-secret LENGTH RFC 4226
/// (HOTP, which the TOTP of `otpauth://totp/` extends) recommends for a key, "it is
/// recommended that they be 160 bits".
const VIRTUAL_MFA_DEVICE_SEED_BYTES: usize = 20;

/// The quiet zone a rendered QR code carries, in modules: the QR Code specification
/// requires a light margin of four modules on every side, without which a scanner cannot
/// find the symbol.
const QR_CODE_QUIET_ZONE_MODULES: usize = 4;

/// How many pixels wide one module of a rendered QR code is, so the PNG a client displays
/// is large enough to scan rather than a one-pixel-per-module thumbnail.
const QR_CODE_MODULE_PIXELS: usize = 4;

/// The 8-bit grayscale sample of a DARK module of a rendered QR code.
const QR_CODE_DARK_SAMPLE: u8 = 0x00;

/// The 8-bit grayscale sample of a LIGHT module, and of the quiet zone around the symbol.
const QR_CODE_LIGHT_SAMPLE: u8 = 0xFF;

/// One virtual MFA device the account holds, keyed in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::virtual_mfa_devices`] by its
/// serial number.
///
/// The record keeps what the device IS — its name, the path it was created under, the
/// seed it was provisioned with, the user it is enabled for and the tags attached to it —
/// and not the QR code a create answered with, which is a rendering of the seed
/// ([`virtual_mfa_device_qr_code`]) rather than a second fact about the device.
///
/// `Debug` is written out rather than derived, so no formatter of the account's state can
/// print the seed ([`Self::base32_string_seed`]); it shows which device the record is and
/// that a seed is held, the same shape [`PasswordVerifier`] takes for the same reason.
#[derive(Clone)]
pub struct VirtualMfaDeviceRecord {
    /// "The name of the virtual MFA device" (`CreateVirtualMFADevice$VirtualMFADeviceName`)
    /// in the case the creator chose, which is also the last segment of the serial number
    /// the device is held under.
    pub device_name: String,
    /// The path the device was created under — "use with path to uniquely identify a
    /// virtual MFA device" — held RESOLVED: a create that names no path creates the device
    /// at [`DEFAULT_PATH`].
    pub path: String,
    /// The base32 seed the device was provisioned with, "defined as specified in RFC3548"
    /// (`VirtualMFADevice$Base32StringSeed`).
    ///
    /// Minted once, by the create ([`mint_virtual_mfa_device_seed`]), and reported only by
    /// that response: "the seed information contained in the QR code and the Base32 string
    /// should be treated like any other secret access information"
    /// (`CreateVirtualMFADevice`), so no later read of the device carries it.
    pub base32_string_seed: String,
    /// [`name_key`] of the IAM user the device is enabled for, or `None` while no user
    /// holds it — the distinction `ListVirtualMFADevices`'s `AssignmentStatus` filters on.
    ///
    /// The user is held by KEY rather than by value, so the device reports the user as the
    /// account holds it now: a later change to that user is part of what the next read of
    /// the device reports.
    pub user_key: Option<String>,
    /// "The date and time on which the virtual MFA device was enabled"
    /// (`VirtualMFADevice$EnableDate`), or `None` while no user holds it.
    pub enable_date: Option<::aws_smithy_types::DateTime>,
    /// "A list of tags that are attached to the virtual MFA device"
    /// (`VirtualMFADevice$Tags`), as the create attached them, or `None` when it attached
    /// none. The tag READS of this device are what sort them by key; the record holds the
    /// pairs rather than an order.
    pub tags: Option<Vec<Tag>>,
}

/// What a device's record shows when the account's state is formatted: which device it
/// is, whether a user holds it, and that a seed is held — never the seed itself, which
/// "should be treated like any other secret access information"
/// (`CreateVirtualMFADevice`).
impl ::std::fmt::Debug for VirtualMfaDeviceRecord {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("VirtualMfaDeviceRecord")
            .field("device_name", &self.device_name)
            .field("path", &self.path)
            .field("user_key", &self.user_key)
            .field("enable_date", &self.enable_date)
            .field("tags", &self.tags)
            .finish_non_exhaustive()
    }
}

/// The ARN that is a virtual MFA device's serial number:
/// `arn:aws:iam::<account>:mfa<path><name>`.
///
/// Built the same way [`server_certificate_arn`] is, from the `mfa` resource type
/// ([`VIRTUAL_MFA_DEVICE_RESOURCE`]): `path` already begins and ends with `/` (the model's
/// `pathType` pattern), and IAM is a global service, so the region segment is empty.
///
/// The path and the name TOGETHER identify the device — "use with path to uniquely
/// identify a virtual MFA device" — which is why this value, and not the name alone, is
/// the key the account holds a device under. One pair maps to exactly one serial: a name
/// cannot carry the `/` that separates path segments (`virtualMFADeviceName` is
/// `^[\w+=,.@-]+$`) and a path both begins and ends with one (`pathType` is
/// `^(\u002F)|(\u002F[\u0021-\u007E]+\u002F)$`), both refused by the constraint walk
/// before a handler sees them.
pub fn virtual_mfa_device_arn(path: &str, virtual_mfa_device_name: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("{VIRTUAL_MFA_DEVICE_RESOURCE}{path}{virtual_mfa_device_name}"),
    )
}

/// A fresh `Base32StringSeed` for a virtual MFA device: seed material spelled in base32, as
/// "the base32 seed defined as specified in RFC3548"
/// (`VirtualMFADevice$Base32StringSeed`), [`VIRTUAL_MFA_DEVICE_SEED_BYTES`] bytes of it.
///
/// Nothing in a service may construct a secret itself, so the material is minted by the
/// one component that generates them, the emulator's credential store, and the record is
/// then removed from it again — the shape [`mint_service_specific_credential`] uses for
/// the same reason: a virtual MFA device's seed authenticates its holder to IAM through a
/// time-based one-time password, never to this emulator over SigV4, so the pair must not
/// stay in the credential store.
///
/// `None` when the store minted no material, which leaves the caller to refuse rather than
/// report a seed it made up.
pub fn mint_virtual_mfa_device_seed(
    store: &::emulator_server::CredentialStore,
) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_ACCESS_KEY);
    let secret = minted
        .secret(::emulator_server::aws_credentials::SECRET_ACCESS_KEY)
        .map(str::to_string);
    // The pair is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    let material: Vec<u8> = secret?
        .bytes()
        .take(VIRTUAL_MFA_DEVICE_SEED_BYTES)
        .collect();
    (material.len() == VIRTUAL_MFA_DEVICE_SEED_BYTES)
        .then(|| ::data_encoding::BASE32_NOPAD.encode(&material))
}

/// The `otpauth://` URI a virtual MFA device's QR code encodes:
/// `otpauth://totp/$virtualMFADeviceName@$AccountName?secret=$Base32String`, as
/// `VirtualMFADevice$QRCodePNG` states it — "`AccountName` is the user name if set
/// (otherwise, the account ID otherwise), and `Base32String` is the seed in base32
/// format".
///
/// Stated here, beside the rendering, so the image a client scans and the seed the create
/// reports carry the same secret.
///
/// Each part occupies its own slot by construction: the device name admits none of the
/// characters that delimit a URI (`virtualMFADeviceName` is `^[\w+=,.@-]+$`, refused by
/// the constraint walk before a handler sees it), the account name is the emulator's own
/// account id or an IAM user name (`^[\w+=,.@-]+$` likewise), and the seed is base32.
pub fn virtual_mfa_device_otpauth_uri(
    virtual_mfa_device_name: &str,
    account_name: &str,
    base32_string_seed: &str,
) -> String {
    format!("otpauth://totp/{virtual_mfa_device_name}@{account_name}?secret={base32_string_seed}")
}

/// `VirtualMFADevice$QRCodePNG` for a device provisioned with `base32_string_seed`: "a QR
/// code PNG image that encodes
/// `otpauth://totp/$virtualMFADeviceName@$AccountName?secret=$Base32String`".
///
/// A real QR code of that URI ([`virtual_mfa_device_otpauth_uri`]), encoded by the
/// `qrcode` crate and written as an 8-bit grayscale PNG by the `png` crate, so an
/// authenticator app that scans the image provisions exactly the device this account
/// holds. The symbol carries the quiet zone its specification requires
/// ([`QR_CODE_QUIET_ZONE_MODULES`]) and is scaled to
/// [`QR_CODE_MODULE_PIXELS`] pixels per module.
///
/// `None` when the URI does not fit in a QR symbol at all, which leaves the caller to
/// refuse rather than report an image that encodes nothing.
pub fn virtual_mfa_device_qr_code(
    virtual_mfa_device_name: &str,
    account_name: &str,
    base32_string_seed: &str,
) -> Option<Vec<u8>> {
    let uri =
        virtual_mfa_device_otpauth_uri(virtual_mfa_device_name, account_name, base32_string_seed);
    let code = ::qrcode::QrCode::new(uri.as_bytes()).ok()?;
    let modules = code.to_colors();
    let symbol_modules = code.width();
    let side_modules = symbol_modules + 2 * QR_CODE_QUIET_ZONE_MODULES;
    let side_pixels = side_modules * QR_CODE_MODULE_PIXELS;
    let mut samples = vec![QR_CODE_LIGHT_SAMPLE; side_pixels * side_pixels];
    for (position, module) in modules.iter().enumerate() {
        if *module != ::qrcode::Color::Dark {
            continue;
        }
        let row = position / symbol_modules;
        let column = position % symbol_modules;
        for pixel_row in 0..QR_CODE_MODULE_PIXELS {
            for pixel_column in 0..QR_CODE_MODULE_PIXELS {
                let y = (row + QR_CODE_QUIET_ZONE_MODULES) * QR_CODE_MODULE_PIXELS + pixel_row;
                let x =
                    (column + QR_CODE_QUIET_ZONE_MODULES) * QR_CODE_MODULE_PIXELS + pixel_column;
                samples[y * side_pixels + x] = QR_CODE_DARK_SAMPLE;
            }
        }
    }
    let mut rendered = Vec::new();
    {
        let side = u32::try_from(side_pixels).ok()?;
        let mut encoder = ::png::Encoder::new(&mut rendered, side, side);
        encoder.set_color(::png::ColorType::Grayscale);
        encoder.set_depth(::png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&samples).ok()?;
        writer.finish().ok()?;
    }
    Some(rendered)
}

/// How long one time step of a virtual MFA device's one-time pass code lasts.
///
/// The device the `otpauth://totp/` URI provisions is a TOTP authenticator
/// ([`virtual_mfa_device_otpauth_uri`]), and RFC 6238 states this as the default time step —
/// "we RECOMMEND a default time-step size of 30 seconds" — which is what the authenticator
/// apps that scan such a URI use when it names none.
const PASS_CODE_TIME_STEP: ::std::time::Duration = ::std::time::Duration::from_secs(30);

/// How many digits one pass code carries: "the format for this parameter is a string of six
/// digits" (`EnableMFADeviceRequest$AuthenticationCode1`).
const PASS_CODE_DIGITS: u32 = 6;

/// How many time steps either side of the current one a presented pass code is still accepted
/// at: the resynchronisation allowance RFC 6238 states — "we RECOMMEND that at most one
/// time step is allowed as the network delay" — which is what keeps a code generated a moment
/// before the step rolled over from being rejected.
const PASS_CODE_DRIFT_STEPS: u64 = 1;

/// The time step `at` falls in: the TOTP counter, `floor(unix seconds / time step)`.
///
/// `None` for an instant before the UNIX epoch, which no counter counts from.
fn pass_code_time_step(at: ::std::time::SystemTime) -> Option<u64> {
    let since_epoch = at.duration_since(::std::time::UNIX_EPOCH).ok()?;
    Some(since_epoch.as_secs() / PASS_CODE_TIME_STEP.as_secs())
}

/// The seed material a `Base32StringSeed` spells, or `None` for text that is not base32 at all
/// — "the base32 seed defined as specified in RFC3548"
/// (`VirtualMFADevice$Base32StringSeed`), which [`mint_virtual_mfa_device_seed`] writes without
/// padding and an authenticator app may be handed with it.
fn pass_code_key(base32_string_seed: &str) -> Option<Vec<u8>> {
    let spelled = base32_string_seed.trim_end_matches('=');
    ::data_encoding::BASE32_NOPAD
        .decode(spelled.as_bytes())
        .ok()
        .filter(|key| !key.is_empty())
}

/// The pass code a device holding `key` emits for the time step `step`.
///
/// RFC 6238's TOTP over RFC 4226's HOTP, computed by the `totp-lite` crate rather than here:
/// SHA-1 is the algorithm an `otpauth://` URI implies when it names none, which is what the
/// authenticator app that scanned the device's QR code computes with, and the code has to be
/// byte-identical to the one that app shows.
fn time_based_pass_code(key: &[u8], step: u64) -> String {
    ::totp_lite::totp_custom::<::totp_lite::Sha1>(
        PASS_CODE_TIME_STEP.as_secs(),
        PASS_CODE_DIGITS,
        key,
        step.saturating_mul(PASS_CODE_TIME_STEP.as_secs()),
    )
}

/// Whether a virtual MFA device provisioned with `base32_string_seed` emits `code` around
/// `at` — the one-time pass code a request presents as proof that its sender holds the device.
///
/// Decided against the steps within the drift allowance ([`PASS_CODE_DRIFT_STEPS`]) of the one
/// `at` falls in, so a code generated just before the step rolled over still authenticates. A
/// seed that is not base32, or an instant no counter counts from, authenticates nothing.
#[must_use]
pub fn virtual_mfa_device_answers(
    base32_string_seed: &str,
    code: &str,
    at: ::std::time::SystemTime,
) -> bool {
    let Some(key) = pass_code_key(base32_string_seed) else {
        return false;
    };
    let Some(step) = pass_code_time_step(at) else {
        return false;
    };
    (step.saturating_sub(PASS_CODE_DRIFT_STEPS)..=step.saturating_add(PASS_CODE_DRIFT_STEPS))
        .any(|step| time_based_pass_code(&key, step) == code)
}

/// Whether `first` and `second` are the pass codes that device emits in TWO CONSECUTIVE time
/// steps around `at`.
///
/// What `EnableMFADevice` authenticates a device with: "an authentication code emitted by the
/// device" and "a subsequent authentication code emitted by the device"
/// (`EnableMFADeviceRequest$AuthenticationCode1` / `$AuthenticationCode2`) — so the pair proves
/// the sender holds a device whose clock runs with this one, which one code alone does not.
#[must_use]
pub fn virtual_mfa_device_answers_in_sequence(
    base32_string_seed: &str,
    first: &str,
    second: &str,
    at: ::std::time::SystemTime,
) -> bool {
    let Some(key) = pass_code_key(base32_string_seed) else {
        return false;
    };
    let Some(step) = pass_code_time_step(at) else {
        return false;
    };
    (step.saturating_sub(PASS_CODE_DRIFT_STEPS)..=step.saturating_add(PASS_CODE_DRIFT_STEPS)).any(
        |step| {
            time_based_pass_code(&key, step) == first
                && time_based_pass_code(&key, step.saturating_add(1)) == second
        },
    )
}

impl ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// The virtual MFA device the account holds under the serial number `serial_number`,
    /// or `None` when it holds none.
    ///
    /// A READ: it only consults the store.
    pub fn virtual_mfa_device(&self, serial_number: &str) -> Option<&VirtualMfaDeviceRecord> {
        self.virtual_mfa_devices.get(serial_number)
    }

    /// The device held under `serial_number` as every READER of a virtual MFA device
    /// reports it: the serial, the user it is enabled for as the account holds that user
    /// now, and when it was enabled.
    ///
    /// The SECRETS are deliberately absent. "The seed information contained in the QR code
    /// and the Base32 string should be treated like any other secret access information …
    /// after you provision your virtual device, you should ensure that the information is
    /// destroyed following secure procedures" (`CreateVirtualMFADevice`), so the seed and
    /// the QR code are reported by the create that minted them and by nothing afterwards.
    ///
    /// So are the TAGS, because a LISTING reports a subset of the resource: "IAM
    /// resource-listing operations return a subset of the available attributes for the
    /// resource. For example, this operation does not return tags, even though they are an
    /// attribute of the returned object. To view tag information for a virtual MFA device,
    /// see ListMFADeviceTags" (`ListVirtualMFADevices`). The record keeps them — that is
    /// what the tag reads report — and this projection leaves them out.
    ///
    /// A READ: it only consults the store.
    pub fn virtual_mfa_device_report(
        &self,
        serial_number: &str,
        record: &VirtualMfaDeviceRecord,
    ) -> VirtualMFADevice {
        VirtualMFADevice {
            serial_number: serial_number.to_string(),
            base32_string_seed: None,
            qr_code_png: None,
            // The user as the account holds it NOW, so renaming or re-tagging that user is
            // part of what the next read of the device reports; a device whose user the
            // account no longer holds reports no user rather than a resurrected copy.
            user: record
                .user_key
                .as_ref()
                .and_then(|user_key| self.users.get(user_key))
                .cloned(),
            enable_date: record.enable_date,
            tags: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238's own published TOTP-SHA-1 test values, which pin the algorithm a virtual MFA
    /// device's pass codes are computed by: the shared secret is the ASCII `12345678901234567890`
    /// of its Appendix B (spelled in base32, which is how a device's seed is held), and each row
    /// is one of that appendix's instants with the SHA-1 code it publishes for it, read to the
    /// six digits an authentication code carries.
    const RFC_6238_SHA1_VALUES: &[(u64, &str)] = &[
        (59, "287082"),
        (1_111_111_109, "081804"),
        (1_111_111_111, "050471"),
        (1_234_567_890, "005924"),
        (2_000_000_000, "279037"),
        (20_000_000_000, "353130"),
    ];

    /// The base32 spelling of that published shared secret.
    const RFC_6238_SEED: &str = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";

    /// The instant `seconds` after the UNIX epoch.
    fn at(seconds: u64) -> ::std::time::SystemTime {
        ::std::time::UNIX_EPOCH + ::std::time::Duration::from_secs(seconds)
    }

    /// RULE: a device answers exactly the time-based pass codes of its own seed — the values
    /// RFC 6238 publishes for each instant — and no other code.
    #[test]
    fn a_device_answers_the_published_pass_codes_of_its_seed() {
        for (seconds, code) in RFC_6238_SHA1_VALUES {
            let answered = virtual_mfa_device_answers(RFC_6238_SEED, code, at(*seconds));
            assert!(
                answered,
                "case {seconds}: expected the device to answer {code}, got answered={answered}"
            );
            // One digit off is a different code, and a code of another instant is not this
            // instant's: a window wide enough to admit either would admit anything.
            let shifted = format!("{:06}", (code.parse::<u32>().unwrap_or(0) + 1) % 1_000_000);
            assert!(
                !virtual_mfa_device_answers(RFC_6238_SEED, &shifted, at(*seconds)),
                "case {seconds}: expected {shifted} not to be answered, got answered=true"
            );
            let elsewhere = at(seconds.saturating_add(600));
            assert!(
                !virtual_mfa_device_answers(RFC_6238_SEED, code, elsewhere),
                "case {seconds}: expected {code} not to be answered ten minutes later, got \
                 answered=true"
            );
        }
    }

    /// RULE: the pair `EnableMFADevice` authenticates a device with is two codes IN A ROW, so
    /// the published code of one instant followed by the next step's is accepted and any other
    /// pairing is not.
    #[test]
    fn a_pair_of_pass_codes_is_accepted_only_in_sequence() {
        // The two instants of RFC 6238's appendix that fall in CONSECUTIVE time steps:
        // 1111111109 is in step 37037036 and 1111111111 in 37037037.
        let (first, second) = ("081804", "050471");
        let now = at(1_111_111_109);
        let rows: [(&str, &str, &str, bool); 4] = [
            ("the pair in order", first, second, true),
            ("the pair reversed", second, first, false),
            ("one code twice", first, first, false),
            ("a code of another instant", first, "287082", false),
        ];
        for (case, one, two, accepted) in rows {
            let answer = virtual_mfa_device_answers_in_sequence(RFC_6238_SEED, one, two, now);
            assert_eq!(
                answer, accepted,
                "case {case:?}: expected accepted={accepted}, got {answer}"
            );
        }
    }

    /// RULE: a seed that is not base32 at all answers nothing, rather than answering every
    /// code or panicking.
    #[test]
    fn a_seed_that_is_not_base32_answers_nothing() {
        for case in ["", "not base32!", "0189"] {
            let answered = virtual_mfa_device_answers(case, "287082", at(59));
            assert!(
                !answered,
                "case {case:?}: expected no code to be answered, got answered={answered}"
            );
        }
    }
}
