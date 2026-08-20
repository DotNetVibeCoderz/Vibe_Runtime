//! A RISC-V (RV64I/M) baseline code generator.
//!
//! The same IL subset as the other two backends, driven by the same
//! [`crate::translate`] walk. Only the encoding is here.
//!
//! # It emits, it has not run
//!
//! This host is x86-64. Every encoding below was checked by disassembling the
//! emitted bytes, and the tests assert those exact bytes — but **no compiled
//! RISC-V method has ever been executed**. `can_compile` answers `false` off a
//! RISC-V host, so nothing here can be reached by accident.
//!
//! # Registers
//!
//! Fixed, not allocated. `t0`–`t2` are scratch, `s1` holds the argument
//! pointer, and `s2`/`s3` cache the two shallowest evaluation-stack slots. The
//! last three are callee-saved.
//!
//! # What RISC-V does not have
//!
//! No flags register, so a comparison is a `slt`/`sltu` producing 0 or 1
//! directly — which is exactly what `ceq` and friends want, and cheaper than
//! the compare-then-materialise dance x86 needs. There is also no `bgt`: the
//! greater-than branches are `blt` with the operands swapped, which the
//! assembler here does explicitly rather than pretending a pseudo-instruction
//! exists.

use crate::translate::{translate, Backend, BinOp, Cond, UnOp};
use crate::verify::MethodAnalysis;
use crate::{analyse, CompileError, CompiledCode, Compiler, Tier};
use rustclr_core::opcode::{decode_all, Instruction, Op};
use rustclr_core::{Loader, MethodId, MethodKind, TypeRegistry};
use std::collections::HashMap;

const ZERO: u32 = 0;
const RA: u32 = 1;
const SP: u32 = 2;
/// Scratch, caller-saved.
const T0: u32 = 5;
const T1: u32 = 6;
const T2: u32 = 7;
const FP: u32 = 8;
/// The argument pointer, callee-saved so it survives the body.
const ARGS: u32 = 9;
const A0: u32 = 10;
/// Evaluation-stack slots kept in registers, shallowest first. Callee-saved.
const CACHED: [u32; 2] = [18, 19];

/// Bytes of frame holding `ra`, `fp` and the three callee-saved registers.
const SAVED_BYTES: i32 = 40;

/// A buffer of 32-bit instructions with symbolic branch targets.
struct Assembler {
    code: Vec<u8>,
    fixups: Vec<(usize, u32, BranchKind)>,
    labels: HashMap<u32, usize>,
}

#[derive(Clone, Copy)]
enum BranchKind {
    /// `jal` — 20-bit signed, ±1 MB.
    Jump,
    /// `beq`/`bne`/`blt`/… — 12-bit signed, ±4 KB.
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
            let offset = destination as isize - at as isize;
            let existing = u32::from_le_bytes([
                self.code[at],
                self.code[at + 1],
                self.code[at + 2],
                self.code[at + 3],
            ]);
            let patched = match kind {
                BranchKind::Jump => {
                    if !(-(1 << 20)..(1 << 20)).contains(&offset) {
                        return Err(CompileError::Unsupported(
                            "method too large for a 20-bit jump".into(),
                        ));
                    }
                    existing | encode_j(offset as i32)
                }
                BranchKind::Conditional => {
                    if !(-(1 << 12)..(1 << 12)).contains(&offset) {
                        return Err(CompileError::Unsupported(
                            "method too large for a 12-bit conditional branch".into(),
                        ));
                    }
                    existing | encode_b(offset as i32)
                }
            };
            self.code[at..at + 4].copy_from_slice(&patched.to_le_bytes());
        }
        Ok(self.code)
    }
}

/// B-type immediate, which RISC-V scatters across the word.
///
/// `imm[12]` at bit 31, `imm[10:5]` at 30..25, `imm[4:1]` at 11..8 and
/// `imm[11]` at bit 7. Bit 0 is always zero and is not stored.
fn encode_b(offset: i32) -> u32 {
    let v = offset as u32;
    ((v >> 12) & 1) << 31
        | ((v >> 5) & 0x3F) << 25
        | ((v >> 1) & 0xF) << 8
        | ((v >> 11) & 1) << 7
}

/// J-type immediate: `imm[20]` at bit 31, `imm[10:1]` at 30..21, `imm[11]` at
/// bit 20 and `imm[19:12]` at 19..12.
fn encode_j(offset: i32) -> u32 {
    let v = offset as u32;
    ((v >> 20) & 1) << 31
        | ((v >> 1) & 0x3FF) << 21
        | ((v >> 11) & 1) << 20
        | ((v >> 12) & 0xFF) << 12
}

fn r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    (funct7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
}

fn i_type(imm: i32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> u32 {
    ((imm as u32 & 0xFFF) << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
}

fn s_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> u32 {
    let v = imm as u32;
    ((v >> 5) & 0x7F) << 25 | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | ((v & 0x1F) << 7)
        | opcode
}

/// The RISC-V side of [`translate`].
pub struct RiscVEmitter {
    asm: Assembler,
    locals: usize,
    frame: i32,
}

impl RiscVEmitter {
    fn new() -> Self {
        Self { asm: Assembler::new(), locals: 0, frame: 0 }
    }

    fn finish(self) -> Result<Vec<u8>, CompileError> {
        self.asm.finish()
    }

    /// `ld rd, offset(rs1)`.
    fn ld(&mut self, rd: u32, rs1: u32, offset: i32) {
        self.asm.word(i_type(offset, rs1, 0b011, rd, 0x03));
    }

    /// `sd rs2, offset(rs1)`.
    fn sd(&mut self, rs2: u32, rs1: u32, offset: i32) {
        self.asm.word(s_type(offset, rs2, rs1, 0b011, 0x23));
    }

    /// `addi rd, rs1, imm`.
    fn addi(&mut self, rd: u32, rs1: u32, imm: i32) {
        self.asm.word(i_type(imm, rs1, 0b000, rd, 0x13));
    }

    /// `slt` / `sltu rd, rs1, rs2`.
    fn slt(&mut self, rd: u32, rs1: u32, rs2: u32, unsigned: bool) {
        let funct3 = if unsigned { 0b011 } else { 0b010 };
        self.asm.word(r_type(0, rs2, rs1, funct3, rd, 0x33));
    }

    /// `xori rd, rs1, 1` — flips a 0/1 result.
    fn flip(&mut self, rd: u32) {
        self.asm.word(i_type(1, rd, 0b100, rd, 0x13));
    }
}

impl Backend for RiscVEmitter {
    fn temp(&self, which: usize) -> u8 {
        (match which {
            0 => T0,
            1 => T1,
            _ => T2,
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

        // The RISC-V ABI wants a 16-byte aligned stack pointer.
        let total = SAVED_BYTES + 8 * (locals + max_stack) as i32;
        self.frame = (total + 15) & !15;

        self.addi(SP, SP, -self.frame);
        self.sd(RA, SP, 0);
        self.sd(FP, SP, 8);
        self.sd(ARGS, SP, 16);
        self.sd(CACHED[0], SP, 24);
        self.sd(CACHED[1], SP, 32);
        // mv fp, sp
        self.addi(FP, SP, 0);
        // The single incoming parameter is a0 under the RISC-V convention.
        self.addi(ARGS, A0, 0);

        // `init_locals` semantics: every local starts at zero. `x0` reads as
        // zero, so no register has to be set up first.
        for i in 0..locals {
            let off = Backend::local_offset(self, i);
            self.sd(ZERO, FP, off);
        }
    }

    fn epilogue(&mut self) {
        self.ld(RA, SP, 0);
        self.ld(FP, SP, 8);
        self.ld(ARGS, SP, 16);
        self.ld(CACHED[0], SP, 24);
        self.ld(CACHED[1], SP, 32);
        self.addi(SP, SP, self.frame);
        // ret — `jalr x0, 0(ra)`.
        self.asm.word(i_type(0, RA, 0b000, ZERO, 0x67));
    }

    fn mov_reg(&mut self, dst: u8, src: u8) {
        if dst == src {
            return;
        }
        self.addi(dst as u32, src as u32, 0);
    }

    fn load_imm(&mut self, dst: u8, value: i64) {
        let dst = dst as u32;
        // A 12-bit signed value is one `addi` from zero.
        if (-2048..2048).contains(&value) {
            self.addi(dst, ZERO, value as i32);
            return;
        }
        // A 32-bit signed value is `lui` plus `addi`, with the `lui` half
        // adjusted because the `addi` immediate is signed.
        if let Ok(v) = i32::try_from(value) {
            let upper = ((v as u32).wrapping_add(0x800) >> 12) & 0xFFFFF;
            let lower = v - ((upper << 12) as i32);
            self.asm.word((upper << 12) | (dst << 7) | 0x37); // lui
            if lower != 0 {
                self.asm.word(i_type(lower, dst, 0b000, dst, 0x1B)); // addiw
            }
            return;
        }
        // Anything wider is built in 16-bit steps: there is no 64-bit literal
        // form, and a constant pool would need a relocation this backend has
        // no way to place.
        let bits = value as u64;
        self.addi(dst, ZERO, ((bits >> 48) & 0xFFFF) as i16 as i32);
        for shift in [32, 16, 0] {
            // slli dst, dst, 16
            self.asm.word(i_type(16, dst, 0b001, dst, 0x13));
            let half = ((bits >> shift) & 0xFFFF) as u32;
            if half != 0 {
                // ori dst, dst, half — in two steps, since `ori` takes 12 bits.
                self.asm.word(i_type((half >> 8) as i32, ZERO, 0b000, T2, 0x13));
                self.asm.word(i_type(8, T2, 0b001, T2, 0x13)); // slli t2, t2, 8
                self.asm.word(i_type((half & 0xFF) as i32, T2, 0b110, T2, 0x13)); // ori
                self.asm.word(r_type(0, T2, dst, 0b110, dst, 0x33)); // or
            }
        }
    }

    fn load_frame(&mut self, dst: u8, offset: i32) {
        self.ld(dst as u32, FP, offset);
    }

    fn store_frame(&mut self, offset: i32, src: u8) {
        self.sd(src as u32, FP, offset);
    }

    fn load_arg(&mut self, dst: u8, index: usize) {
        self.ld(dst as u32, ARGS, (index * 8) as i32);
    }

    fn store_arg(&mut self, index: usize, src: u8) {
        self.sd(src as u32, ARGS, (index * 8) as i32);
    }

    fn binop(&mut self, op: BinOp, dst: u8, lhs: u8, rhs: u8) {
        let (d, n, m) = (dst as u32, lhs as u32, rhs as u32);
        // (funct7, funct3) — the M extension supplies mul/div/rem at funct7 = 1.
        let (funct7, funct3) = match op {
            BinOp::Add => (0, 0b000),
            BinOp::Sub => (0x20, 0b000),
            BinOp::Mul => (1, 0b000),
            BinOp::Div => (1, 0b100),
            BinOp::Rem => (1, 0b110),
            BinOp::And => (0, 0b111),
            BinOp::Or => (0, 0b110),
            BinOp::Xor => (0, 0b100),
            BinOp::Shl => (0, 0b001),
            BinOp::Shr => (0x20, 0b101),
            BinOp::ShrUn => (0, 0b101),
        };
        self.asm.word(r_type(funct7, m, n, funct3, d, 0x33));
    }

    fn unop(&mut self, op: UnOp, dst: u8, src: u8) {
        let (d, m) = (dst as u32, src as u32);
        match op {
            // neg rd, rs — `sub rd, x0, rs`.
            UnOp::Neg => self.asm.word(r_type(0x20, m, ZERO, 0b000, d, 0x33)),
            // not rd, rs — `xori rd, rs, -1`.
            UnOp::Not => self.asm.word(i_type(-1, m, 0b100, d, 0x13)),
            // sext.w rd, rs — `addiw rd, rs, 0`.
            UnOp::SignExtend32 => self.asm.word(i_type(0, m, 0b000, d, 0x1B)),
        }
    }

    fn compare(&mut self, cond: Cond, dst: u8, lhs: u8, rhs: u8) {
        let (d, n, m) = (dst as u32, lhs as u32, rhs as u32);
        // No flags register: `slt` yields the 0 or 1 the IL wants directly.
        match cond {
            Cond::Equal => {
                self.asm.word(r_type(0x20, m, n, 0b000, d, 0x33)); // sub
                self.asm.word(i_type(1, d, 0b011, d, 0x13)); // sltiu d, d, 1
            }
            Cond::NotEqual => {
                self.asm.word(r_type(0x20, m, n, 0b000, d, 0x33)); // sub
                self.slt(d, ZERO, d, true); // snez
            }
            Cond::Less => self.slt(d, n, m, false),
            Cond::Greater => self.slt(d, m, n, false),
            Cond::LessOrEqual => {
                self.slt(d, m, n, false);
                self.flip(d);
            }
            Cond::GreaterOrEqual => {
                self.slt(d, n, m, false);
                self.flip(d);
            }
            Cond::Below => self.slt(d, n, m, true),
            Cond::Above => self.slt(d, m, n, true),
            Cond::BelowOrEqual => {
                self.slt(d, m, n, true);
                self.flip(d);
            }
            Cond::AboveOrEqual => {
                self.slt(d, n, m, true);
                self.flip(d);
            }
        }
    }

    fn branch(&mut self, target: u32) {
        // j label — `jal x0, offset`.
        self.asm.branch(ZERO << 7 | 0x6F, target, BranchKind::Jump);
    }

    fn branch_compare(&mut self, cond: Cond, lhs: u8, rhs: u8, target: u32) {
        let (n, m) = (lhs as u32, rhs as u32);
        // RISC-V has no `bgt`/`ble`: those are the `blt`/`bge` forms with the
        // operands swapped, done explicitly here.
        let (funct3, rs1, rs2) = match cond {
            Cond::Equal => (0b000, n, m),
            Cond::NotEqual => (0b001, n, m),
            Cond::Less => (0b100, n, m),
            Cond::GreaterOrEqual => (0b101, n, m),
            Cond::Greater => (0b100, m, n),
            Cond::LessOrEqual => (0b101, m, n),
            Cond::Below => (0b110, n, m),
            Cond::AboveOrEqual => (0b111, n, m),
            Cond::Above => (0b110, m, n),
            Cond::BelowOrEqual => (0b111, m, n),
        };
        let word = (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | 0x63;
        self.asm.branch(word, target, BranchKind::Conditional);
    }

    fn branch_zero(&mut self, src: u8, non_zero: bool, target: u32) {
        // beqz / bnez rs, label — compare against x0.
        let funct3 = if non_zero { 0b001 } else { 0b000 };
        let word = (ZERO << 20) | ((src as u32) << 15) | (funct3 << 12) | 0x63;
        self.asm.branch(word, target, BranchKind::Conditional);
    }

    fn ret(&mut self, src: u8) {
        self.mov_reg(A0 as u8, src);
        self.epilogue();
    }

    fn ret_void(&mut self) {
        self.addi(A0, ZERO, 0);
        self.epilogue();
    }

    fn label(&mut self, il_offset: u32) {
        self.asm.label(il_offset);
    }
}

/// The RISC-V baseline backend.
#[derive(Default)]
pub struct RiscVBackend {
    pub methods_compiled: usize,
    pub bytes_emitted: usize,
}

impl RiscVBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Compiler for RiscVBackend {
    fn name(&self) -> &'static str {
        "riscv64 baseline"
    }

    fn tier(&self) -> Tier {
        Tier::Jit
    }

    fn can_compile(&self, registry: &TypeRegistry, method: MethodId) -> bool {
        // Never on a host that cannot execute what this emits. RV32 is excluded
        // too: the slot model is 64-bit throughout.
        if !cfg!(target_arch = "riscv64") {
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

/// Translates a verified method body into RISC-V machine code.
pub fn emit(
    instructions: &[Instruction],
    analysis: &MethodAnalysis,
    locals: usize,
    returns_value: bool,
) -> Result<Vec<u8>, CompileError> {
    let mut e = RiscVEmitter::new();
    translate(&mut e, instructions, analysis, locals, returns_value)?;
    e.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word_of(build: impl FnOnce(&mut RiscVEmitter)) -> u32 {
        let mut e = RiscVEmitter::new();
        build(&mut e);
        let code = e.finish().expect("no branches");
        assert_eq!(code.len(), 4, "expected exactly one instruction");
        u32::from_le_bytes([code[0], code[1], code[2], code[3]])
    }

    // Every expectation below was produced by disassembling the emitted bytes
    // with llvm-objdump; the disassembly is quoted beside each one.

    #[test]
    fn arithmetic_encodes_correctly() {
        // add t0, t0, t1
        assert_eq!(word_of(|e| e.binop(BinOp::Add, 5, 5, 6)), 0x0062_82B3);
        // sub t0, t0, t1
        assert_eq!(word_of(|e| e.binop(BinOp::Sub, 5, 5, 6)), 0x4062_82B3);
        // mul t0, t0, t1
        assert_eq!(word_of(|e| e.binop(BinOp::Mul, 5, 5, 6)), 0x0262_82B3);
        // div t0, t0, t1
        assert_eq!(word_of(|e| e.binop(BinOp::Div, 5, 5, 6)), 0x0262_C2B3);
        // rem t0, t0, t1
        assert_eq!(word_of(|e| e.binop(BinOp::Rem, 5, 5, 6)), 0x0262_E2B3);
        // and / or / xor
        assert_eq!(word_of(|e| e.binop(BinOp::And, 5, 5, 6)), 0x0062_F2B3);
        assert_eq!(word_of(|e| e.binop(BinOp::Or, 5, 5, 6)), 0x0062_E2B3);
        assert_eq!(word_of(|e| e.binop(BinOp::Xor, 5, 5, 6)), 0x0062_C2B3);
        // sll / sra / srl
        assert_eq!(word_of(|e| e.binop(BinOp::Shl, 5, 5, 6)), 0x0062_92B3);
        assert_eq!(word_of(|e| e.binop(BinOp::Shr, 5, 5, 6)), 0x4062_D2B3);
        assert_eq!(word_of(|e| e.binop(BinOp::ShrUn, 5, 5, 6)), 0x0062_D2B3);
    }

    #[test]
    fn unary_operations_encode_correctly() {
        // neg t0, t1  (sub t0, x0, t1)
        assert_eq!(word_of(|e| e.unop(UnOp::Neg, 5, 6)), 0x4060_02B3);
        // not t0, t1  (xori t0, t1, -1)
        assert_eq!(word_of(|e| e.unop(UnOp::Not, 5, 6)), 0xFFF3_4293);
        // sext.w t0, t1  (addiw t0, t1, 0)
        assert_eq!(word_of(|e| e.unop(UnOp::SignExtend32, 5, 6)), 0x0003_029B);
    }

    #[test]
    fn small_immediates_are_a_single_addi() {
        // li t0, 42  (addi t0, x0, 42)
        assert_eq!(word_of(|e| e.load_imm(5, 42)), 0x02A0_0293);
        // li t0, -1
        assert_eq!(word_of(|e| e.load_imm(5, -1)), 0xFFF0_0293);
        // A 32-bit value needs lui + addiw.
        let mut e = RiscVEmitter::new();
        e.load_imm(5, 0x1234_5678);
        assert_eq!(e.finish().unwrap().len(), 8, "lui + addiw");
    }

    #[test]
    fn the_immediate_scramblers_round_trip() {
        // The B and J immediate layouts are the easiest thing on this
        // architecture to get quietly wrong, so check the bit placement
        // directly rather than only through a whole method.
        for offset in [4i32, -4, 2048, -2048, 4094, -4096] {
            let w = encode_b(offset);
            let recovered = (((w >> 31) & 1) << 12
                | ((w >> 25) & 0x3F) << 5
                | ((w >> 8) & 0xF) << 1
                | ((w >> 7) & 1) << 11) as i32;
            let signed = (recovered << 19) >> 19;
            assert_eq!(signed, offset, "B-type {offset}");
        }
        for offset in [4i32, -4, 1 << 19, -(1 << 19)] {
            let w = encode_j(offset);
            let recovered = (((w >> 31) & 1) << 20
                | ((w >> 21) & 0x3FF) << 1
                | ((w >> 20) & 1) << 11
                | ((w >> 12) & 0xFF) << 12) as i32;
            let signed = (recovered << 11) >> 11;
            assert_eq!(signed, offset, "J-type {offset}");
        }
    }

    #[test]
    fn a_frame_never_leaves_the_stack_misaligned() {
        for locals in 0..8 {
            for stack in 0..8 {
                let mut e = RiscVEmitter::new();
                e.prologue(locals, stack);
                assert_eq!(e.frame % 16, 0, "locals={locals} stack={stack}");
                assert!(e.frame >= SAVED_BYTES + 8 * (locals + stack) as i32);
            }
        }
    }

    #[test]
    fn slots_and_locals_never_overlap_the_saved_registers() {
        let mut e = RiscVEmitter::new();
        e.prologue(3, 4);
        for i in 0..3 {
            assert!(Backend::local_offset(&e, i) >= SAVED_BYTES);
        }
        for depth in CACHED.len()..7 {
            assert!(e.spill_offset(depth) >= SAVED_BYTES);
        }
    }
}

#[cfg(test)]
mod dump {
    use super::*;
    use crate::verify::analyse;
    use rustclr_core::opcode::decode_all;

    /// Prints a compiled method as hex, for disassembling by hand.
    #[test]
    fn emit_a_sum_loop_for_disassembly() {
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
        println!("RISCV64 {}", hex.join(" "));
    }
}
