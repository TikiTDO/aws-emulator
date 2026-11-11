// SPDX-License-Identifier: CC0-1.0

//! The schema and the codec a structure needs when a member of it carries
//! `@timestampFormat("date-time")`.
//!
//! WHY THIS MODULE EXISTS. Every `Timestamp` member of this service's guardrail
//! responses carries `@timestampFormat("date-time")` —
//! `smithy_query(query="operation", operation="GetGuardrail")` reports
//! `"timestampFormat": "date-time"` on `createdAt` and `updatedAt`, and
//! `smithy_query(query="shape", shape="GuardrailSummary")` the same on its two — and so
//! does every `Timestamp` member of the prompt router responses
//! (`query="operation", operation="GetPromptRouter"`), because the trait sits on the
//! `Timestamp` shape they all target. On the restJson1 wire that makes the RFC 3339 /
//! ISO-8601 STRING form part of the contract.
//! The JSON codec decides the rendering per member, from the member schema's own format
//! (`vendor/smithy-schema-server/src/json.rs::write_timestamp` ->
//! `Schema::timestamp_format()`), and falls back to the protocol default — epoch
//! seconds, a JSON NUMBER — when the member schema reports no format.
//!
//! `vendor/smithy-macros` captures `@length`/`@range`/`@pattern`/`@enum`/`@uniqueItems`,
//! `@xmlName`, `@jsonName` and the HTTP bindings onto each member schema it emits, but it
//! never emits `.with_timestamp_format(..)` (grep the tree: every call site is
//! hand-written service code). So a macro-built `date-time` member schema reports NO
//! format and every such member went onto the wire as a bare epoch number — a type no
//! generated SDK client can deserialize into the modelled `date-time`.
//!
//! `vendor/` is read-only from inside the emulator, so the fix lives here: the macros
//! below emit, for one structure, exactly what `#[smithy_macros::smithy(shape = ...)]`
//! emits — one `M_<MEMBER>` member schema per member (index = that member's position in
//! the model's member list), a `SCHEMA` naming them in that order, the
//! `<SHAPE>_SCHEMA` static, and the `SerializableStruct`/`DeserializableStruct` pair —
//! plus the one chain call the codegen omits, which [`bedrock_member_schema!`] attaches
//! for the `date_time` member kind.
//!
//! WHAT IS DELIBERATELY ABSENT.
//!
//! * Constraint traits (`@length`, `@pattern`, …). The validating walk that enforces them
//!   runs on operation INPUTS only (`aws_smithy_schema_server::validate_to_message`, which
//!   `smithy_service` calls once on the deserialized input before the handler runs). Every
//!   structure built with these macros is output-only, so the macro's captured constraints
//!   would be inert here too.
//! * Map/list sub-member schemas (`.with_list_member(..)`). Only the XML codecs read them,
//!   to name an element; this service's sole protocol is restJson1.
//! * `@xmlName`/`@jsonName`. No member of the shapes below declares either: each one's
//!   wire name IS its Smithy member name.
//!
//! THE FRACTION A DATE-TIME KEEPS. `aws_smithy_types` renders an RFC 3339 instant with its
//! fractional second TRIMMED of trailing zeros, and with no fraction at all when the
//! instant falls on a whole second. Amazon Bedrock renders the two instants of a PROMPT
//! ROUTER record at a fixed millisecond width instead, so a whole second still carries
//! `.000` there. The `date_time_fixed_millis` member kind below keeps that width: the value
//! on the wire is the same RFC 3339 JSON STRING the `date_time` kind writes — same member
//! schema, same `@timestampFormat`, never an epoch number — and only the digits between the
//! second and the `Z` differ ([`millis_text`]).
//!
//! REMOVE THIS MODULE when the vendored codegen learns `smithy.api#timestampFormat`: the
//! fix belongs there, because every `@timestampFormat` member of every service in this
//! workspace has the same defect, and this is not the first service to have to compensate
//! for it by hand — `issues/things-that-do-not-work/20261022T000000Z-smithy-macros-drops-
//! timestampformat-from-member-schemas.md` carries the generator gap, and
//! `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/types/
//! date_time_wire.rs` carries a near-identical macro for the same reason. Once
//! `vendor/smithy-macros` emits `with_timestamp_format`, delete these macros and the
//! `bedrock_schema_and_codec!` blocks and put `#[smithy_macros::smithy(shape = ...)]` back
//! on the structures that carry one. `tests/all/guardrail_wire_shape.rs` pins the wire
//! form, so it fails if the format does not reach the wire.
//!
//! The structures that need them are spread across this service's per-operation `types/`
//! modules, where each shape is declared exactly once, so `types/mod.rs` pulls this module
//! in with `#[macro_use]` — ahead of those modules, which is what puts these macros in
//! their textual scope. They are NOT `#[macro_export]`ed: `src/ops/mod.rs` reaches
//! `types` through an `include!`, so a `macro_rules!` declared under it counts as
//! macro-expanded and cannot be named by an absolute `$crate::` path (rust-lang #52234).

use ::aws_smithy_schema::serde::SerdeError;
use ::aws_smithy_types::date_time::Format;
use ::aws_smithy_types::DateTime;

/// The number of fractional-second digits a millisecond-width rendering keeps.
const MILLISECOND_DIGITS: usize = 3;

/// `value` as the RFC 3339 text of a `date-time` member whose fraction is kept at
/// millisecond width: the rendering `aws_smithy_types` produces, with its fractional
/// second padded or truncated to exactly [`MILLISECOND_DIGITS`] digits.
///
/// The calendar rendering itself stays in the crate that owns it — this adjusts only the
/// fraction of the text that crate returns, which is the one part of the form that crate
/// decides for us: it trims a zero fraction away entirely. A fraction FINER than a
/// millisecond is truncated rather than rounded, so the rendered instant is never later
/// than the one it renders.
pub fn millis_text(value: &DateTime) -> Result<String, SerdeError> {
    let rendered = value
        .fmt(Format::DateTime)
        .map_err(|error| SerdeError::custom(format!("invalid timestamp: {error}")))?;
    let instant = rendered.strip_suffix('Z').unwrap_or(&rendered);
    let (seconds, fraction) = match instant.split_once('.') {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (instant, ""),
    };
    let mut digits: String = fraction.chars().take(MILLISECOND_DIGITS).collect();
    while digits.len() < MILLISECOND_DIGITS {
        digits.push('0');
    }
    Ok(format!("{seconds}.{digits}Z"))
}

/// The `ShapeType` one member kind names. Mirrors the vendored codegen's
/// `codegen::shape_type_ident`.
macro_rules! bedrock_shape_type {
    (string) => {
        ::aws_smithy_schema::ShapeType::String
    };
    (date_time) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (date_time_fixed_millis) => {
        ::aws_smithy_schema::ShapeType::Timestamp
    };
    (integer) => {
        ::aws_smithy_schema::ShapeType::Integer
    };
    (long) => {
        ::aws_smithy_schema::ShapeType::Long
    };
    (double) => {
        ::aws_smithy_schema::ShapeType::Double
    };
    (boolean) => {
        ::aws_smithy_schema::ShapeType::Boolean
    };
    (list_of_string) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (map_of_string) => {
        ::aws_smithy_schema::ShapeType::Map
    };
    (list_of_struct) => {
        ::aws_smithy_schema::ShapeType::List
    };
    (struct_of) => {
        ::aws_smithy_schema::ShapeType::Structure
    };
    (union_of) => {
        ::aws_smithy_schema::ShapeType::Union
    };
}

/// One member's schema, as the vendored codegen would emit it, plus the
/// `@timestampFormat("date-time")` chain call it cannot.
///
/// `$kind` is the member's shape kind — `string` (a `string` or an `enum` shape, both
/// string-encoded), `boolean`, `integer`, `long`, `double`, `date_time`, `list_of_string`,
/// `map_of_string`, `list_of_struct(T)`, `struct_of(T)` or `union_of(T)`.
macro_rules! bedrock_member_schema {
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, date_time) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            ::aws_smithy_schema::ShapeType::Timestamp,
            $wire,
            $idx,
        )
        // The one call the vendored codegen omits; see the module note.
        .with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)
    };
    // The SAME member schema: a `date_time_fixed_millis` member is a `date-time`
    // `Timestamp` member like any other, and differs only in the fraction its RENDERING
    // keeps (see [`millis_text`]), so what it reports about itself must not differ.
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, date_time_fixed_millis) => {
        bedrock_member_schema!($ns, $local, $wire, $idx, date_time)
    };
    ($ns:literal, $local:literal, $wire:literal, $idx:literal, $kind:tt $(($inner:ty))?) => {
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                concat!($ns, "#", $local, "$", $wire),
                $ns,
                $wire,
            ),
            bedrock_shape_type!($kind),
            $wire,
            $idx,
        )
    };
}

/// Write one member, as the vendored codegen's `codegen::emit_write` does for that kind: a
/// list of structures writes each element against the element shape's own `SCHEMA`.
macro_rules! bedrock_write_member {
    ($s:ident, $m:expr, $v:expr, string) => { $s.write_string($m, $v.as_str())?; };
    ($s:ident, $m:expr, $v:expr, date_time) => { $s.write_timestamp($m, $v)?; };
    // A `date-time` whose fraction is kept at millisecond width: the JSON codec's
    // `write_timestamp` renders through `aws_smithy_types`, which TRIMS a zero fraction,
    // so the text is rendered here and written as the JSON string a `date-time` member is
    // on the wire. The member key and the string the writer emits are the ones
    // `write_timestamp` would emit; only the digits between the second and the `Z` differ.
    // See [`millis_text`].
    ($s:ident, $m:expr, $v:expr, date_time_fixed_millis) => {
        $s.write_string($m, &date_time_wire::millis_text($v)?)?;
    };
    ($s:ident, $m:expr, $v:expr, boolean) => { $s.write_boolean($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, integer) => { $s.write_integer($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, long) => { $s.write_long($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, double) => { $s.write_double($m, *$v)?; };
    ($s:ident, $m:expr, $v:expr, list_of_string) => {
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_string($m, __it.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, list_of_struct($inner:ty)) => {
        // A list of structures writes each element against the ELEMENT shape's own
        // `SCHEMA`, which is what the vendored codegen does for this kind
        // (`vendor/smithy-macros/src/codegen.rs::emit_write`, the `ShapeKind::List`
        // arm's `item_schema_expr`).
        $s.write_list($m, &|__sl| {
            for __it in $v.iter() {
                __sl.write_struct(&<$inner>::SCHEMA, __it)?;
            }
            Ok(())
        })?;
    };
    // A map of strings writes each entry as the vendored codegen's `emit_write` does for
    // `ShapeKind::Map`: the key as a string, then the value, both against the MEMBER's
    // schema (`vendor/smithy-macros/src/codegen.rs`, the `ShapeKind::Map` arm).
    ($s:ident, $m:expr, $v:expr, map_of_string) => {
        $s.write_map($m, &|__sm| {
            for (__k, __v) in $v.iter() {
                __sm.write_string($m, __k.as_str())?;
                __sm.write_string($m, __v.as_str())?;
            }
            Ok(())
        })?;
    };
    ($s:ident, $m:expr, $v:expr, struct_of($inner:ty)) => { $s.write_struct($m, $v)?; };
    // A union takes the same path as a structure: the vendored codegen's `emit_write`
    // writes `ShapeKind::Structure | ShapeKind::Union` alike (`write_struct`), and the
    // union's own `SerializableStruct` impl writes the one member that is set.
    ($s:ident, $m:expr, $v:expr, union_of($inner:ty)) => { $s.write_struct($m, $v)?; };
}

/// Read one member, as the vendored codegen's `codegen::emit_read` does for that kind.
///
/// Element counts are bounded by the request body the server already reads with a cap, and
/// nesting DEPTH by the codec itself: the vendored JSON deserializer takes a depth guard at
/// the entry of `read_struct`, `read_list` and `read_map` and refuses a body nesting deeper
/// than its `MAX_DEPTH`, which the protocol boundary renders as a client refusal.
macro_rules! bedrock_read_member {
    ($d:ident, $m:expr, string) => { $d.read_string($m)? };
    ($d:ident, $m:expr, date_time) => { $d.read_timestamp($m)? };
    ($d:ident, $m:expr, date_time_fixed_millis) => { $d.read_timestamp($m)? };
    ($d:ident, $m:expr, boolean) => { $d.read_boolean($m)? };
    ($d:ident, $m:expr, integer) => { $d.read_integer($m)? };
    ($d:ident, $m:expr, long) => { $d.read_long($m)? };
    ($d:ident, $m:expr, double) => { $d.read_double($m)? };
    ($d:ident, $m:expr, list_of_string) => {{
        let mut __out: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            __out.push(__dl.read_string($m)?);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, list_of_struct($inner:ty)) => {{
        let mut __out: ::std::vec::Vec<$inner> = ::std::vec::Vec::new();
        $d.read_list($m, &mut |__dl| {
            __out.push(
                <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(__dl)?,
            );
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, map_of_string) => {{
        let mut __out: ::std::collections::HashMap<::std::string::String, ::std::string::String> =
            ::std::collections::HashMap::new();
        $d.read_map($m, &mut |__k, __dm| {
            let __value = __dm.read_string($m)?;
            __out.insert(__k, __value);
            Ok(())
        })?;
        __out
    }};
    ($d:ident, $m:expr, struct_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
    ($d:ident, $m:expr, union_of($inner:ty)) => {
        <$inner as ::aws_smithy_schema_server::DeserializableStruct>::deserialize($d)?
    };
}

/// Write a member only when the value is there: a `@required` member always, an `Option`
/// one when it is `Some` — the vendored codegen's own `is_option` split.
macro_rules! bedrock_write_present_member {
    ($s:ident, $this:ident, $m:path, $field:ident, req, $($kind:tt)+) => {
        bedrock_write_member!($s, &$m, (&$this.$field), $($kind)+);
    };
    ($s:ident, $this:ident, $m:path, $field:ident, opt, $($kind:tt)+) => {
        if let ::std::option::Option::Some(__v) = $this.$field.as_ref() {
            bedrock_write_member!($s, &$m, __v, $($kind)+);
        }
    };
}

/// The value a read slot contributes to the struct: a `@required` member absent from the
/// wire is refused by name, an optional one stays `None`.
macro_rules! bedrock_member_value {
    ($slot:ident, $wire:literal, req) => {
        $slot.ok_or_else(|| ::aws_smithy_schema_server::missing_member($wire))?
    };
    ($slot:ident, $wire:literal, opt) => {
        $slot
    };
}

/// The schema, the serializer and the deserializer for one structure, from its member
/// list: everything `#[smithy_macros::smithy(shape = ...)]` emits for an output shape,
/// with `@timestampFormat` honoured.
///
/// Each member entry is `(CONST, field, "wireName", index, req|opt, kind)`; `req` is a
/// `@required` member (a bare Rust field, refused when absent on the wire) and `opt` an
/// `Option` one. The entries must be given in the model's member order, which is the order
/// `smithy_query(query="shape", shape=...)` lists them in and the order each struct
/// declares its fields in.
macro_rules! bedrock_schema_and_codec {
    (
        $ty:ident, $schema_static:ident, $shape:literal, $ns:literal, $local:literal,
        $( ( $cname:ident, $field:ident, $wire:literal, $idx:literal, $presence:tt, $($kind:tt)+ ) ),+ $(,)?
    ) => {
        #[allow(non_upper_case_globals)]
        impl $ty {
            $(
                pub const $cname: ::aws_smithy_schema::Schema =
                    bedrock_member_schema!($ns, $local, $wire, $idx, $($kind)+);
            )+

            pub const SCHEMA: ::aws_smithy_schema::Schema =
                ::aws_smithy_schema::Schema::new_struct(
                    ::aws_smithy_schema::ShapeId::from_static($shape, $ns, $local),
                    ::aws_smithy_schema::ShapeType::Structure,
                    &[$(&Self::$cname,)+],
                );
        }

        pub static $schema_static: &::aws_smithy_schema::Schema = &$ty::SCHEMA;

        impl ::aws_smithy_schema::serde::SerializableStruct for $ty {
            fn serialize_members(
                &self,
                s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
            ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
                $(
                    bedrock_write_present_member!(
                        s, self, Self::$cname, $field, $presence, $($kind)+
                    );
                )+
                Ok(())
            }
        }

        impl ::aws_smithy_schema_server::DeserializableStruct for $ty {
            fn deserialize(
                d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
            ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
                $( let mut $field = ::std::option::Option::None; )+
                d.read_struct(&Self::SCHEMA, &mut |member, d| {
                    match member.member_index() {
                        $(
                            ::std::option::Option::Some($idx) => {
                                $field = ::std::option::Option::Some(
                                    bedrock_read_member!(d, &Self::$cname, $($kind)+),
                                );
                            }
                        )+
                        _ => {}
                    }
                    Ok(())
                })?;
                Ok(Self {
                    $( $field: bedrock_member_value!($field, $wire, $presence) ),+
                })
            }
        }
    };
}
