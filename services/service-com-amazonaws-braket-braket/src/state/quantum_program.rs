// SPDX-License-Identifier: CC0-1.0

//! The program a quantum task's `action` document carries, and the gate-model circuit
//! it is lowered to.
//!
//! `CreateQuantumTask$action` is a `JsonValue` carrying a Braket IR document: an object
//! whose `braketSchemaHeader` names the schema it follows and the version of it. Two
//! questions are answered here, and nowhere else:
//!
//! * is the document a Braket IR action document at all — the check the create takes
//!   before a task is minted ([`action_refusal`]);
//! * what program does it describe — the qubits, the gates and the measurements a
//!   simulator needs to run it ([`gate_model_program`]).
//!
//! The OpenQASM 3 source of a `braket.ir.openqasm.program` action is LEXED and PARSED by
//! the Qiskit project's OpenQASM 3 crates (`oq3_lexer`, `oq3_parser`), so what this
//! module reads is the real language's own token and node stream rather than a pattern
//! over the source text. The parser reports its tree as a flat stream of enter/token/exit
//! steps, which [`Tree`] collects into an arena of nodes holding indices: building it and
//! dropping it are both iterative, so the only recursion left over caller-supplied text is
//! the parser's own descent (bounded by [`MAX_PROGRAM_SOURCE_BYTES`], on the stack
//! [`super::quantum_simulation::PROGRAM_STACK_BYTES`] sizes) and this module's constant
//! folding (bounded by [`MAX_CONSTANT_DEPTH`]).
//!
//! Lowering walks that arena: a statement form the emulator cannot run is reported as the
//! reason the task could not be executed, never silently skipped.

use oq3_parser::{LexedStr, StrStep, SyntaxKind, TopEntryPoint};

/// The member of the action document that names its schema.
const SCHEMA_HEADER: &str = "braketSchemaHeader";

/// The OpenQASM program action's `source` member: the program text itself.
const SOURCE_MEMBER: &str = "source";

/// Every action schema this service publishes an action type for, which is what makes a
/// document a Braket IR action document (`super::ACTION_OPENQASM` and the other four
/// action families the device inventory publishes).
const ACTION_SCHEMAS: &[&str] = &[
    super::ACTION_OPENQASM,
    super::ACTION_JAQCD,
    super::ACTION_AHS,
    super::ACTION_ANNEALING,
    super::ACTION_BLACKBIRD,
];

/// How Amazon Braket refuses an `action` that is not one of its IR documents: the fixed
/// sentence pointing the caller at the schema package that defines them. It interpolates
/// nothing from the request, so every malformed document is refused identically.
pub const INVALID_ACTION_REFUSAL: &str = "Provided action is not valid. Please refer to the \
github repo amazon-braket-schemas-python for examples";

/// Why the action document of `action` is not one this service can accept, or `None`
/// when it is a Braket IR action document.
///
/// The document has to be a JSON object carrying a `braketSchemaHeader` whose `name` is
/// one of the schemas this service publishes and whose `version` is a string: that is
/// what every Braket IR document carries, and a value that carries none describes no
/// action at all.
#[must_use]
pub fn action_refusal(action: &str) -> Option<&'static str> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(action) else {
        return Some(INVALID_ACTION_REFUSAL);
    };
    let header = document.get(SCHEMA_HEADER);
    let name = header
        .and_then(|header| header.get("name"))
        .and_then(serde_json::Value::as_str);
    let version = header
        .and_then(|header| header.get("version"))
        .and_then(serde_json::Value::as_str);
    match (name, version) {
        (Some(name), Some(_)) if ACTION_SCHEMAS.contains(&name) => None,
        _ => Some(INVALID_ACTION_REFUSAL),
    }
}

/// Why a program could not be lowered to a circuit a simulator can run.
///
/// It is reported as the quantum task's `failureReason`, so it states what about the
/// program the emulator could not run — the statement form, the gate name or the bound
/// that was exceeded — in the service's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramRefusal(pub String);

impl ProgramRefusal {
    /// The refusal carrying `reason`, which becomes the task's `failureReason`.
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }

    /// The reason, as the `failureReason` of a task that ends in the failure status.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.0
    }
}

/// One gate of a lowered circuit: the gate the program named, the qubits it acts on in
/// the order the program listed them, and its angle parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct GateCall {
    /// The gate's name as the program spells it, lowercased.
    pub name: String,
    /// The qubits the gate acts on, as absolute indices into the program's qubits.
    pub targets: Vec<usize>,
    /// The gate's parameters, in radians where they are angles.
    pub parameters: Vec<f64>,
}

/// A gate-model program, lowered to what a state-vector simulator needs.
#[derive(Debug, Clone, PartialEq)]
pub struct GateModelProgram {
    /// How many qubits the program declares.
    pub qubit_count: usize,
    /// The gates, in program order.
    pub gates: Vec<GateCall>,
    /// The qubits the program measures, ascending. A program that measures none has
    /// every qubit measured, which is what a shot-based run of it reports.
    pub measured_qubits: Vec<usize>,
}

/// The program the `action` document describes, or why it is not one this emulator can
/// run.
///
/// Only the gate-model OpenQASM action is lowered here: the other action families
/// ([`ACTION_SCHEMAS`]) describe analog, annealing and photonic programs, which no
/// gate-model simulator runs, so a task carrying one is reported as not executed rather
/// than answered with measurements nothing produced.
pub fn gate_model_program(action: &str) -> Result<GateModelProgram, ProgramRefusal> {
    let document: serde_json::Value = serde_json::from_str(action)
        .map_err(|_| ProgramRefusal::new(INVALID_ACTION_REFUSAL))?;
    let action_type = document
        .get(SCHEMA_HEADER)
        .and_then(|header| header.get("name"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ProgramRefusal::new(INVALID_ACTION_REFUSAL))?;
    if action_type != super::ACTION_OPENQASM {
        return Err(ProgramRefusal::new(format!(
            "The action type {action_type} is not one this emulator's simulator runs: it \
             runs {} programs.",
            super::ACTION_OPENQASM
        )));
    }
    let source = document
        .get(SOURCE_MEMBER)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            ProgramRefusal::new(format!(
                "The {} action carries no {SOURCE_MEMBER} program.",
                super::ACTION_OPENQASM
            ))
        })?;
    lower_openqasm(source)
}

/// The circuit an OpenQASM 3 program describes.
///
/// A lexer or parser error is the program's own, so it is reported with that crate's
/// first message rather than guessed at.
///
/// ONE bound on the program TEXT is what keeps the parse itself bounded, and it is taken
/// before the lexer sees a byte: its LENGTH. A recursive-descent parser descends once per
/// nested construct and every construct costs at least one token, which costs at least
/// one byte of the source, so no program of at most [`MAX_PROGRAM_SOURCE_BYTES`] bytes
/// can ask the parser for more frames than that, whatever it is made of. The bound is
/// deliberately a plain length and not a count of brackets, operators or statements: a
/// measure of the program's STRUCTURE taken on raw bytes would have to agree with the
/// lexer about comments and string literals to be sound, and this one has nothing to
/// agree with. [`super::quantum_simulation::PROGRAM_STACK_BYTES`] is the stack sized for
/// it.
fn lower_openqasm(source: &str) -> Result<GateModelProgram, ProgramRefusal> {
    if source.len() > MAX_PROGRAM_SOURCE_BYTES {
        return Err(ProgramRefusal::new(format!(
            "The OpenQASM program is {} bytes, more than the {MAX_PROGRAM_SOURCE_BYTES} \
             this emulator reads for one quantum task.",
            source.len()
        )));
    }
    let tree = Tree::parse(source)?;
    let mut lowering = Lowering::new(&tree);
    for statement in tree.children(tree.root) {
        lowering.statement(*statement)?;
    }
    Ok(lowering.finish())
}

/// The most program text this emulator reads for one quantum task.
///
/// This is the figure the parse's own descent is bounded by (see [`lower_openqasm`]), so
/// it is chosen for the stack that descent runs on rather than for the longest program
/// imaginable: it holds a few hundred gate statements, far more than the qubit, gate and
/// work ceilings below admit a run of, and a program past it is reported as not executed
/// with the figures that exceeded it.
pub const MAX_PROGRAM_SOURCE_BYTES: usize = 8 << 10;

/// The deepest this module's own constant evaluation descends.
///
/// A constant is folded by walking the expression the parser built ([`Lowering::value`]),
/// which is recursion of this module's own; the budget is spent per descent, so a
/// constant nested deeper than this is reported as one the emulator could not evaluate
/// rather than followed.
pub const MAX_CONSTANT_DEPTH: usize = 64;

/// The most qubits a program this emulator runs may declare.
///
/// A state-vector simulator holds one amplitude per basis state, so the memory a run
/// needs doubles with every qubit; this is the emulator's own ceiling on that state, and
/// a program past it is reported as not executed rather than run out of memory. Amazon
/// Braket's own simulators publish higher qubit counts, which is why the ceiling is
/// stated as the emulator's and not as the device's.
pub const MAX_PROGRAM_QUBITS: usize = 20;

/// The most gates a program this emulator runs may apply, bounding the work one task
/// costs together with [`MAX_PROGRAM_QUBITS`].
pub const MAX_PROGRAM_GATES: usize = 4_096;

// ---------------------------------------------------------------------------
// The parsed program, as a flat arena
// ---------------------------------------------------------------------------

/// One node of a parsed program: a composite node of the grammar, or one token.
#[derive(Debug)]
struct Node {
    kind: SyntaxKind,
    /// The token's text; empty for a composite node.
    text: String,
    /// Indices into [`Tree::nodes`], in program order.
    children: Vec<usize>,
}

/// A parsed OpenQASM 3 program, held as an arena of nodes that name their children by
/// INDEX.
///
/// `oq3_parser` reports its tree as a flat stream of steps, so collecting it needs no
/// recursion — and because a node holds indices rather than owning its children, neither
/// does dropping it. That is what leaves the parser's own descent as the only recursion
/// over caller-supplied text.
#[derive(Debug)]
struct Tree {
    nodes: Vec<Node>,
    root: usize,
}

impl Tree {
    /// Lex and parse `source`, or report the first error either stage found.
    fn parse(source: &str) -> Result<Self, ProgramRefusal> {
        let lexed = LexedStr::new(source);
        if let Some((_, message)) = lexed.errors().next() {
            return Err(ProgramRefusal::new(format!(
                "The OpenQASM program could not be read: {message}."
            )));
        }
        let parsed = TopEntryPoint::SourceFile.parse(&lexed.to_input());

        let mut nodes: Vec<Node> = Vec::new();
        let mut open: Vec<usize> = Vec::new();
        let mut root: Option<usize> = None;
        let mut error: Option<String> = None;
        lexed.intersperse_trivia(&parsed, &mut |step| match step {
            StrStep::Enter { kind } => {
                let index = nodes.len();
                nodes.push(Node {
                    kind,
                    text: String::new(),
                    children: Vec::new(),
                });
                match open.last().copied() {
                    Some(parent) => {
                        if let Some(parent) = nodes.get_mut(parent) {
                            parent.children.push(index);
                        }
                    }
                    None => root = root.or(Some(index)),
                }
                open.push(index);
            }
            StrStep::Token { kind, text } => {
                // Whitespace and comments carry no part of the program.
                if kind.is_trivia() {
                    return;
                }
                let index = nodes.len();
                nodes.push(Node {
                    kind,
                    text: text.to_string(),
                    children: Vec::new(),
                });
                if let Some(parent) = open.last().copied() {
                    if let Some(parent) = nodes.get_mut(parent) {
                        parent.children.push(index);
                    }
                }
            }
            StrStep::Exit => {
                open.pop();
            }
            StrStep::Error { msg, pos } => {
                error = error.take().or_else(|| Some(format!("{msg} (at byte {pos})")));
            }
        });
        if let Some(message) = error {
            return Err(ProgramRefusal::new(format!(
                "The OpenQASM program could not be parsed: {message}."
            )));
        }
        let root = root.ok_or_else(|| {
            ProgramRefusal::new("The OpenQASM program parsed to nothing at all.".to_string())
        })?;
        Ok(Self { nodes, root })
    }

    /// The kind of node `index`, or `None` for an index this tree does not hold.
    fn kind(&self, index: usize) -> Option<SyntaxKind> {
        self.nodes.get(index).map(|node| node.kind)
    }

    /// The children of node `index`, in program order.
    fn children(&self, index: usize) -> &[usize] {
        self.nodes
            .get(index)
            .map_or(&[], |node| node.children.as_slice())
    }

    /// The text of node `index` when it is a token; the empty string otherwise.
    fn text(&self, index: usize) -> &str {
        self.nodes.get(index).map_or("", |node| node.text.as_str())
    }

    /// The first child of node `index` of kind `kind`.
    fn child(&self, index: usize, kind: SyntaxKind) -> Option<usize> {
        self.children(index)
            .iter()
            .copied()
            .find(|child| self.kind(*child) == Some(kind))
    }

    /// The first child of node `index` whose kind is one of `kinds`.
    fn child_of_any(&self, index: usize, kinds: &[SyntaxKind]) -> Option<usize> {
        self.children(index)
            .iter()
            .copied()
            .find(|child| self.kind(*child).is_some_and(|kind| kinds.contains(&kind)))
    }

    /// Whether the subtree at `index` holds a node of kind `kind`.
    ///
    /// Walked with an explicit stack: a program's tree is as deep as the parser let it be,
    /// and nothing here adds recursion to it.
    fn subtree_holds(&self, index: usize, kind: SyntaxKind) -> Option<usize> {
        let mut pending = vec![index];
        while let Some(node) = pending.pop() {
            if self.kind(node) == Some(kind) {
                return Some(node);
            }
            pending.extend_from_slice(self.children(node));
        }
        None
    }

    /// The source text of the subtree at `index`, as one line: the tokens it holds,
    /// separated by single spaces.
    fn subtree_text(&self, index: usize) -> String {
        let mut text = String::new();
        let mut pending = vec![index];
        let mut tokens: Vec<usize> = Vec::new();
        while let Some(node) = pending.pop() {
            let children = self.children(node);
            if children.is_empty() {
                tokens.push(node);
            } else {
                // Pushed in reverse so the stack pops them in program order.
                pending.extend(children.iter().rev().copied());
            }
        }
        for token in tokens {
            let token = self.text(token);
            if token.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(token);
        }
        text
    }
}

// ---------------------------------------------------------------------------
// The node kinds this lowering reads
//
// Each one is a `SyntaxKind` of the OpenQASM 3 grammar `oq3_parser` reports, named as
// that crate's enum spells it.
// ---------------------------------------------------------------------------

const VERSION_STRING: SyntaxKind = SyntaxKind::VERSION_STRING;
const INCLUDE: SyntaxKind = SyntaxKind::INCLUDE;
const BARRIER: SyntaxKind = SyntaxKind::BARRIER;
const QUANTUM_DECLARATION_STATEMENT: SyntaxKind = SyntaxKind::QUANTUM_DECLARATION_STATEMENT;
const CLASSICAL_DECLARATION_STATEMENT: SyntaxKind = SyntaxKind::CLASSICAL_DECLARATION_STATEMENT;
const ASSIGNMENT_STMT: SyntaxKind = SyntaxKind::ASSIGNMENT_STMT;
const EXPR_STMT: SyntaxKind = SyntaxKind::EXPR_STMT;
const MEASURE: SyntaxKind = SyntaxKind::MEASURE;
const MEASURE_EXPRESSION: SyntaxKind = SyntaxKind::MEASURE_EXPRESSION;
const GATE_CALL_EXPR: SyntaxKind = SyntaxKind::GATE_CALL_EXPR;
const QUBIT_TYPE: SyntaxKind = SyntaxKind::QUBIT_TYPE;
const DESIGNATOR: SyntaxKind = SyntaxKind::DESIGNATOR;
const NAME: SyntaxKind = SyntaxKind::NAME;
const IDENTIFIER: SyntaxKind = SyntaxKind::IDENTIFIER;
const INDEXED_IDENTIFIER: SyntaxKind = SyntaxKind::INDEXED_IDENTIFIER;
const INDEX_OPERATOR: SyntaxKind = SyntaxKind::INDEX_OPERATOR;
const HARDWARE_QUBIT: SyntaxKind = SyntaxKind::HARDWARE_QUBIT;
const QUBIT_LIST: SyntaxKind = SyntaxKind::QUBIT_LIST;
const ARG_LIST: SyntaxKind = SyntaxKind::ARG_LIST;
const EXPRESSION_LIST: SyntaxKind = SyntaxKind::EXPRESSION_LIST;
const LITERAL: SyntaxKind = SyntaxKind::LITERAL;
const INT_NUMBER: SyntaxKind = SyntaxKind::INT_NUMBER;
const FLOAT_NUMBER: SyntaxKind = SyntaxKind::FLOAT_NUMBER;
const PAREN_EXPR: SyntaxKind = SyntaxKind::PAREN_EXPR;
const PREFIX_EXPR: SyntaxKind = SyntaxKind::PREFIX_EXPR;
const BIN_EXPR: SyntaxKind = SyntaxKind::BIN_EXPR;

/// The expression kinds a constant may be written as.
const CONSTANT_KINDS: &[SyntaxKind] = &[LITERAL, IDENTIFIER, PAREN_EXPR, PREFIX_EXPR, BIN_EXPR];

/// The forms a qubit operand takes.
const OPERAND_KINDS: &[SyntaxKind] = &[IDENTIFIER, INDEXED_IDENTIFIER, HARDWARE_QUBIT];

// ---------------------------------------------------------------------------
// Lowering
// ---------------------------------------------------------------------------

/// The qubit registers a program has declared so far, and the circuit built from its
/// statements.
#[derive(Debug)]
struct Lowering<'t> {
    tree: &'t Tree,
    /// `(name, first absolute index, length)` per declared qubit register, in
    /// declaration order.
    registers: Vec<(String, usize, usize)>,
    qubit_count: usize,
    gates: Vec<GateCall>,
    /// The measured qubits, in the order the program measured them; de-duplicated and
    /// sorted by [`Lowering::finish`].
    measured: Vec<usize>,
}

impl<'t> Lowering<'t> {
    fn new(tree: &'t Tree) -> Self {
        Self {
            tree,
            registers: Vec::new(),
            qubit_count: 0,
            gates: Vec::new(),
            measured: Vec::new(),
        }
    }

    fn finish(mut self) -> GateModelProgram {
        self.measured.sort_unstable();
        self.measured.dedup();
        let measured_qubits = if self.measured.is_empty() {
            (0..self.qubit_count).collect()
        } else {
            self.measured
        };
        GateModelProgram {
            qubit_count: self.qubit_count,
            gates: self.gates,
            measured_qubits,
        }
    }

    /// Lower one statement, or report why it is not one this emulator runs.
    fn statement(&mut self, node: usize) -> Result<(), ProgramRefusal> {
        match self.tree.kind(node) {
            // The version marker, an include of a gate library and a barrier change no
            // state the simulator holds.
            Some(VERSION_STRING | INCLUDE | BARRIER) => Ok(()),
            Some(QUANTUM_DECLARATION_STATEMENT) => self.declare_qubits(node),
            // A classical declaration holds results rather than driving the circuit; the
            // one that measures as it declares still measures.
            Some(CLASSICAL_DECLARATION_STATEMENT) => self.measure_in(node, false),
            // The only assignment a shot-based program makes that the simulator has to
            // honour is a measurement; one computing a classical value is not a program
            // this emulator runs.
            Some(ASSIGNMENT_STMT) => self.measure_in(node, true),
            Some(MEASURE) => {
                let operand = self
                    .tree
                    .child_of_any(node, OPERAND_KINDS)
                    .ok_or_else(|| self.unsupported(node))?;
                let qubits = self.operand_qubits(operand)?;
                self.measured.extend(qubits);
                Ok(())
            }
            Some(EXPR_STMT) => {
                match self.tree.child_of_any(node, &[GATE_CALL_EXPR, MEASURE_EXPRESSION]) {
                    Some(child) if self.tree.kind(child) == Some(GATE_CALL_EXPR) => {
                        self.gate_call(child)
                    }
                    Some(child) => self.measure_expression(child),
                    None => Err(self.unsupported(node)),
                }
            }
            _ => Err(self.unsupported(node)),
        }
    }

    /// Record a `qubit[n] name;` declaration, which is what fixes a program's qubits and
    /// their order.
    fn declare_qubits(&mut self, node: usize) -> Result<(), ProgramRefusal> {
        let name = self
            .tree
            .child(node, NAME)
            .map(|name| self.token_text(name))
            .ok_or_else(|| self.unsupported(node))?;
        let designator = self
            .tree
            .child(node, QUBIT_TYPE)
            .and_then(|kind| self.tree.child(kind, DESIGNATOR));
        let size = match designator {
            Some(designator) => {
                let expr = self
                    .tree
                    .child_of_any(designator, CONSTANT_KINDS)
                    .ok_or_else(|| self.unsupported(node))?;
                self.integer(expr).ok_or_else(|| {
                    ProgramRefusal::new(format!(
                        "The size of the qubit register {name} is not a constant the \
                         emulator could evaluate."
                    ))
                })?
            }
            // `qubit name;` declares one qubit.
            None => 1,
        };
        if size == 0 {
            return Err(ProgramRefusal::new(format!(
                "The qubit register {name} declares no qubits."
            )));
        }
        if self.qubit_count.saturating_add(size) > MAX_PROGRAM_QUBITS {
            return Err(ProgramRefusal::new(format!(
                "The program declares more than the {MAX_PROGRAM_QUBITS} qubits this \
                 emulator's simulator holds a state vector for."
            )));
        }
        self.registers.push((name, self.qubit_count, size));
        self.qubit_count += size;
        Ok(())
    }

    /// Lower one gate call.
    ///
    /// A single-qubit gate named with a whole register is BROADCAST over that register's
    /// qubits, as OpenQASM defines; a gate of more than one qubit takes one qubit per
    /// operand, so a register in that position is not a call this emulator lowers.
    fn gate_call(&mut self, node: usize) -> Result<(), ProgramRefusal> {
        // The gate's name is the call's own identifier; the identifiers nested in its
        // qubit list name the qubits it acts on, so only a DIRECT child is the name.
        let name = self
            .tree
            .child(node, IDENTIFIER)
            .map(|identifier| self.token_text(identifier).to_lowercase())
            .ok_or_else(|| self.unsupported(node))?;
        let mut parameters = Vec::new();
        if let Some(arguments) = self
            .tree
            .child(node, ARG_LIST)
            .and_then(|list| self.tree.child(list, EXPRESSION_LIST))
            .or_else(|| self.tree.child(node, EXPRESSION_LIST))
        {
            for argument in self.tree.children(arguments) {
                if !self
                    .tree
                    .kind(*argument)
                    .is_some_and(|kind| CONSTANT_KINDS.contains(&kind))
                {
                    continue;
                }
                let value = self.value(*argument, MAX_CONSTANT_DEPTH).ok_or_else(|| {
                    ProgramRefusal::new(format!(
                        "A parameter of the gate {name} is not a constant the emulator \
                         could evaluate."
                    ))
                })?;
                parameters.push(value);
            }
        }
        let mut operands: Vec<Vec<usize>> = Vec::new();
        if let Some(list) = self.tree.child(node, QUBIT_LIST) {
            for operand in self.tree.children(list) {
                if !self
                    .tree
                    .kind(*operand)
                    .is_some_and(|kind| OPERAND_KINDS.contains(&kind))
                {
                    continue;
                }
                operands.push(self.operand_qubits(*operand)?);
            }
        }
        let Some(first) = operands.first() else {
            return Err(ProgramRefusal::new(format!(
                "The gate {name} names no qubit to act on."
            )));
        };
        if self.gates.len() + first.len() > MAX_PROGRAM_GATES {
            return Err(ProgramRefusal::new(format!(
                "The program applies more than the {MAX_PROGRAM_GATES} gates this \
                 emulator's simulator runs."
            )));
        }
        if operands.len() == 1 {
            // One operand: broadcast over whatever it names.
            for target in first.clone() {
                self.gates.push(GateCall {
                    name: name.clone(),
                    targets: vec![target],
                    parameters: parameters.clone(),
                });
            }
            return Ok(());
        }
        let mut targets = Vec::with_capacity(operands.len());
        for operand in &operands {
            match operand.as_slice() {
                [qubit] => targets.push(*qubit),
                _ => {
                    return Err(ProgramRefusal::new(format!(
                        "The gate {name} is applied to a whole qubit register, which this \
                         emulator does not broadcast for a gate of {} qubits.",
                        operands.len()
                    )))
                }
            }
        }
        if targets
            .iter()
            .collect::<std::collections::BTreeSet<&usize>>()
            .len()
            != targets.len()
        {
            return Err(ProgramRefusal::new(format!(
                "The gate {name} names the same qubit more than once."
            )));
        }
        self.gates.push(GateCall {
            name,
            targets,
            parameters,
        });
        Ok(())
    }

    /// Record the qubits measured by the measurement inside `node`.
    ///
    /// `required` says whether a statement with no measurement in it is one this emulator
    /// runs: an assignment computes a classical value, which it does not, while a
    /// declaration just holds storage, which needs nothing of the simulator.
    fn measure_in(&mut self, node: usize, required: bool) -> Result<(), ProgramRefusal> {
        match self.tree.subtree_holds(node, MEASURE_EXPRESSION) {
            Some(measure) => self.measure_expression(measure),
            None if required => Err(self.unsupported(node)),
            None => Ok(()),
        }
    }

    /// Record the qubits one `measure` expression measures.
    fn measure_expression(&mut self, node: usize) -> Result<(), ProgramRefusal> {
        let operand = self
            .tree
            .child_of_any(node, OPERAND_KINDS)
            .ok_or_else(|| self.unsupported(node))?;
        let qubits = self.operand_qubits(operand)?;
        self.measured.extend(qubits);
        Ok(())
    }

    /// The qubits an operand names: one for `q[i]`, every qubit of the register for a
    /// bare `q`.
    fn operand_qubits(&self, node: usize) -> Result<Vec<usize>, ProgramRefusal> {
        match self.tree.kind(node) {
            Some(IDENTIFIER) => self.register_qubits(self.token_text(node).as_str()),
            Some(INDEXED_IDENTIFIER) => self.indexed_qubit(node),
            Some(HARDWARE_QUBIT) => Err(ProgramRefusal::new(format!(
                "The program addresses the hardware qubit {}, which no simulator has.",
                self.tree.subtree_text(node)
            ))),
            _ => Err(self.unsupported(node)),
        }
    }

    /// Every qubit of the register `name`.
    fn register_qubits(&self, name: &str) -> Result<Vec<usize>, ProgramRefusal> {
        let (_, first, length) = self
            .registers
            .iter()
            .find(|(declared, _, _)| declared == name)
            .ok_or_else(|| {
                ProgramRefusal::new(format!(
                    "The program names the qubit register {name}, which it does not declare."
                ))
            })?;
        Ok((*first..first + length).collect())
    }

    /// The one qubit `name[index]` names.
    fn indexed_qubit(&self, node: usize) -> Result<Vec<usize>, ProgramRefusal> {
        let name = self
            .tree
            .child(node, IDENTIFIER)
            .map(|identifier| self.token_text(identifier))
            .ok_or_else(|| self.unsupported(node))?;
        let (_, first, length) = self
            .registers
            .iter()
            .find(|(declared, _, _)| *declared == name)
            .ok_or_else(|| {
                ProgramRefusal::new(format!(
                    "The program names the qubit register {name}, which it does not declare."
                ))
            })?;
        let operator = self
            .tree
            .child(node, INDEX_OPERATOR)
            .ok_or_else(|| self.unsupported(node))?;
        let list = self.tree.child(operator, EXPRESSION_LIST).ok_or_else(|| {
            ProgramRefusal::new(format!(
                "The program indexes the qubit register {name} with a set or a range, \
                 which this emulator does not lower."
            ))
        })?;
        let mut indices = self
            .tree
            .children(list)
            .iter()
            .copied()
            .filter(|child| {
                self.tree
                    .kind(*child)
                    .is_some_and(|kind| CONSTANT_KINDS.contains(&kind))
            });
        let index = indices.next().ok_or_else(|| self.unsupported(node))?;
        if indices.next().is_some() {
            return Err(ProgramRefusal::new(format!(
                "The program indexes the qubit register {name} with more than one index, \
                 which this emulator does not lower."
            )));
        }
        let index = self.integer(index).ok_or_else(|| {
            ProgramRefusal::new(format!(
                "An index of the qubit register {name} is not a constant the emulator \
                 could evaluate."
            ))
        })?;
        if index >= *length {
            return Err(ProgramRefusal::new(format!(
                "The program addresses qubit {index} of the register {name}, which holds \
                 {length}."
            )));
        }
        Ok(vec![first + index])
    }

    /// The text of the token `node` names, or of the first token under it — which is how
    /// a `NAME`, an `IDENTIFIER` and a number literal each state their own value.
    fn token_text(&self, node: usize) -> String {
        if !self.tree.text(node).is_empty() {
            return self.tree.text(node).to_string();
        }
        let mut pending: Vec<usize> = self.tree.children(node).iter().rev().copied().collect();
        while let Some(child) = pending.pop() {
            if !self.tree.text(child).is_empty() {
                return self.tree.text(child).to_string();
            }
            pending.extend(self.tree.children(child).iter().rev().copied());
        }
        String::new()
    }

    /// A constant expression as a non-negative integer — a register size or a qubit
    /// index.
    fn integer(&self, node: usize) -> Option<usize> {
        let value = self.value(node, MAX_CONSTANT_DEPTH)?;
        (value >= 0.0 && value.fract() == 0.0 && value < MAX_INDEX).then_some(value as usize)
    }

    /// A constant expression as a real number — a gate's angle, a register's size.
    ///
    /// Only the forms a program's constant can take are evaluated: a decimal literal, the
    /// mathematical constants OpenQASM defines, a sign, a parenthesis and the four
    /// arithmetic operators. Anything else is not a constant this emulator can read, and
    /// the caller is told so rather than having a value guessed for it; a value that is
    /// not FINITE is not one either, since it would reach the simulator as an angle no
    /// rotation has.
    ///
    /// `depth` is the budget this descent spends, so a constant nested deeper than
    /// [`MAX_CONSTANT_DEPTH`] is reported rather than followed.
    fn value(&self, node: usize, depth: usize) -> Option<f64> {
        let deeper = depth.checked_sub(1)?;
        let value = match self.tree.kind(node)? {
            LITERAL => {
                let number = self.tree.child_of_any(node, &[INT_NUMBER, FLOAT_NUMBER])?;
                self.tree.text(number).replace('_', "").parse::<f64>().ok()
            }
            INT_NUMBER | FLOAT_NUMBER => {
                self.tree.text(node).replace('_', "").parse::<f64>().ok()
            }
            IDENTIFIER => match self.token_text(node).as_str() {
                "pi" | "π" => Some(std::f64::consts::PI),
                "tau" | "τ" => Some(std::f64::consts::TAU),
                "euler" | "ℇ" => Some(std::f64::consts::E),
                _ => None,
            },
            PAREN_EXPR => {
                let inner = self.tree.child_of_any(node, CONSTANT_KINDS)?;
                self.value(inner, deeper)
            }
            PREFIX_EXPR => {
                let operator = self.operator(node)?;
                let operand = self.tree.child_of_any(node, CONSTANT_KINDS)?;
                let value = self.value(operand, deeper)?;
                match operator.as_str() {
                    "-" => Some(-value),
                    "+" => Some(value),
                    _ => None,
                }
            }
            BIN_EXPR => {
                let operator = self.operator(node)?;
                let mut operands = self.tree.children(node).iter().copied().filter(|child| {
                    self.tree
                        .kind(*child)
                        .is_some_and(|kind| CONSTANT_KINDS.contains(&kind))
                });
                let left = self.value(operands.next()?, deeper)?;
                let right = self.value(operands.next()?, deeper)?;
                match operator.as_str() {
                    "+" => Some(left + right),
                    "-" => Some(left - right),
                    "*" => Some(left * right),
                    "/" => (right != 0.0).then_some(left / right),
                    _ => None,
                }
            }
            _ => None,
        }?;
        value.is_finite().then_some(value)
    }

    /// The operator token of an expression node: its first child that is a token and not
    /// an operand.
    fn operator(&self, node: usize) -> Option<String> {
        self.tree
            .children(node)
            .iter()
            .copied()
            .find(|child| {
                !self.tree.text(*child).is_empty()
                    && self
                        .tree
                        .kind(*child)
                        .is_some_and(|kind| !CONSTANT_KINDS.contains(&kind))
            })
            .map(|child| self.tree.text(child).to_string())
    }

    /// The refusal a statement form this emulator does not lower earns: the statement as
    /// the program wrote it, so the caller can see which one stopped the run.
    fn unsupported(&self, node: usize) -> ProgramRefusal {
        ProgramRefusal::new(format!(
            "This emulator's simulator does not run the OpenQASM statement `{}`.",
            self.tree.subtree_text(node)
        ))
    }
}

/// The ceiling an index or a size is evaluated under, so a cast to `usize` is always
/// exact. It is far above [`MAX_PROGRAM_QUBITS`], which is the bound a program is
/// actually held to.
const MAX_INDEX: f64 = 1e6;

#[cfg(test)]
mod tests {
    use super::*;

    /// The action document of an OpenQASM program, as `CreateQuantumTask` takes it.
    fn openqasm_action(source: &str) -> String {
        serde_json::json!({
            "braketSchemaHeader": { "name": super::super::ACTION_OPENQASM, "version": "1" },
            "source": source,
        })
        .to_string()
    }

    /// A document is a Braket IR action document exactly when it carries a schema header
    /// naming one of the published action schemas.
    #[test]
    fn an_action_is_refused_unless_it_is_a_braket_ir_document() {
        // (case, document, refused)
        let cases: Vec<(&str, String, bool)> = vec![
            (
                "an OpenQASM program",
                openqasm_action("OPENQASM 3.0; qubit[1] q;"),
                false,
            ),
            (
                "an annealing problem",
                serde_json::json!({
                    "braketSchemaHeader": { "name": super::super::ACTION_ANNEALING, "version": "1" },
                })
                .to_string(),
                false,
            ),
            (
                "a document with no schema header",
                serde_json::json!({ "foo": "bar" }).to_string(),
                true,
            ),
            (
                "a header naming no Braket schema",
                serde_json::json!({
                    "braketSchemaHeader": { "name": "my.own.program", "version": "1" },
                })
                .to_string(),
                true,
            ),
            (
                "a header carrying no version",
                serde_json::json!({
                    "braketSchemaHeader": { "name": super::super::ACTION_OPENQASM },
                })
                .to_string(),
                true,
            ),
            (
                "a version that is not the string every schema carries",
                serde_json::json!({
                    "braketSchemaHeader": { "name": super::super::ACTION_OPENQASM, "version": 1 },
                })
                .to_string(),
                true,
            ),
            ("text that is not JSON at all", "not json".to_string(), true),
        ];
        for (case, document, refused) in cases {
            let answer = action_refusal(&document);
            assert_eq!(
                answer.is_some(),
                refused,
                "case {case}: expected refused={refused}, got {answer:?}"
            );
            if refused {
                assert_eq!(
                    answer,
                    Some(INVALID_ACTION_REFUSAL),
                    "case {case}: expected the service's own sentence, got {answer:?}"
                );
            }
        }
    }

    /// A gate-model program lowers to the qubits it declares, the gates it applies and
    /// the qubits it measures.
    #[test]
    fn a_program_lowers_to_its_qubits_gates_and_measurements() {
        let bell = openqasm_action(
            "OPENQASM 3.0; bit[2] b; qubit[2] q; h q[0]; cnot q[0], q[1]; \
             b[0] = measure q[0]; b[1] = measure q[1];",
        );
        let program = gate_model_program(&bell).expect("the Bell program lowers");
        assert_eq!(
            program.qubit_count, 2,
            "case the Bell circuit: expected 2 qubits, got {program:?}"
        );
        assert_eq!(
            program
                .gates
                .iter()
                .map(|gate| (gate.name.as_str(), gate.targets.clone()))
                .collect::<Vec<(&str, Vec<usize>)>>(),
            vec![("h", vec![0]), ("cnot", vec![0, 1])],
            "case the Bell circuit: expected h then cnot, got {:?}",
            program.gates
        );
        assert_eq!(
            program.measured_qubits,
            vec![0, 1],
            "case the Bell circuit: expected both qubits measured, got {program:?}"
        );
    }

    /// The forms a program's qubits, angles and measurements take, each lowered to the
    /// same circuit vocabulary.
    #[test]
    fn the_program_forms_each_lower_to_the_circuit_they_describe() {
        // (case, source, qubit count, gate names, measured qubits)
        type Case = (&'static str, &'static str, usize, Vec<&'static str>, Vec<usize>);
        let cases: Vec<Case> = vec![
            (
                "a single unindexed qubit",
                "OPENQASM 3.0; qubit q; x q;",
                1,
                vec!["x"],
                vec![0],
            ),
            (
                "two registers, addressed by index",
                "OPENQASM 3.0; qubit[2] a; qubit[1] b; cnot a[1], b[0];",
                3,
                vec!["cnot"],
                vec![0, 1, 2],
            ),
            (
                "a single-qubit gate broadcast over a register",
                "OPENQASM 3.0; qubit[3] q; h q;",
                3,
                vec!["h", "h", "h"],
                vec![0, 1, 2],
            ),
            (
                "an angle expression",
                "OPENQASM 3.0; qubit[1] q; rx(pi / 2) q[0]; measure q[0];",
                1,
                vec!["rx"],
                vec![0],
            ),
            (
                "a barrier and an include, which change no state",
                "OPENQASM 3.0; include \"stdgates.inc\"; qubit[2] q; barrier q; z q[1];",
                2,
                vec!["z"],
                vec![0, 1],
            ),
            (
                "a measurement of one qubit only",
                "OPENQASM 3.0; bit b; qubit[2] q; h q[0]; b = measure q[1];",
                2,
                vec!["h"],
                vec![1],
            ),
            (
                "a comment between the statements",
                "OPENQASM 3.0;\n// the circuit\nqubit[1] q;\nx q[0]; // flipped\n",
                1,
                vec!["x"],
                vec![0],
            ),
        ];
        for (case, source, qubits, names, measured) in cases {
            let program =
                gate_model_program(&openqasm_action(source)).unwrap_or_else(|refusal| {
                    panic!("case {case}: expected a lowered program, got {refusal:?}")
                });
            assert_eq!(
                program.qubit_count, qubits,
                "case {case}: expected {qubits} qubits, got {program:?}"
            );
            assert_eq!(
                program
                    .gates
                    .iter()
                    .map(|gate| gate.name.as_str())
                    .collect::<Vec<&str>>(),
                names,
                "case {case}: expected the gates {names:?}, got {:?}",
                program.gates
            );
            assert_eq!(
                program.measured_qubits, measured,
                "case {case}: expected the measured qubits {measured:?}, got {program:?}"
            );
        }
    }

    /// The program TEXT is bounded by its LENGTH before it is parsed, and a constant
    /// nested deeper than the emulator folds is reported rather than followed — the two
    /// bounds that keep the parse and the constant evaluation inside their stack.
    #[test]
    fn a_program_past_the_text_bounds_is_refused_before_it_is_parsed() {
        let long = format!(
            "OPENQASM 3.0; qubit[1] q;{}",
            " ".repeat(MAX_PROGRAM_SOURCE_BYTES)
        );
        let refusal = gate_model_program(&openqasm_action(&long))
            .expect_err("a program past the length bound is refused");
        assert!(
            refusal.reason().contains("more than the"),
            "case past the length bound: expected the length bound in the reason, \
             got {refusal:?}"
        );

        // Nesting the emulator will not fold — in brackets, or spelled WITHOUT them as a
        // chain of prefix or binary operators — is a constant it reports rather than
        // evaluates. Each of these stays inside the length bound, so it is the depth
        // budget that answers, and the task is told which parameter stopped it.
        let chains = [
            (
                "bracket nesting past the constant depth",
                format!(
                    "OPENQASM 3.0; qubit[1] q; rx({}1{}) q[0];",
                    "(".repeat(MAX_CONSTANT_DEPTH + 1),
                    ")".repeat(MAX_CONSTANT_DEPTH + 1)
                ),
            ),
            (
                "a prefix-operator chain",
                format!(
                    "OPENQASM 3.0; qubit[1] q; rx({}1) q[0];",
                    "-".repeat(MAX_CONSTANT_DEPTH + 1)
                ),
            ),
            (
                "a binary-operator chain",
                format!(
                    "OPENQASM 3.0; qubit[1] q; rx(1{}) q[0];",
                    "+1".repeat(MAX_CONSTANT_DEPTH + 1)
                ),
            ),
            (
                "a chain of the constants it knows",
                format!(
                    "OPENQASM 3.0; qubit[1] q; rx({}1) q[0];",
                    "pi*".repeat(MAX_CONSTANT_DEPTH + 1)
                ),
            ),
        ];
        for (case, source) in chains {
            let refusal = gate_model_program(&openqasm_action(&source))
                .expect_err(&format!("case {case}: expected a refusal"));
            assert!(
                refusal
                    .reason()
                    .contains("not a constant the emulator could evaluate"),
                "case {case}: expected the constant-depth reason, got {refusal:?}"
            );
        }

        // The control: the same shapes just inside the bounds are read.
        let shallow = format!(
            "OPENQASM 3.0; qubit[1] q; rx({}1{}) q[0];",
            "(".repeat(4),
            ")".repeat(4)
        );
        let program = gate_model_program(&openqasm_action(&shallow))
            .expect("a program inside the bounds lowers");
        assert_eq!(
            program.gates.len(),
            1,
            "case inside the bounds: expected the one gate, got {program:?}"
        );
    }

    /// A program the simulator cannot run is refused with the reason, not lowered to a
    /// circuit that drops what it could not read.
    #[test]
    fn a_program_the_simulator_cannot_run_is_refused_with_its_reason() {
        // (case, source, a fragment of the reason)
        let cases: Vec<(&str, &str, &str)> = vec![
            (
                "a syntax error",
                "OPENQASM 3.0; qubit[2] q; h q[0]",
                "could not be parsed",
            ),
            (
                "an undeclared register",
                "OPENQASM 3.0; qubit[1] q; h r[0];",
                "does not declare",
            ),
            (
                "an index past the register",
                "OPENQASM 3.0; qubit[2] q; h q[5];",
                "which holds 2",
            ),
            (
                "a control flow statement",
                "OPENQASM 3.0; qubit[1] q; for int i in [0:1] { h q[0]; }",
                "does not run the OpenQASM statement",
            ),
            (
                "a register past the qubit ceiling",
                "OPENQASM 3.0; qubit[64] q; h q[0];",
                "qubits this emulator's simulator holds",
            ),
            (
                "a non-constant angle",
                "OPENQASM 3.0; input float t; qubit[1] q; rx(t) q[0];",
                "does not run the OpenQASM statement",
            ),
            (
                "a classical assignment that computes rather than measures",
                "OPENQASM 3.0; bit[2] b; qubit[1] q; b[0] = 1;",
                "does not run the OpenQASM statement",
            ),
        ];
        for (case, source, fragment) in cases {
            let refusal = gate_model_program(&openqasm_action(source))
                .expect_err(&format!("case {case}: expected a refusal"));
            assert!(
                refusal.reason().contains(fragment),
                "case {case}: expected a reason carrying {fragment:?}, got {refusal:?}"
            );
        }
    }

    /// An action family no gate-model simulator runs is reported as such, rather than
    /// answered with measurements.
    #[test]
    fn an_action_family_the_simulator_does_not_run_is_reported() {
        for action_type in [
            super::super::ACTION_AHS,
            super::super::ACTION_ANNEALING,
            super::super::ACTION_BLACKBIRD,
            super::super::ACTION_JAQCD,
        ] {
            let document = serde_json::json!({
                "braketSchemaHeader": { "name": action_type, "version": "1" },
            })
            .to_string();
            let refusal = gate_model_program(&document)
                .expect_err(&format!("case {action_type}: expected a refusal"));
            assert!(
                refusal.reason().contains(action_type),
                "case {action_type}: expected the action type in the reason, got {refusal:?}"
            );
        }
    }
}
