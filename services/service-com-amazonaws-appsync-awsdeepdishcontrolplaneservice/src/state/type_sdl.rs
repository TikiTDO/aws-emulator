// SPDX-License-Identifier: CC0-1.0

//! How one stored GraphQL type is written back as SDL: the single rendering every READ of
//! a type answers with.
//!
//! A read reports `Type$definition` as the service's own printing of the type system it
//! holds rather than as the text some request supplied, so one type is reported with one
//! document however it got there — created on its own or declared inside a schema
//! document — and a client can diff, hash or line-parse what it reads.
//!
//! The layout is recorded behaviour: the declaring keyword and the type's name on a line
//! of their own, then a blank line, then the body's opening brace on its own line, each
//! member written unindented and followed by a blank line, consecutive members separated
//! by one further blank line, the closing brace on its own line, and the whole document
//! terminated by a newline. A kind whose definition has no `{ … }` body — a union, a
//! scalar — is the declaration alone, terminated the same way.
//!
//! Everything the document says comes from the parsed definition
//! ([`super::TypeDefinition`]): the kind's keyword, the interfaces it implements, the
//! members that kind carries with their arguments, default values, descriptions and
//! `@deprecated` directives. Nothing is taken from the request.

use super::schema_types::{
    Deprecation, EnumValueDefinition, FieldDefinition, InputValueDefinition, TypeDefinition,
    TypeRef, TypeWrapper, DEFAULT_DEPRECATION_REASON, DEPRECATED_DIRECTIVE,
    DEPRECATED_REASON_ARGUMENT, ENUM_KIND, INPUT_OBJECT_KIND, INTERFACE_KIND, OBJECT_KIND,
    SCALAR_KIND, UNION_KIND,
};

/// The line a rendered body opens with.
const BLOCK_OPEN: &str = "{";

/// The line a rendered body closes with.
const BLOCK_CLOSE: &str = "}";

/// The quotation a GraphQL block string — the form a description is written in — is
/// delimited by.
const BLOCK_QUOTE: &str = "\"\"\"";

/// The keyword GraphQL's SDL declares an OBJECT type with, which is also what a parsed
/// definition carrying no kind at all would be written with.
const OBJECT_KEYWORD: &str = "type";

impl TypeDefinition {
    /// This type's definition as the service renders it in GraphQL SDL.
    ///
    /// The whole definition is rendered — the description in front of it, the interfaces
    /// it implements, and every member the kind carries — so the document declares the
    /// same type it was parsed from and parses back to it.
    pub fn render_sdl(&self) -> String {
        let mut lines = description_lines(self.description.as_deref());
        lines.push(self.declaration_line());

        let members = self.member_lines();
        if !members.is_empty() {
            lines.push(String::new());
            lines.push(BLOCK_OPEN.to_string());
            for (index, member) in members.into_iter().enumerate() {
                // Consecutive members are separated by one blank line on top of the one
                // each member is followed by.
                if index > 0 {
                    lines.push(String::new());
                }
                lines.extend(member);
                lines.push(String::new());
            }
            lines.push(BLOCK_CLOSE.to_string());
        }

        let mut document = lines.join("\n");
        document.push('\n');
        document
    }

    /// The line that declares this type: its keyword and name, the interfaces an object
    /// or interface type implements, and the members a union names.
    fn declaration_line(&self) -> String {
        let mut line = format!("{} {}", keyword(self.kind), self.name);
        if !self.interfaces.is_empty() {
            line.push_str(" implements ");
            line.push_str(&self.interfaces.join(" & "));
        }
        if self.kind == UNION_KIND && !self.members.is_empty() {
            line.push_str(" = ");
            line.push_str(&self.members.join(" | "));
        }
        line
    }

    /// The members of this type's body, each as the lines it is written over: its
    /// description, if it carries one, and the member itself.
    ///
    /// Only the members the kind can hold are written, so a kind with no body at all
    /// renders as its declaration alone.
    fn member_lines(&self) -> Vec<Vec<String>> {
        match self.kind {
            OBJECT_KIND | INTERFACE_KIND => self.fields.iter().map(field_lines).collect(),
            INPUT_OBJECT_KIND => self.input_fields.iter().map(input_field_lines).collect(),
            ENUM_KIND => self.enum_values.iter().map(enum_value_lines).collect(),
            _ => Vec::new(),
        }
    }
}

/// The SDL keyword that declares a type of `kind`, as GraphQL's SDL grammar spells each
/// of the kinds a definition can declare.
fn keyword(kind: &str) -> &'static str {
    match kind {
        INTERFACE_KIND => "interface",
        INPUT_OBJECT_KIND => "input",
        ENUM_KIND => "enum",
        UNION_KIND => "union",
        SCALAR_KIND => "scalar",
        _ => OBJECT_KEYWORD,
    }
}

/// One field of an object or interface type: its description, then its name, the
/// arguments it takes, the type it resolves to and the deprecation it carries.
fn field_lines(field: &FieldDefinition) -> Vec<String> {
    let mut lines = description_lines(field.description.as_deref());
    let mut line = field.name.clone();
    if !field.arguments.is_empty() {
        let arguments: Vec<String> = field.arguments.iter().map(input_value_text).collect();
        line.push('(');
        line.push_str(&arguments.join(", "));
        line.push(')');
    }
    if let Some(value_type) = field.value_type.as_ref() {
        line.push_str(": ");
        line.push_str(&type_text(value_type));
    }
    line.push_str(&deprecation_text(&field.deprecation));
    lines.push(line);
    lines
}

/// One field of an input object type: its description, then the field itself — the same
/// shape an argument has.
fn input_field_lines(input: &InputValueDefinition) -> Vec<String> {
    let mut lines = description_lines(input.description.as_deref());
    lines.push(input_value_text(input));
    lines
}

/// One value of an enum type: its description, then the value and the deprecation it
/// carries.
fn enum_value_lines(value: &EnumValueDefinition) -> Vec<String> {
    let mut lines = description_lines(value.description.as_deref());
    let mut line = value.name.clone();
    line.push_str(&deprecation_text(&value.deprecation));
    lines.push(line);
    lines
}

/// One argument or input field: its name, the type it takes and the default value it
/// states, written the way GraphQL writes one.
fn input_value_text(input: &InputValueDefinition) -> String {
    let mut text = input.name.clone();
    if let Some(value_type) = input.value_type.as_ref() {
        text.push_str(": ");
        text.push_str(&type_text(value_type));
    }
    if let Some(default) = input.default_value.as_ref() {
        text.push_str(" = ");
        text.push_str(default);
    }
    text
}

/// One type reference, as GraphQL writes it: a name, a list of them in brackets, or a
/// non-null one marked with `!`.
///
/// Written from the name at the reference's root OUTWARDS over the wrappers it nests
/// (`TypeRef::flattened`), so a reference costs one step per wrapper and no stack frame
/// per wrapper however deeply the stored definition nested it.
fn type_text(value_type: &TypeRef) -> String {
    let (wrappers, name) = value_type.flattened();
    let mut text = name.to_string();
    for wrapper in wrappers.iter().rev() {
        match wrapper {
            TypeWrapper::List => text = format!("[{text}]"),
            TypeWrapper::NonNull => text.push('!'),
        }
    }
    text
}

/// The directive a deprecated field or enum value carries, or nothing for one that is not
/// deprecated.
///
/// The reason is written only when the definition states one of its own: the directive's
/// own default reason is what `@deprecated` alone already means, so writing it would add
/// to the definition rather than render it.
fn deprecation_text(deprecation: &Deprecation) -> String {
    if !deprecation.is_deprecated {
        return String::new();
    }
    match deprecation.reason.as_deref() {
        Some(reason) if reason != DEFAULT_DEPRECATION_REASON => format!(
            " @{DEPRECATED_DIRECTIVE}({DEPRECATED_REASON_ARGUMENT}: {})",
            quoted(reason)
        ),
        _ => format!(" @{DEPRECATED_DIRECTIVE}"),
    }
}

/// The lines a description is written over, or none at all for a definition or member
/// that carries none.
///
/// A description is a GraphQL block string written in front of what it describes: on one
/// line when the text is a single line that the delimiters cannot run into, and over its
/// own lines between two delimiters otherwise.
fn description_lines(description: Option<&str>) -> Vec<String> {
    let Some(description) = description else {
        return Vec::new();
    };
    let text = description.replace(BLOCK_QUOTE, "\\\"\"\"");
    if text.contains('\n') || text.starts_with('"') || text.ends_with('"') {
        let mut lines = vec![BLOCK_QUOTE.to_string()];
        lines.extend(text.lines().map(str::to_string));
        lines.push(BLOCK_QUOTE.to_string());
        return lines;
    }
    vec![format!("{BLOCK_QUOTE}{text}{BLOCK_QUOTE}")]
}

/// `text` as a GraphQL string value, with everything the grammar escapes inside one
/// escaped.
fn quoted(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}
