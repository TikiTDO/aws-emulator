// SPDX-License-Identifier: CC0-1.0

//! The Amazon States Language's INTRINSIC FUNCTIONS: the parser for one invocation's
//! own syntax (`States.Format('{}', $.name)`), and what each named function computes.
//!
//! One concern of the interpreter, held in one file: the invocation text lives inside a
//! single JSON string of a caller's definition, so what it may ask for is bounded here
//! ([`MAX_EXPRESSION_CHARS`], [`MAX_EXPRESSION_DEPTH`], [`MAX_ARRAY_ITEMS`]) rather
//! than by the document around it.

use super::*;

/// Evaluate an intrinsic function invocation (`States.Format('{}', $.name)`).
pub(super) fn intrinsic(text: &str, input: &Value, context: &Value) -> Result<Value, Failure> {
    invoke_intrinsic(text, input, context, 1)
}

/// [`intrinsic`], carrying how deeply the invocation nests.
///
/// This is this crate's own parser for ASL's intrinsic syntax, over text that is a
/// value of a definition the model bounds only at 1 MiB — and the nesting lives
/// inside ONE JSON string, so `serde_json`'s own recursion limit does not bound it.
/// Both the length of the invocation ([`MAX_EXPRESSION_CHARS`]) and how deeply it
/// nests ([`MAX_EXPRESSION_DEPTH`]) are therefore bounded here, and an invocation
/// past either is reported with [`RUNTIME_ERROR`] rather than parsed.
pub(super) fn invoke_intrinsic(
    text: &str,
    input: &Value,
    context: &Value,
    depth: usize,
) -> Result<Value, Failure> {
    let text = text.trim();
    if depth > MAX_EXPRESSION_DEPTH {
        return Err(runtime(intrinsic_failure(
            text,
            &format!(
                "an invocation nests more than the {MAX_EXPRESSION_DEPTH} levels this service \
                 evaluates"
            ),
        )));
    }
    if text.len() > MAX_EXPRESSION_CHARS {
        return Err(runtime(intrinsic_failure(
            text,
            &format!(
                "an invocation of {} bytes is longer than the {MAX_EXPRESSION_CHARS} this service \
                 evaluates",
                text.len()
            ),
        )));
    }
    let open = text.find('(').ok_or_else(|| {
        runtime(intrinsic_failure(
            text,
            "this is not an intrinsic-function invocation",
        ))
    })?;
    if !text.ends_with(')') {
        return Err(runtime(intrinsic_failure(
            text,
            "the invocation is not closed",
        )));
    }
    let name = text[..open].trim();
    let arguments = split_arguments(&text[open + 1..text.len() - 1])
        .map_err(|why| runtime(intrinsic_failure(text, &why)))?;
    // Each argument may select a whole document, and an invocation may carry as many
    // arguments as its text has room for, so the arguments are measured as they are
    // evaluated and bounded together: the function below cannot be handed more than a
    // state's output may carry, so it cannot build more either.
    let mut spent = 0usize;
    let mut values = Vec::with_capacity(arguments.len());
    for argument in &arguments {
        let value = argument_value(argument, input, context, depth)
            .map_err(|failure| argument_failure(text, argument, input, context, failure))?;
        spent = spent.saturating_add(measure_payload(&value, UNNAMED_PRODUCER)?);
        if spent > MAX_PAYLOAD_BYTES {
            return Err(runtime(intrinsic_failure(
                text,
                &format!(
                    "the arguments together are larger than the {MAX_PAYLOAD_BYTES} bytes a \
                     state's output may carry"
                ),
            )));
        }
        values.push(value);
    }
    let result = apply_intrinsic(name, &values, &arguments).map_err(|why| {
        runtime(intrinsic_failure(
            text,
            &match why {
                // "Invalid arguments in <function>": the one detail every way an
                // argument can be wrong is reported under.
                Invalid::Arguments => invalid_intrinsic_arguments(name),
                // The function itself failed on the arguments it was given, and says
                // what it could not do with them, naming itself.
                Invalid::Function(why) => why,
            },
        ))
    })?;
    // And what it built is bounded too, because a function may expand what it was
    // given.
    measure_payload(&result, UNNAMED_PRODUCER)?;
    Ok(result)
}

/// `failure`, as the invocation `invocation` reading the argument `argument` reports it.
///
/// An argument that is a `*Path` the document does not carry a value for is the one the
/// service words a diagnosis of its own for: the FUNCTION, the path, and the document it
/// was looked for in ([`intrinsic_function_error`], [`intrinsic_path_not_found`]), which
/// is the whole of what such a failure reports. Every other reason an argument could not
/// be evaluated is reported as this invocation failing to evaluate
/// ([`intrinsic_failure`]), and a failure a NESTED invocation already worded is carried
/// exactly as it was raised: it names the invocation it belongs to, which is the one
/// that failed. The quota sentence a nested invocation's own document raised is carried
/// as raised for the same reason — it is the STATE's result that outgrew the ceiling,
/// and the state being executed is what names itself in it ([`result_too_large`]).
fn argument_failure(
    invocation: &str,
    argument: &str,
    input: &Value,
    context: &Value,
    failure: Failure,
) -> Failure {
    let Some(why) = failure.cause else {
        return Failure {
            error: failure.error,
            cause: None,
        };
    };
    if why.starts_with(INTRINSIC_OPENING)
        || why.starts_with(FUNCTION_ERROR_OPENING)
        || is_unnamed_producer(&why)
    {
        return Failure {
            error: failure.error,
            cause: Some(why),
        };
    }
    let path = argument.trim();
    let cause = match is_unresolved_path(&why, path) {
        true => intrinsic_function_error(
            invocation,
            &intrinsic_path_not_found(path, if is_context_path(path) { context } else { input }),
        ),
        false => intrinsic_failure(invocation, &why),
    };
    Failure {
        error: failure.error,
        cause: Some(cause),
    }
}

/// Split an intrinsic's argument list on the commas that are not inside a nested
/// invocation or a single-quoted string.
pub(super) fn split_arguments(text: &str) -> Result<Vec<String>, String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' if quoted => {
                current.push(character);
                if let Some(escaped) = characters.next() {
                    current.push(escaped);
                }
            }
            '\'' => {
                quoted = !quoted;
                current.push(character);
            }
            '(' if !quoted => {
                depth += 1;
                current.push(character);
            }
            ')' if !quoted => {
                depth = depth.checked_sub(1).ok_or("unbalanced ')'")?;
                current.push(character);
            }
            ',' if !quoted && depth == 0 => {
                arguments.push(std::mem::take(&mut current));
            }
            other => current.push(other),
        }
    }
    if quoted {
        return Err("an unterminated quoted string".to_string());
    }
    if depth != 0 {
        return Err("an unclosed '('".to_string());
    }
    if !current.trim().is_empty() || !arguments.is_empty() {
        arguments.push(current);
    }
    Ok(arguments)
}

/// One intrinsic argument: a nested invocation, a path, a quoted string, or a JSON
/// literal. `depth` is the nesting of the invocation this argument belongs to.
pub(super) fn argument_value(
    text: &str,
    input: &Value,
    context: &Value,
    depth: usize,
) -> Result<Value, Failure> {
    let text = text.trim();
    if text.starts_with("States.") && text.contains('(') {
        return invoke_intrinsic(text, input, context, depth + 1);
    }
    if text.starts_with('$') {
        return select(text, input, context);
    }
    if let Some(quoted) = quoted_literal(text) {
        // "escape ... the following characters: ' { } \" with a backslash.
        let mut unescaped = String::with_capacity(quoted.len());
        let mut characters = quoted.chars();
        while let Some(character) = characters.next() {
            if character == '\\' {
                if let Some(escaped) = characters.next() {
                    unescaped.push(escaped);
                }
            } else {
                unescaped.push(character);
            }
        }
        return Ok(Value::String(unescaped));
    }
    serde_json::from_str(text).map_err(|error| {
        runtime(format!(
            "'{text}' is not a path, a quoted string or a JSON literal: {error}"
        ))
    })
}

/// The inside of an intrinsic argument written as a single-quoted string LITERAL,
/// with its backslash escapes still in place; `None` for an argument that is a path,
/// a nested invocation or a JSON literal.
pub(super) fn quoted_literal(text: &str) -> Option<&str> {
    text.trim()
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
}

/// The hashing algorithms `States.Hash` names — "MD5, SHA-1, SHA-256, SHA-384 and
/// SHA-512" — each computed by the maintained implementation of it.
///
/// The list is the function's own: an algorithm is in it because the language's
/// `States.Hash` documents it, so a definition asking for one gets the digest the
/// real service would return, and one outside it is an argument the function does not
/// take. The digest is a DATA TRANSFORMATION a payload template
/// asked for and nothing else — nothing in this emulator decides anything on one, so
/// MD5 and SHA-1 are here as members of a published catalog rather than as a choice
/// of cryptography.
pub(super) const HASH_ALGORITHMS: &[&str] = &["MD5", "SHA-1", "SHA-256", "SHA-384", "SHA-512"];

/// `data` hashed with one of [`HASH_ALGORITHMS`], as the lowercase hex digest
/// `States.Hash` returns; `None` for a name that is not one of them.
fn hash_hex(algorithm: &str, data: &[u8]) -> Option<String> {
    // The catalog decides WHICH names the function takes, so a name outside it is
    // answered for before any digest is computed.
    if !HASH_ALGORITHMS.contains(&algorithm) {
        return None;
    }
    // One `Digest` trait, implemented by each of the three crates below.
    use ::sha2::Digest as _;
    Some(match algorithm {
        "MD5" => ::hex::encode(::md5::Md5::digest(data)),
        "SHA-1" => ::hex::encode(::sha1::Sha1::digest(data)),
        "SHA-256" => ::hex::encode(::sha2::Sha256::digest(data)),
        "SHA-384" => ::hex::encode(::sha2::Sha384::digest(data)),
        "SHA-512" => ::hex::encode(::sha2::Sha512::digest(data)),
        _ => return None,
    })
}

/// Why one intrinsic function could not be applied, in the two classes the service
/// reports them under ([`invoke_intrinsic`]).
pub(super) enum Invalid {
    /// The arguments are not the ones this function takes: one is missing, one is of
    /// another type, one is a figure outside the function's domain. The service reports
    /// every one of those the same way ([`invalid_intrinsic_arguments`]).
    Arguments,
    /// The function itself could not do what the arguments asked of it, and says what
    /// it could not do ([`intrinsic_function_error`]).
    Function(String),
}

/// Apply one intrinsic function to its evaluated arguments.
///
/// `raw` carries each argument's own TEXT, as the invocation wrote it, because the
/// backslash escaping of `'`, `{`, `}` and `\` is syntax of that text rather than of
/// the value it denotes: `States.Format` therefore scans its template from the
/// literal the invocation carries, where a brace an escape made literal is data and
/// consumes no argument.
///
/// The set below is every documented intrinsic. Like any other name this closed
/// allow-list does not carry, one outside it is reported with [`RUNTIME_ERROR`]
/// rather than approximated.
pub(super) fn apply_intrinsic(
    name: &str,
    arguments: &[Value],
    raw: &[String],
) -> Result<Value, Invalid> {
    let argument = |index: usize| -> Result<&Value, Invalid> {
        arguments.get(index).ok_or(Invalid::Arguments)
    };
    // A JSON number is finite by construction, but the arithmetic below is only
    // total for a finite value, so the guard is stated rather than assumed.
    let number = |index: usize| -> Result<f64, Invalid> {
        argument(index)?
            .as_f64()
            .filter(|number| number.is_finite())
            .ok_or(Invalid::Arguments)
    };
    let string = |index: usize| -> Result<String, Invalid> {
        argument(index)?
            .as_str()
            .map(str::to_string)
            .ok_or(Invalid::Arguments)
    };
    let array = |index: usize| -> Result<Vec<Value>, Invalid> {
        let items = argument(index)?
            .as_array()
            .cloned()
            .ok_or(Invalid::Arguments)?;
        if items.len() > MAX_ARRAY_ITEMS {
            return Err(Invalid::Function(format!(
                "an array of {} items is longer than the {MAX_ARRAY_ITEMS} this function walks",
                items.len()
            )));
        }
        Ok(items)
    };

    match name {
        "States.Format" => {
            // A brace pair is a placeholder only where the invocation wrote it BARE:
            // inside a quoted literal a backslash makes the character after it data,
            // so `\{`, `\}`, `\'` and `\\` stand for themselves and consume no
            // argument. The escapes are therefore applied HERE, over the literal's
            // own text, and not a second time over the value it denotes. A template
            // that came from a path or a nested invocation is not invocation text, so
            // a backslash in it is data of its own.
            let selected = string(0)?;
            let literal = raw.first().and_then(|text| quoted_literal(text));
            let template = literal.unwrap_or(selected.as_str());
            let mut rendered = String::with_capacity(template.len());
            let mut next = 1usize;
            let mut characters = template.chars().peekable();
            while let Some(character) = characters.next() {
                match character {
                    '\\' if literal.is_some() => {
                        if let Some(escaped) = characters.next() {
                            rendered.push(escaped);
                        }
                    }
                    '{' if characters.peek() == Some(&'}') => {
                        characters.next();
                        let value = argument(next)?;
                        next += 1;
                        match value {
                            Value::String(text) => rendered.push_str(text),
                            other => rendered.push_str(&other.to_string()),
                        }
                    }
                    other => rendered.push(other),
                }
            }
            Ok(Value::String(rendered))
        }
        "States.StringToJson" => {
            let text = string(0)?;
            serde_json::from_str(&text).map_err(|error| {
                Invalid::Function(format!(
                    "Cannot convert String to Json in {name}, caused by: {}",
                    crate::state::json_parse_quoted_diagnostic(&text, &error)
                ))
            })
        }
        "States.JsonToString" => Ok(Value::String(argument(0)?.to_string())),
        "States.Array" => Ok(Value::Array(arguments.to_vec())),
        "States.ArrayPartition" => {
            let items = array(0)?;
            let size = number(1)?;
            if size < 1.0 {
                return Err(Invalid::Arguments);
            }
            Ok(Value::Array(
                items
                    .chunks(size as usize)
                    .map(|chunk| Value::Array(chunk.to_vec()))
                    .collect(),
            ))
        }
        "States.ArrayContains" => {
            let items = array(0)?;
            let looked_for = argument(1)?;
            Ok(Value::Bool(items.iter().any(|item| item == looked_for)))
        }
        "States.ArrayRange" => {
            let (start, end, step) = (number(0)?, number(1)?, number(2)?);
            if step == 0.0 {
                return Err(Invalid::Arguments);
            }
            let mut values = Vec::new();
            let mut current = start;
            while (step > 0.0 && current <= end) || (step < 0.0 && current >= end) {
                values.push(json_number(current));
                current += step;
                // The array is built from three numbers, so nothing the caller sent
                // bounds its length: this does.
                if values.len() > MAX_ARRAY_RANGE_VALUES {
                    return Err(Invalid::Function(format!(
                        "the range is longer than the {MAX_ARRAY_RANGE_VALUES} values this \
                         function builds"
                    )));
                }
            }
            Ok(Value::Array(values))
        }
        "States.ArrayGetItem" => {
            let items = array(0)?;
            let index = number(1)?;
            if index < 0.0 {
                return Err(Invalid::Arguments);
            }
            items
                .get(index as usize)
                .cloned()
                .ok_or(Invalid::Arguments)
        }
        "States.ArrayLength" => Ok(json!(array(0)?.len())),
        "States.ArrayUnique" => {
            // Compared by the canonical TEXT of each value, through a set, so the
            // pass is O(n log n) rather than the O(n^2) a linear scan per item would
            // cost on an array the execution input sized.
            let mut seen = BTreeSet::new();
            let mut unique: Vec<Value> = Vec::new();
            for item in array(0)? {
                if seen.insert(item.to_string()) {
                    unique.push(item);
                }
            }
            Ok(Value::Array(unique))
        }
        "States.JsonMerge" => {
            let mut merged = argument(0)?
                .as_object()
                .cloned()
                .ok_or(Invalid::Arguments)?;
            let second = argument(1)?.as_object().ok_or(Invalid::Arguments)?;
            let deep = argument(2)?.as_bool().unwrap_or(false);
            if deep {
                // "Currently, Step Functions only supports the shallow merging mode;
                // therefore, you must specify the Boolean value as false."
                return Err(Invalid::Arguments);
            }
            for (key, value) in second {
                merged.insert(key.clone(), value.clone());
            }
            Ok(Value::Object(merged))
        }
        "States.MathAdd" => Ok(json_number(number(0)? + number(1)?)),
        "States.MathRandom" => {
            let (start, end) = (number(0)?, number(1)?);
            if end < start {
                return Err(Invalid::Arguments);
            }
            let mut seed = match arguments.get(2).and_then(Value::as_f64) {
                Some(seed) => seed as u64,
                None => super::super::now_nanos(),
            };
            // The draw is in `[start, end)`: the end of the range is EXCLUSIVE, so
            // the span is the DISTANCE between the bounds rather than one more than
            // it, and a range whose bounds meet carries only its start.
            //
            // Both bounds are caller-chosen finite numbers, so every step of this is
            // saturating: a float-to-integer cast saturates, and the addition would
            // otherwise overflow for a range near the integer maxima.
            let span = (end - start) as u64;
            let drawn = super::super::splitmix64(&mut seed) % span.max(1);
            let drawn = i64::try_from(drawn).unwrap_or(i64::MAX);
            Ok(json!((start as i64).saturating_add(drawn)))
        }
        // "States.Base64Encode ... encode data based on MIME Base64 encoding scheme.
        // You can use this function to pass data to other AWS services without using
        // an AWS Lambda function", with a documented 10,000-character input limit.
        "States.Base64Encode" => {
            let text = string(0)?;
            if text.len() > MAX_BASE64_CHARS {
                return Err(Invalid::Function(format!(
                    "the input is {} characters, more than the {MAX_BASE64_CHARS} this function \
                     encodes",
                    text.len()
                )));
            }
            Ok(Value::String(::aws_smithy_types::base64::encode(text)))
        }
        "States.Base64Decode" => {
            let text = string(0)?;
            if text.len() > MAX_BASE64_CHARS {
                return Err(Invalid::Function(format!(
                    "the input is {} characters, more than the {MAX_BASE64_CHARS} this function \
                     decodes",
                    text.len()
                )));
            }
            let decoded =
                ::aws_smithy_types::base64::decode(&text).map_err(|_| Invalid::Arguments)?;
            String::from_utf8(decoded)
                .map(Value::String)
                .map_err(|_| Invalid::Arguments)
        }
        "States.StringSplit" => {
            let text = string(0)?;
            // Both arguments are a CALLER's, each as large as a state's document, and
            // the split asks whether every character of the text is one of the
            // delimiters: the delimiters are therefore held in a SET, so the pass costs
            // the length of the text times the log of the delimiter count rather than
            // the PRODUCT of the two lengths a linear scan per character would cost.
            let delimiters: BTreeSet<char> = string(1)?.chars().collect();
            let parts = text
                .split(|character| delimiters.contains(&character))
                .filter(|part| !part.is_empty())
                .map(|part| Value::String(part.to_string()))
                .collect();
            Ok(Value::Array(parts))
        }
        // "States.Hash(data, algorithm) ... calculate the hash value of a given
        // input", returned as the hex digest of one of the algorithms the language
        // names, with a documented 10,000-character input limit.
        "States.Hash" => {
            let data = string(0)?;
            let characters = data.chars().count();
            if characters > MAX_HASH_CHARS {
                return Err(Invalid::Function(format!(
                    "the input is {characters} characters, more than the {MAX_HASH_CHARS} this \
                     function hashes"
                )));
            }
            let algorithm = string(1)?;
            hash_hex(&algorithm, data.as_bytes())
                .map(Value::String)
                .ok_or(Invalid::Arguments)
        }
        "States.UUID" => {
            let mut seed = super::super::now_nanos();
            let high = super::super::splitmix64(&mut seed);
            let low = super::super::splitmix64(&mut seed);
            Ok(Value::String(super::super::uuid_v4(high, low)))
        }
        other => Err(Invalid::Function(format!(
            "'{other}' is an intrinsic function this emulator does not evaluate, so nothing was \
             run for it"
        ))),
    }
}

/// A number an intrinsic computed, as the JSON value the wire carries: a whole
/// result is a whole number, which is what the service's own intrinsics return.
pub(super) fn json_number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        return json!(value as i64);
    }
    json!(value)
}
