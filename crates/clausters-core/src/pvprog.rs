//! Bin-expression programs: the general per-frame spectral mechanism.
//!
//! A [`PvProgram`] is a small postfix program evaluated once per bin on each
//! fresh spectral frame — the `PV_Kernel` UGen's payload. Its opcodes are the
//! shared [`builtins`](crate::builtins) operator tables plus a handful of
//! per-bin loads (`mag`, `phase`, `bin`, `nbins`, `binfreq`, `p0`…), so a
//! client authors bin expressions with the *same* operator vocabulary it
//! already uses for value math and UGen graphs, and the server evaluates them
//! with the same scalar `apply_*` functions — pure `f32`, bit-identical
//! between RT and NRT.
//!
//! The lifecycle mirrors the RT rules: [`compile`] validates a program on the
//! network thread (opcode validity, stack discipline, parameter arity, length
//! cap) and precomputes the exact stack depth, so [`PvProgram::eval`] runs on
//! the audio thread as a fixed loop over a caller-provided stack — no
//! allocation, no recursion, no invalid program ever reaching it.
//!
//! Programs are a **per-bin map**: one `(mag, phase, bin, params…)` in, one
//! value out, no state between bins or frames. Cross-frame state and bin
//! remapping stay with the curated `PV_*` implementations (see
//! `docs/decisions.md`, the per-frame-mechanism entry).

use crate::builtins::{BinaryOp, UnaryOp, apply_binary, apply_unary};

/// Hard cap on a program's opcode count: bounds the per-hop work a def can
/// request (the whole program runs `nbins` times on the hop's block) and the
/// wire payload. Far above any expression written by hand.
pub const MAX_PROGRAM_OPS: usize = 256;

/// One postfix opcode. Loads push a value; operators pop their operands and
/// push the result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PvOp {
    /// Push a literal constant.
    Const(f32),
    /// Push the bin's magnitude.
    Mag,
    /// Push the bin's phase in radians (`atan2(im, re)`).
    Phase,
    /// Push the bin index (`0..=winsize/2`, as `f32`).
    Bin,
    /// Push the bin count (`winsize/2 + 1`).
    Nbins,
    /// Push the bin's center frequency in Hz (`bin * samplerate / winsize`).
    Binfreq,
    /// Push parameter `i` — the UGen's signal input `i + 1`, sampled at the
    /// hop.
    Param(u8),
    /// Apply a unary operator from the shared table to the top of stack.
    Unary(UnaryOp),
    /// Apply a binary operator from the shared table to the top two values
    /// (the earlier push is the left operand).
    Binary(BinaryOp),
}

/// Resolves one wire word to its opcode: a load name (`"mag"`, `"phase"`,
/// `"bin"`, `"nbins"`, `"binfreq"`, `"p0"`…`"p31"`) or an operator wire name
/// from the shared [`builtins`](crate::builtins) tables (which keep unary and
/// binary names disjoint). Constants are numbers on the wire, not words.
pub fn parse_word(word: &str) -> Option<PvOp> {
    match word {
        "mag" => return Some(PvOp::Mag),
        "phase" => return Some(PvOp::Phase),
        "bin" => return Some(PvOp::Bin),
        "nbins" => return Some(PvOp::Nbins),
        "binfreq" => return Some(PvOp::Binfreq),
        _ => {}
    }
    if let Some(idx) = word.strip_prefix('p')
        && let Ok(i) = idx.parse::<u8>()
        && !idx.starts_with('+')
    {
        return Some(PvOp::Param(i));
    }
    if let Some(op) = UnaryOp::from_name(word) {
        return Some(PvOp::Unary(op));
    }
    BinaryOp::from_name(word).map(PvOp::Binary)
}

/// A validated postfix program over per-bin values. Build one with
/// [`compile`]; evaluate with [`eval`](Self::eval) against a stack of at least
/// [`stack_depth`](Self::stack_depth) floats.
#[derive(Clone, Debug)]
pub struct PvProgram {
    ops: Vec<PvOp>,
    /// Maximum stack depth the program reaches (the caller's stack size).
    stack_depth: usize,
    /// Whether the program reads [`PvOp::Phase`] — lets the evaluator's caller
    /// skip the `atan2` when no program needs the polar phase.
    uses_phase: bool,
}

/// The per-bin values a program reads, filled by the caller for each bin.
#[derive(Clone, Copy)]
pub struct BinCtx<'a> {
    pub mag: f32,
    pub phase: f32,
    pub bin: f32,
    pub nbins: f32,
    pub binfreq: f32,
    /// The UGen's parameter inputs, sampled at the hop. An out-of-range
    /// `Param` reads 0 (compile rejects it; this is the eval-side backstop).
    pub params: &'a [f32],
}

impl PvProgram {
    /// The single-opcode identity program (`[Mag]` / `[Phase]`) — what an
    /// omitted expression means.
    pub fn identity(op: PvOp) -> Self {
        Self {
            ops: vec![op],
            stack_depth: 1,
            uses_phase: op == PvOp::Phase,
        }
    }

    /// Whether this program is exactly the one-opcode identity of `op`.
    pub fn is_identity(&self, op: PvOp) -> bool {
        self.ops.len() == 1 && self.ops[0] == op
    }

    pub fn stack_depth(&self) -> usize {
        self.stack_depth
    }

    pub fn uses_phase(&self) -> bool {
        self.uses_phase
    }

    /// Evaluates the program for one bin. `stack` must hold at least
    /// [`stack_depth`](Self::stack_depth) floats; allocation-free, RT-safe.
    #[inline]
    pub fn eval(&self, ctx: &BinCtx, stack: &mut [f32]) -> f32 {
        let mut sp = 0usize;
        for op in &self.ops {
            match *op {
                PvOp::Const(x) => {
                    stack[sp] = x;
                    sp += 1;
                }
                PvOp::Mag => {
                    stack[sp] = ctx.mag;
                    sp += 1;
                }
                PvOp::Phase => {
                    stack[sp] = ctx.phase;
                    sp += 1;
                }
                PvOp::Bin => {
                    stack[sp] = ctx.bin;
                    sp += 1;
                }
                PvOp::Nbins => {
                    stack[sp] = ctx.nbins;
                    sp += 1;
                }
                PvOp::Binfreq => {
                    stack[sp] = ctx.binfreq;
                    sp += 1;
                }
                PvOp::Param(i) => {
                    stack[sp] = ctx.params.get(i as usize).copied().unwrap_or(0.0);
                    sp += 1;
                }
                PvOp::Unary(u) => {
                    stack[sp - 1] = apply_unary(u, stack[sp - 1]);
                }
                PvOp::Binary(b) => {
                    sp -= 1;
                    stack[sp - 1] = apply_binary(b, stack[sp - 1], stack[sp]);
                }
            }
        }
        stack[sp - 1]
    }
}

/// Validates an opcode list into a runnable [`PvProgram`]. Checks the length
/// cap, postfix stack discipline (an operator never pops an empty stack, the
/// program nets exactly one value) and that every [`PvOp::Param`] index is
/// below `n_params` (the UGen's parameter-input count). Errors are meant for a
/// `/fail` reply, so they say what is wrong where.
pub fn compile(ops: Vec<PvOp>, n_params: usize) -> Result<PvProgram, String> {
    if ops.is_empty() {
        return Err("empty program".into());
    }
    if ops.len() > MAX_PROGRAM_OPS {
        return Err(format!(
            "program too long ({} ops, max {MAX_PROGRAM_OPS})",
            ops.len()
        ));
    }
    let mut depth = 0usize;
    let mut max_depth = 0usize;
    let mut uses_phase = false;
    for (i, op) in ops.iter().enumerate() {
        let (pops, note) = match op {
            PvOp::Unary(_) => (1, "unary operator"),
            PvOp::Binary(_) => (2, "binary operator"),
            _ => (0, ""),
        };
        if depth < pops {
            return Err(format!(
                "op {i}: {note} needs {pops} operand(s), stack has {depth}"
            ));
        }
        if let PvOp::Param(p) = op
            && *p as usize >= n_params
        {
            return Err(format!(
                "op {i}: parameter p{p} out of range (the UGen has {n_params} \
                 parameter input(s))"
            ));
        }
        if *op == PvOp::Phase {
            uses_phase = true;
        }
        depth = depth - pops + 1;
        max_depth = max_depth.max(depth);
    }
    if depth != 1 {
        return Err(format!(
            "program must leave exactly one value on the stack, leaves {depth}"
        ));
    }
    Ok(PvProgram {
        ops,
        stack_depth: max_depth,
        uses_phase,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(mag: f32, phase: f32) -> BinCtx<'static> {
        BinCtx {
            mag,
            phase,
            bin: 3.0,
            nbins: 257.0,
            binfreq: 140.625,
            params: &[0.5, 2.0],
        }
    }

    #[test]
    fn words_resolve_to_opcodes() {
        assert_eq!(parse_word("mag"), Some(PvOp::Mag));
        assert_eq!(parse_word("phase"), Some(PvOp::Phase));
        assert_eq!(parse_word("bin"), Some(PvOp::Bin));
        assert_eq!(parse_word("nbins"), Some(PvOp::Nbins));
        assert_eq!(parse_word("binfreq"), Some(PvOp::Binfreq));
        assert_eq!(parse_word("p0"), Some(PvOp::Param(0)));
        assert_eq!(parse_word("p31"), Some(PvOp::Param(31)));
        assert_eq!(parse_word("mul"), Some(PvOp::Binary(BinaryOp::Mul)));
        assert_eq!(parse_word("tanh"), Some(PvOp::Unary(UnaryOp::Tanh)));
        assert_eq!(parse_word("nope"), None);
        assert_eq!(parse_word("p"), None);
        assert_eq!(parse_word("p+1"), None);
    }

    #[test]
    fn eval_runs_a_gate_expression() {
        // mag * (mag >= p0): the spectral gate.
        let prog = compile(
            vec![
                PvOp::Mag,
                PvOp::Mag,
                PvOp::Param(0),
                PvOp::Binary(BinaryOp::Ge),
                PvOp::Binary(BinaryOp::Mul),
            ],
            1,
        )
        .unwrap();
        let mut stack = vec![0.0; prog.stack_depth()];
        assert_eq!(prog.eval(&ctx(0.8, 0.0), &mut stack), 0.8);
        assert_eq!(prog.eval(&ctx(0.3, 0.0), &mut stack), 0.0);
        assert!(!prog.uses_phase());
    }

    #[test]
    fn identity_round_trips() {
        let mag = PvProgram::identity(PvOp::Mag);
        let phase = PvProgram::identity(PvOp::Phase);
        assert!(mag.is_identity(PvOp::Mag));
        assert!(!mag.is_identity(PvOp::Phase));
        assert!(phase.uses_phase());
        let mut stack = [0.0f32; 1];
        assert_eq!(mag.eval(&ctx(0.7, 1.2), &mut stack), 0.7);
        assert_eq!(phase.eval(&ctx(0.7, 1.2), &mut stack), 1.2);
    }

    #[test]
    fn compile_rejects_bad_programs() {
        // Underflow: an operator with nothing to pop.
        assert!(compile(vec![PvOp::Binary(BinaryOp::Mul)], 0).is_err());
        assert!(compile(vec![PvOp::Mag, PvOp::Binary(BinaryOp::Mul)], 0).is_err());
        // Nets two values instead of one.
        assert!(compile(vec![PvOp::Mag, PvOp::Phase], 0).is_err());
        // Empty, over-long, parameter out of range.
        assert!(compile(vec![], 0).is_err());
        assert!(compile(vec![PvOp::Mag; MAX_PROGRAM_OPS + 1], 0).is_err());
        assert!(compile(vec![PvOp::Param(1)], 1).is_err());
        assert!(compile(vec![PvOp::Param(0)], 1).is_ok());
    }

    #[test]
    fn stack_depth_is_the_high_water_mark() {
        // (mag + p0) * (phase + p1) as postfix `mag p0 add phase p1 add mul`
        // runs depths 1,2,1,2,3,2,1 — the high-water mark is 3.
        let prog = compile(
            vec![
                PvOp::Mag,
                PvOp::Param(0),
                PvOp::Binary(BinaryOp::Add),
                PvOp::Phase,
                PvOp::Param(1),
                PvOp::Binary(BinaryOp::Add),
                PvOp::Binary(BinaryOp::Mul),
            ],
            2,
        )
        .unwrap();
        assert_eq!(prog.stack_depth(), 3);
        let mut stack = vec![0.0; prog.stack_depth()];
        let v = prog.eval(&ctx(1.5, 1.0), &mut stack);
        assert_eq!(v, (1.5 + 0.5) * (1.0 + 2.0));
    }
}
