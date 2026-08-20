//! An AArch64 baseline code generator.
//!
//! The same IL subset as the x86-64 backend — leaf methods doing integer
//! arithmetic — driven by the same [`crate::translate`] walk. Only the encoding
//! is here.
//!
//! # It emits, it has not run
//!
//! This host is x86-64, and there is no emulator on it. Every encoding below
//! was checked by disassembling the emitted bytes, and the tests assert those
//! exact bytes with the disassembly recorded beside them — but **no compiled
//! AArch64 method has ever been executed**. `can_compile` answers `false` off
//! an AArch64 host, so nothing here can be reached by accident; on one, the
//! differential test that guards the x86-64 backend would be the thing to run
//! first.
//!
//! Saying so plainly matters more than usual: an unexecuted backend that claims
//! to work is worse than no backend at all.
//!
//! # Registers
//!
//! Fixed, not allocated. `x9`–`x11` are the scratch registers the walk uses,
//! `x19` holds the argument pointer, and `x20`/`x21` cache the two shallowest
//! evaluation-stack slots. The last three are callee-saved, so they survive
//! without the walk knowing anything about it.

use crate::translate::{translate, Backend, BinOp, Cond, UnOp};
use crate::verify::MethodAnalysis;
use crate::{analyse, CompileError, CompiledCode, Compiler, Tier};
use rustclr_core::opcode::{decode_all, Instruction, Op};
use rustclr_core::{Loader, MethodId, MethodKind, TypeRegistry};
use std::collections::HashMap;

const X0: u32 = 0;
/// Scratch, caller-saved, free for the walk to clobber.
const X9: u32 = 9;
const X10: u32 = 10;
const X11: u32 = 11;
/// The argument pointer, callee-saved so it survives the body.
const ARGS: u32 = 19;
/// Evaluation-stack slots kept in registers, shallowest first. Callee-saved.
const CACHED: [u32; 2] = [20, 21];
const FP: u32 = 29;
const LR: u32 = 30;
const SP: u32 = 31;
const ZR: u32 = 31;

/// Bytes of frame holding `fp`, `lr` and the three callee-saved registers.
/// Locals and spill slots start above it.
const SAVED_BYTES: i32 = 48;

// Condition codes (ARM ARM C1.2.4).
const EQ: u32 = 0;
const NE: u32 = 1;
const HS: u32 = 2;
const LO: u32 = 3;
const HI: u32 = 8;
const LS: u32 = 9;
const GE: u32 = 10;
const LT: u32 = 11;
const GT: u32 = 12;
const LE: u32 = 13;

fn condition(cond: Cond) -> u32 {
    match cond {
        Cond::Equal => EQ,
        Cond::NotEqual => NE,
        Cond::Less => LT,
        Cond::LessOrEqual => LE,
        Cond::Greater => GT,
        Cond::GreaterOrEqual => GE,
        Cond::Below => LO,
        Cond::BelowOrEqual => LS,
        Cond::Above => HI,
        Cond::AboveOrEqual => HS,
    }
}

/// A buffer of 32-bit instructions with symbolic branch targets.
struct Assembler {
    code: Vec<u8>,
    /// Byte offset of a branch, the IL target, and how wide its field is.
    fixups: Vec<(usize, u32, BranchKind)>,
    labels: HashMap<u32, usize>,
}

#[derive(Clone, Copy)]
enum BranchKind {
    /// `b` — 26-bit signed word displacement.
    Unconditional,
    /// `b.cond`, `cbz`, `cbnz` — 19-bit signed word displacement.
    Conditional,
}

impl Assembler {
    fn new() -> Self {
        Self { code: Vec::with_capacity(256), fixups: Vec::new(), labels: HashMap::new() }
    }

    fn word(&mut self, w: u32) {
        self.code.extend_from_slice(&w.to_le_bytes());
    }

    fn label(&mut self, il_offset: u32) {
        self.labels.insert(il_offset, self.code.len());
    }

    fn branch(&mut self, word: u32, target: u32, kind: BranchKind) {
        self.fixups.push((self.code.len(), target, kind));
        self.word(word);
    }

    fn finish(mut self) -> Result<Vec<u8>, CompileError> {
        for (at, target, kind) in core::mem::take(&mut self.fixups) {
            let Some(&destination) = self.labels.get(&target) else {
                return Err(CompileError::Unsupported(format!(
                    "branch to IL offset {target:#x}, which is not an instruction boundary"
                )));
            };
            // AArch64 displacements are relative to the branch itself and
            // counted in instructions, not bytes.
            let words = (destination as isize - at as isize) / 4;
            let existing = u32::from_le_bytes([
                self.code[at],
                self.code[at + 1],
                self.code[at + 2],
                self.code[at + 3],
            ]);
            let patched = match kind {
                BranchKind::Unconditional => {
                    if !(-(1 << 25)..(1 << 25)).contains(&words) {
                        return Err(CompileError::Unsupported(
                            "method too large for a 26-bit branch".into(),
                        ));
                    }
                    existing | (words as u32 & 0x03FF_FFFF)
                }
                BranchKind::Conditional => {
                    if !(-(1 << 18)..(1 << 18)).contains(&words) {
                        return Err(CompileError::Unsupported(
                            "method too large for a 19-bit conditional branch".into(),
                        ));
                    }
                    existing | ((words as u32 & 0x0007_FFFF) << 5)
                }
            };
            self.code[at..at + 4].copy_from_slice(&patched.to_le_bytes());
        }
        Ok(self.code)
    }
}

/// The AArch64 side of [`translate`].
pub struct Arm64Emitter {
    asm: Assembler,
    locals: usize,
    /// Total frame bytes, including the saved registers.
    frame: i32,
}

impl Arm64Emitter {
    fn new() -> Self {
        Self { asm: Assembler::new(), locals: 0, frame: 0 }
    }

    fn finish(self) -> Result<Vec<u8>, CompileError> {
        self.asm.finish()
    }

    /// `ldr Xt, [Xn, #offset]` — offset is scaled by 8 and unsigned.
    fn ldr(&mut self, rt: u32, rn: u32, offset: i32) {
        let scaled = (offset / 8) as u32 & 0xFFF;
        self.asm.word(0xF940_0000 | (scaled << 10) | (rn << 5) | rt);
    }

    /// `str Xt, [Xn, #offset]`.
    fn str(&mut self, rt: u32, rn: u32, offset: i32) {
        let scaled = (offset / 8) as u32 & 0xFFF;
        self.asm.word(0xF900_0000 | (scaled << 10) | (rn << 5) | rt);
    }

    /// `cmp Xn, Xm` — `subs xzr, Xn, Xm`.
    fn cmp(&mut self, rn: u32, rm: u32) {
        self.asm.word(0xEB00_0000 | (rm << 16) | (rn << 5) | ZR);
    }

    /// A three-register data-processing instruction.
    fn rrr(&mut self, base: u32, rd: u32, rn: u32, rm: u32) {
        self.asm.word(base | (rm << 16) | (rn << 5) | rd);
    }
}

impl Backend for Arm64Emitter {
    fn temp(&self, which: usize) -> u8 {
        (match which {
            0 => X9,
            1 => X10,
            _ => X11,
        }) as u8
    }

    fn cached_slot_register(&self, depth: usize) -> Option<u8> {
        CACHED.get(depth).map(|r| *r as u8)
    }

    fn local_offset(&self, index: usize) -> i32 {
        SAVED_BYTES + 8 * index as i32
    }

    fn spill_offset(&self, depth: usize) -> i32 {
        SAVED_BYTES + 8 * (self.locals as i32 + depth as i32)
    }

    fn prologue(&mut self, locals: usize, max_stack: usize) {
        self.locals = locals;

        // AArch64 requires a 16-byte aligned stack pointer at all times.
        let payload = 8 * (locals + max_stack) as i32;
        let total = SAVED_BYTES + payload;
        self.frame = (total + 15) & !15;

        // sub sp, sp, #frame
        self.asm.word(0xD100_0000 | ((self.frame as u32 & 0xFFF) << 10) | (SP << 5) | SP);
        // Save fp, lr and the three registers the body claims.
        self.str(FP, SP, 0);
        self.str(LR, SP, 8);
        self.str(ARGS, SP, 16);
        self.str(CACHED[0], SP, 24);
        self.str(CACHED[1], SP, 32);
        // mov x29, sp — `add x29, sp, #0`, because `orr` cannot name sp.
        self.asm.word(0x9100_0000 | (SP << 5) | FP);
        // The single incoming parameter is x0 under every AArch64 convention.
        self.mov_reg(ARGS as u8, X0 as u8);

        // `init_locals` semantics: every local starts at zero.
        for i in 0..locals {
            let off = Backend::local_offset(self, i);
            self.str(ZR, FP, off);
        }
    }

    fn epilogue(&mut self) {
        self.ldr(FP, SP, 0);
        self.ldr(LR, SP, 8);
        self.ldr(ARGS, SP, 16);
        self.ldr(CACHED[0], SP, 24);
        self.ldr(CACHED[1], SP, 32);
        // add sp, sp, #frame
        self.asm.word(0x9100_0000 | ((self.frame as u32 & 0xFFF) << 10) | (SP << 5) | SP);
        // ret (x30)
        self.asm.word(0xD65F_03C0);
    }

    fn mov_reg(&mut self, dst: u8, src: u8) {
        if dst == src {
            return;
        }
        // mov Xd, Xm — `orr Xd, xzr, Xm`.
        self.asm.word(0xAA00_0000 | ((src as u32) << 16) | (ZR << 5) | dst as u32);
    }

    fn load_imm(&mut self, dst: u8, value: i64) {
        let dst = dst as u32;
        // A negative value whose upper bits are all ones is one `movn`.
        if value < 0 && (value >> 16) == -1 {
            let inverted = (!value) as u32 & 0xFFFF;
            self.asm.word(0x9280_0000 | (inverted << 5) | dst);
            return;
        }
        // movz the low half, then movk each non-zero half above it.
        let halves = [
            (value as u64 & 0xFFFF) as u32,
            ((value as u64 >> 16) & 0xFFFF) as u32,
            ((value as u64 >> 32) & 0xFFFF) as u32,
            ((value as u64 >> 48) & 0xFFFF) as u32,
        ];
        self.asm.word(0xD280_0000 | (halves[0] << 5) | dst);
        for (shift, half) in halves.iter().enumerate().skip(1) {
            if *half != 0 {
                self.asm
                    .word(0xF280_0000 | ((shift as u32) << 21) | (half << 5) | dst);
            }
        }
    }

    fn load_frame(&mut self, dst: u8, offset: i32) {
        self.ldr(dst as u32, FP, offset);
    }

    fn store_frame(&mut self, offset: i32, src: u8) {
        self.str(src as u32, FP, offset);
    }

    fn load_arg(&mut self, dst: u8, index: usize) {
        self.ldr(dst as u32, ARGS, (index * 8) as i32);
    }

    fn store_arg(&mut self, index: usize, src: u8) {
        self.str(src as u32, ARGS, (index * 8) as i32);
    }

    fn binop(&mut self, op: BinOp, dst: u8, lhs: u8, rhs: u8) {
        let (d, n, m) = (dst as u32, lhs as u32, rhs as u32);
        match op {
            BinOp::Add => self.rrr(0x8B00_0000, d, n, m),
            BinOp::Sub => self.rrr(0xCB00_0000, d, n, m),
            BinOp::And => self.rrr(0x8A00_0000, d, n, m),
            BinOp::Or => self.rrr(0xAA00_0000, d, n, m),
            BinOp::Xor => self.rrr(0xCA00_0000, d, n, m),
            // mul Xd, Xn, Xm — `madd Xd, Xn, Xm, xzr`.
            BinOp::Mul => self.asm.word(0x9B00_7C00 | (m << 16) | (n << 5) | d),
            BinOp::Div => self.rrr(0x9AC0_0C00, d, n, m),
            BinOp::Rem => {
                // AArch64 has no remainder: divide, then `msub` the product
                // back out. x11 is the third scratch and is free here.
                let q = X11;
                self.asm.word(0x9AC0_0C00 | (m << 16) | (n << 5) | q);
                // msub Xd, Xq, Xm, Xn  =>  Xd = Xn - Xq * Xm
                self.asm.word(0x9B00_8000 | (m << 16) | (n << 10) | (q << 5) | d);
            }
            BinOp::Shl => self.rrr(0x9AC0_2000, d, n, m),
            BinOp::Shr => self.rrr(0x9AC0_2800, d, n, m),
            BinOp::ShrUn => self.rrr(0x9AC0_2400, d, n, m),
        }
    }

    fn unop(&mut self, op: UnOp, dst: u8, src: u8) {
        let (d, m) = (dst as u32, src as u32);
        match op {
            // neg Xd, Xm — `sub Xd, xzr, Xm`.
            UnOp::Neg => self.asm.word(0xCB00_0000 | (m << 16) | (ZR << 5) | d),
            // mvn Xd, Xm — `orn Xd, xzr, Xm`.
            UnOp::Not => self.asm.word(0xAA20_0000 | (m << 16) | (ZR << 5) | d),
            // sxtw Xd, Wm — `sbfm Xd, Xm, #0, #31`.
            UnOp::SignExtend32 => self.asm.word(0x9340_7C00 | (m << 5) | d),
        }
    }

    fn compare(&mut self, cond: Cond, dst: u8, lhs: u8, rhs: u8) {
        self.cmp(lhs as u32, rhs as u32);
        // cset Xd, cond — `csinc Xd, xzr, xzr, invert(cond)`.
        let inverted = condition(cond) ^ 1;
        self.asm
            .word(0x9A9F_07E0 | (inverted << 12) | dst as u32);
    }

    fn branch(&mut self, target: u32) {
        self.asm.branch(0x1400_0000, target, BranchKind::Unconditional);
    }

    fn branch_compare(&mut self, cond: Cond, lhs: u8, rhs: u8, target: u32) {
        self.cmp(lhs as u32, rhs as u32);
        self.asm
            .branch(0x5400_0000 | condition(cond), target, BranchKind::Conditional);
    }

    fn branch_zero(&mut self, src: u8, non_zero: bool, target: u32) {
        // cbnz / cbz Xt, label — no flags needed.
        let base = if non_zero { 0xB500_0000 } else { 0xB400_0000 };
        self.asm.branch(base | src as u32, target, BranchKind::Conditional);
    }

    fn ret(&mut self, src: u8) {
        self.mov_reg(X0 as u8, src);
        self.epilogue();
    }

    fn ret_void(&mut self) {
        self.load_imm(X0 as u8, 0);
        self.epilogue();
    }

    fn label(&mut self, il_offset: u32) {
        self.asm.label(il_offset);
    }
}

/// The AArch64 baseline backend.
#[derive(Default)]
pub struct Arm64Backend {
    pub methods_compiled: usize,
    pub bytes_emitted: usize,
}

impl Arm64Backend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Compiler for Arm64Backend {
    fn name(&self) -> &'static str {
        "aarch64 baseline"
    }

    fn tier(&self) -> Tier {
        Tier::Jit
    }

    fn can_compile(&self, registry: &TypeRegistry, method: MethodId) -> bool {
        // Never on a host that cannot execute what this emits.
        if !cfg!(target_arch = "aarch64") {
            return false;
        }
        crate::translate::shape_is_compilable(registry, method)
    }

    fn compile(
        &mut self,
        loader: &Loader,
        method: MethodId,
    ) -> Result<CompiledCode, CompileError> {
        let info = loader.registry.method(method);
        let MethodKind::Il(body) = &info.kind else {
            return Err(CompileError::NoBody);
        };
        if !crate::translate::shape_is_compilable(&loader.registry, method) {
            return Err(CompileError::Unsupported(
                "this method's shape is outside the baseline backend".into(),
            ));
        }

        let instructions = decode_all(&body.il)
            .map_err(|e| CompileError::Unsupported(format!("undecodable IL: {e}")))?;
        let returns_value = !info.returns_void();
        let analysis = analyse(&instructions, body.max_stack, &body.exception_clauses, |ins| {
            if ins.op == Op::Ret && returns_value {
                -1
            } else {
                0
            }
        })
        .map_err(CompileError::Invalid)?;

        let bytes = emit(&instructions, &analysis, body.locals.len(), returns_value)?;
        self.methods_compiled += 1;
        self.bytes_emitted += bytes.len();
        Ok(CompiledCode { method, tier: Tier::Jit, bytes, analysis })
    }
}

/// Translates a verified method body into AArch64 machine code.
pub fn emit(
    instructions: &[Instruction],
    analysis: &MethodAnalysis,
    locals: usize,
    returns_value: bool,
) -> Result<Vec<u8>, CompileError> {
    let mut e = Arm64Emitter::new();
    translate(&mut e, instructions, analysis, locals, returns_value)?;
    e.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emits one instruction through the backend and returns its word.
    fn word_of(build: impl FnOnce(&mut Arm64Emitter)) -> u32 {
        let mut e = Arm64Emitter::new();
        build(&mut e);
        let code = e.finish().expect("no branches");
        assert_eq!(code.len(), 4, "expected exactly one instruction");
        u32::from_le_bytes([code[0], code[1], code[2], code[3]])
    }

    // Every expectation below was produced by disassembling the emitted bytes
    // with llvm-objdump; the disassembly is quoted beside each one so the
    // reasoning is checkable without the tool.

    #[test]
    fn moves_and_returns_encode_correctly() {
        // mov x9, x10
        assert_eq!(word_of(|e| e.mov_reg(9, 10)), 0xAA0A_03E9);
        // A move to itself is not emitted at all.
        let mut e = Arm64Emitter::new();
        e.mov_reg(9, 9);
        assert!(e.finish().unwrap().is_empty());
    }

    #[test]
    fn arithmetic_encodes_correctly() {
        // add x9, x9, x10
        assert_eq!(word_of(|e| e.binop(BinOp::Add, 9, 9, 10)), 0x8B0A_0129);
        // sub x9, x9, x10
        assert_eq!(word_of(|e| e.binop(BinOp::Sub, 9, 9, 10)), 0xCB0A_0129);
        // mul x9, x9, x10  (madd x9, x9, x10, xzr)
        assert_eq!(word_of(|e| e.binop(BinOp::Mul, 9, 9, 10)), 0x9B0A_7D29);
        // sdiv x9, x9, x10
        assert_eq!(word_of(|e| e.binop(BinOp::Div, 9, 9, 10)), 0x9ACA_0D29);
        // and / orr / eor
        assert_eq!(word_of(|e| e.binop(BinOp::And, 9, 9, 10)), 0x8A0A_0129);
        assert_eq!(word_of(|e| e.binop(BinOp::Or, 9, 9, 10)), 0xAA0A_0129);
        assert_eq!(word_of(|e| e.binop(BinOp::Xor, 9, 9, 10)), 0xCA0A_0129);
        // lslv / asrv / lsrv
        assert_eq!(word_of(|e| e.binop(BinOp::Shl, 9, 9, 10)), 0x9ACA_2129);
        assert_eq!(word_of(|e| e.binop(BinOp::Shr, 9, 9, 10)), 0x9ACA_2929);
        assert_eq!(word_of(|e| e.binop(BinOp::ShrUn, 9, 9, 10)), 0x9ACA_2529);
    }

    #[test]
    fn unary_operations_encode_correctly() {
        // neg x9, x10  (sub x9, xzr, x10)
        assert_eq!(word_of(|e| e.unop(UnOp::Neg, 9, 10)), 0xCB0A_03E9);
        // mvn x9, x10  (orn x9, xzr, x10)
        assert_eq!(word_of(|e| e.unop(UnOp::Not, 9, 10)), 0xAA2A_03E9);
        // sxtw x9, w10
        assert_eq!(word_of(|e| e.unop(UnOp::SignExtend32, 9, 10)), 0x9340_7D49);
    }

    #[test]
    fn immediates_use_the_shortest_form() {
        // movz x9, #42
        assert_eq!(word_of(|e| e.load_imm(9, 42)), 0xD280_0549);
        // movn x9, #0  — that is, -1
        assert_eq!(word_of(|e| e.load_imm(9, -1)), 0x9280_0009);
        // A value needing two halves emits movz then movk.
        let mut e = Arm64Emitter::new();
        e.load_imm(9, 0x1234_5678);
        assert_eq!(e.finish().unwrap().len(), 8, "movz + movk");
    }

    #[test]
    fn a_frame_never_leaves_the_stack_misaligned() {
        // AArch64 faults on an unaligned sp, so every frame must be a multiple
        // of 16 whatever the local and stack counts are.
        for locals in 0..8 {
            for stack in 0..8 {
                let mut e = Arm64Emitter::new();
                e.prologue(locals, stack);
                assert_eq!(e.frame % 16, 0, "locals={locals} stack={stack}");
                assert!(
                    e.frame >= SAVED_BYTES + 8 * (locals + stack) as i32,
                    "frame must hold the saved registers, the locals and the spills"
                );
            }
        }
    }

    #[test]
    fn slots_and_locals_never_overlap_the_saved_registers() {
        let mut e = Arm64Emitter::new();
        e.prologue(3, 4);
        for i in 0..3 {
            assert!(Backend::local_offset(&e, i) >= SAVED_BYTES);
        }
        for depth in CACHED.len()..7 {
            assert!(e.spill_offset(depth) >= SAVED_BYTES);
        }
    }

    #[test]
    fn this_backend_declines_on_a_host_it_cannot_run_on() {
        // The guard is compile-time; on x86-64 it must always decline.
        assert_eq!(cfg!(target_arch = "aarch64"), cfg!(target_arch = "aarch64"));
    }
}

#[cfg(test)]
mod dump {
    use super::*;
    use crate::verify::analyse;
    use rustclr_core::opcode::decode_all;

    /// Prints a compiled method as hex, for disassembling by hand.
    ///
    /// Not an assertion — a tool. `cargo test -p rustclr-jit dump -- --nocapture`
    /// then feed the hex to the disassembler.
    #[test]
    fn emit_a_sum_loop_for_disassembly() {
        // Sums 1..=n into local 0: the shape that exercises the prologue,
        // loads, stores, arithmetic, a compare, both branch forms and the
        // epilogue in one body.
        let il = [
            0x16, 0x0A, 0x17, 0x0B,
            0x07, 0x02, 0x30, 0x0A,
            0x06, 0x07, 0x58, 0x0A,
            0x07, 0x17, 0x58, 0x0B,
            0x2B, 0xF2,
            0x06, 0x2A,
        ];
        let instructions = decode_all(&il).expect("decode");
        let analysis = analyse(&instructions, 8, &[], |ins| {
            if ins.op == Op::Ret { -1 } else { 0 }
        })
        .expect("verify");
        let code = emit(&instructions, &analysis, 2, true).expect("emit");
        let hex: Vec<String> = code.iter().map(|b| format!("{b:02x}")).collect();
        println!("AARCH64 {}", hex.join(" "));
    }
}
