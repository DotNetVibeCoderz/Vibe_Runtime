//! An x86-64 baseline code generator.
//!
//! This is the first tier above interpretation. It takes the methods that are
//! easiest to get right and most worth speeding up — leaf methods doing integer
//! arithmetic over arguments and locals — and turns their IL into machine code.
//! Everything else keeps running in the interpreter, which is the whole point
//! of [`crate::Compiler::can_compile`]: a partial backend is useful on the day
//! it lands rather than when it is finished.
//!
//! # What it compiles
//!
//! Integer arithmetic, comparison, branching, arguments and locals. No calls,
//! no allocation, no exception handling, no floating point, no object access.
//! `can_compile` answers honestly, and a method it declines is not a failure —
//! it is interpreted exactly as before.
//!
//! # How the evaluation stack is handled
//!
//! IL is a stack machine, x86-64 is not. The verifier already computes the
//! evaluation-stack depth at every instruction, so each stack slot has a
//! *statically known* home: the bottom two slots live in `r14` and `r15`, and
//! deeper slots spill to the frame. Most arithmetic runs at depth two or less,
//! so most operations compile to a single register-to-register instruction.
//!
//! # The frame
//!
//! ```text
//!   [rbp - 8]  .. [rbp - 24]              saved rbx, r14, r15
//!   [rbp - 24 - 8*(1 + i)]                local i
//!   [rbp - 24 - 8*(1 + nlocals + i)]      spilled evaluation-stack slot i
//! ```
//!
//! The saved registers come first because the prologue pushes them *after*
//! establishing `rbp`. Starting locals at `[rbp - 8]` would place local 0 on
//! top of saved `rbx` — the compiled method would then return with the
//! caller's registers destroyed, which is exactly the kind of corruption that
//! shows up far from its cause.
//!
//! Arguments arrive as a pointer to an `i64` array — one parameter, so the
//! Windows and System V conventions differ only in which register carries it.
//! The result is returned in `rax` by both.

use crate::codepage::{CodePage, CodePageError};
use crate::translate::{translate, Backend, BinOp, Cond, UnOp};
use crate::verify::MethodAnalysis;
use crate::{analyse, CompileError, CompiledCode, Compiler, Tier};
use rustclr_core::opcode::{decode_all, Instruction, Op};
use rustclr_core::{Loader, MethodId, MethodKind, TypeRegistry};
use std::collections::HashMap;

/// The register holding the argument-array pointer for the life of the body.
///
/// `rbx` is callee-saved under both conventions, so it survives without any
/// further bookkeeping.
const ARGS: u8 = RBX;

const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RBX: u8 = 3;
const RSP: u8 = 4;
const RBP: u8 = 5;
const RDI: u8 = 7;
const R14: u8 = 14;
const R15: u8 = 15;

/// Evaluation-stack slots kept in registers, shallowest first.
const CACHED: [u8; 2] = [R14, R15];

/// Bytes of frame occupied by callee-saved registers pushed after `rbp` is
/// established. Locals and spill slots start below this.
const SAVED_BYTES: i32 = 24;

/// A compiled method, kept alive for as long as the process may call it.
pub struct NativeMethod {
    pub method: MethodId,
    pub arg_count: usize,
    pub returns_value: bool,
    page: CodePage,
}

impl NativeMethod {
    /// Runs the compiled body.
    ///
    /// # Safety
    ///
    /// `args` must have at least [`Self::arg_count`] elements. The emitted code
    /// reads exactly that many and nothing else, and touches no memory it does
    /// not own.
    pub unsafe fn call(&self, args: &[i64]) -> i64 {
        debug_assert!(args.len() >= self.arg_count);
        // SAFETY: `emit` produces a function of exactly this shape — one
        // pointer argument, an integer result in rax — and the page was made
        // executable before this could be constructed.
        let f: extern "C" fn(*const i64) -> i64 =
            unsafe { core::mem::transmute(self.page.as_ptr()) };
        f(args.as_ptr())
    }

    pub fn code_size(&self) -> usize {
        self.page.len()
    }
}

impl core::fmt::Debug for NativeMethod {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NativeMethod")
            .field("method", &self.method)
            .field("code_size", &self.page.len())
            .finish()
    }
}

/// The x86-64 baseline backend.
pub struct X64Backend {
    pub methods_compiled: usize,
    pub bytes_emitted: usize,
    /// Whether small static callees are spliced into their callers.
    ///
    /// Settable so a test can compile the same corpus both ways and compare;
    /// that is the only way to show the inliner is doing anything.
    pub inline: bool,
}

impl Default for X64Backend {
    fn default() -> Self {
        Self { methods_compiled: 0, bytes_emitted: 0, inline: true }
    }
}

impl X64Backend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compiles a method and maps it executable.
    pub fn compile_native(
        &mut self,
        loader: &Loader,
        method: MethodId,
    ) -> Result<NativeMethod, CompileError> {
        let compiled = self.compile(loader, method)?;
        let page = CodePage::commit(&compiled.bytes).map_err(|e: CodePageError| {
            CompileError::Unsupported(format!("executable memory: {e}"))
        })?;
        let info = loader.registry.method(method);
        Ok(NativeMethod {
            method,
            arg_count: info.arg_count(),
            returns_value: !info.returns_void(),
            page,
        })
    }
}

impl Compiler for X64Backend {
    fn name(&self) -> &'static str {
        "x86-64 baseline"
    }

    fn tier(&self) -> Tier {
        Tier::Jit
    }

    fn can_compile(&self, registry: &TypeRegistry, method: MethodId) -> bool {
        // Never on a host that cannot execute what this emits.
        if !cfg!(target_arch = "x86_64") {
            return false;
        }
        if self.inline {
            // With the inliner on, a `call` is no longer disqualifying on its
            // own. This screen still rejects the wrong signatures and any
            // other unsupported opcode; `compile` makes the final decision
            // once it can see the rewritten body.
            return crate::translate::shape_might_compile_after_inlining(registry, method);
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
        let mut instructions = decode_all(&body.il)
            .map_err(|e| CompileError::Unsupported(format!("undecodable IL: {e}")))?;
        let mut locals = body.locals.len();

        // Inline before deciding: a method whose only disqualification is a
        // call to a small helper becomes a leaf once that helper is spliced in,
        // and is then compilable like any other.
        if self.inline {
            if let Some(inlined) = crate::inline::inline_calls(loader, method, &instructions) {
                instructions = inlined.instructions;
                locals += inlined.extra_locals.len();
            }
        }

        // The shape check runs against the *rewritten* body, so an inlined
        // caller is judged on what it actually became.
        if !crate::translate::shape_is_compilable_after_inlining(
            &loader.registry,
            method,
            &instructions,
        ) {
            return Err(CompileError::Unsupported(
                "this method's shape is outside the baseline backend".into(),
            ));
        }

        let returns_value = !info.returns_void();
        // `max_stack` is the declared figure for the original body; splicing
        // can only add to the depth, so the inlined arguments are allowed for.
        let max_stack = body.max_stack.saturating_add(8);
        let analysis = analyse(&instructions, max_stack, &body.exception_clauses, |ins| {
            if ins.op == Op::Ret && returns_value {
                -1
            } else {
                0
            }
        })
        .map_err(CompileError::Invalid)?;

        let bytes = emit(&instructions, &analysis, locals, returns_value)?;
        self.methods_compiled += 1;
        self.bytes_emitted += bytes.len();
        Ok(CompiledCode { method, tier: Tier::Jit, bytes, analysis })
    }
}

/// Opcodes this backend emits code for.
///
/// The list lives with the translation, not here: every backend accepts the
/// same IL subset, and duplicating it per architecture is how they drift.
pub use crate::translate::is_supported;

// -- the assembler ------------------------------------------------------------

/// A growable buffer of machine code with symbolic jump targets.
struct Assembler {
    code: Vec<u8>,
    /// Byte offsets of 32-bit displacements awaiting an IL target offset.
    fixups: Vec<(usize, u32)>,
    /// IL offset to code offset, filled in as instructions are emitted.
    labels: HashMap<u32, usize>,
}

impl Assembler {
    fn new() -> Self {
        Self { code: Vec::with_capacity(256), fixups: Vec::new(), labels: HashMap::new() }
    }

    fn byte(&mut self, b: u8) {
        self.code.push(b);
    }

    fn imm32(&mut self, v: i32) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    fn imm64(&mut self, v: i64) {
        self.code.extend_from_slice(&v.to_le_bytes());
    }

    /// REX prefix. `w` selects 64-bit operand size; `r` and `b` extend the
    /// register fields to reach r8-r15.
    fn rex(&mut self, w: bool, reg: u8, rm: u8) {
        let value = 0x40
            | ((w as u8) << 3)
            | (((reg >> 3) & 1) << 2)
            | ((rm >> 3) & 1);
        if value != 0x40 {
            self.byte(value);
        }
    }

    /// Always emits REX, needed when addressing spl/bpl/sil/dil as bytes.
    fn rex_forced(&mut self, w: bool, reg: u8, rm: u8) {
        self.byte(0x40 | ((w as u8) << 3) | (((reg >> 3) & 1) << 2) | ((rm >> 3) & 1));
    }

    fn modrm(&mut self, mode: u8, reg: u8, rm: u8) {
        self.byte((mode << 6) | ((reg & 7) << 3) | (rm & 7));
    }

    /// `mov dst, src` — 64-bit register to register.
    fn mov_rr(&mut self, dst: u8, src: u8) {
        if dst == src {
            return;
        }
        self.rex(true, src, dst);
        self.byte(0x89);
        self.modrm(0b11, src, dst);
    }

    /// `mov dst, imm64`, narrowing to the shortest encoding that fits.
    fn mov_ri(&mut self, dst: u8, value: i64) {
        if value == 0 {
            // `xor dst, dst` is shorter and breaks the dependency chain.
            self.rex(false, dst, dst);
            self.byte(0x31);
            self.modrm(0b11, dst, dst);
            return;
        }
        if let Ok(v) = i32::try_from(value) {
            if v >= 0 {
                // 32-bit mov zero-extends, which is correct for a positive
                // value and one byte shorter than the sign-extending form.
                self.rex(false, 0, dst);
                self.byte(0xB8 | (dst & 7));
                self.imm32(v);
                return;
            }
            // `mov r64, imm32` sign-extends.
            self.rex(true, 0, dst);
            self.byte(0xC7);
            self.modrm(0b11, 0, dst);
            self.imm32(v);
            return;
        }
        self.rex(true, 0, dst);
        self.byte(0xB8 | (dst & 7));
        self.imm64(value);
    }

    /// `mov dst, [base + disp]`.
    fn mov_r_mem(&mut self, dst: u8, base: u8, disp: i32) {
        self.rex(true, dst, base);
        self.byte(0x8B);
        self.mem_operand(dst, base, disp);
    }

    /// `mov [base + disp], src`.
    fn mov_mem_r(&mut self, base: u8, disp: i32, src: u8) {
        self.rex(true, src, base);
        self.byte(0x89);
        self.mem_operand(src, base, disp);
    }

    /// ModRM plus SIB and displacement for `[base + disp]`.
    fn mem_operand(&mut self, reg: u8, base: u8, disp: i32) {
        let needs_sib = (base & 7) == RSP;
        // rbp and r13 have no zero-displacement form.
        let mode = if disp == 0 && (base & 7) != RBP {
            0b00
        } else if (-128..=127).contains(&disp) {
            0b01
        } else {
            0b10
        };
        self.modrm(mode, reg, if needs_sib { RSP } else { base });
        if needs_sib {
            // scale=0, index=none, base
            self.byte((RSP << 3) | (base & 7));
        }
        match mode {
            0b01 => self.byte(disp as u8),
            0b10 => self.imm32(disp),
            _ => {}
        }
    }

    /// A two-operand ALU instruction: `op dst, src`.
    fn alu_rr(&mut self, opcode: u8, dst: u8, src: u8) {
        self.rex(true, src, dst);
        self.byte(opcode);
        self.modrm(0b11, src, dst);
    }

    fn push(&mut self, reg: u8) {
        if reg >= 8 {
            self.byte(0x41);
        }
        self.byte(0x50 | (reg & 7));
    }

    fn pop(&mut self, reg: u8) {
        if reg >= 8 {
            self.byte(0x41);
        }
        self.byte(0x58 | (reg & 7));
    }

    fn ret(&mut self) {
        self.byte(0xC3);
    }

    /// `jmp rel32`, recorded for later patching.
    fn jmp(&mut self, target: u32) {
        self.byte(0xE9);
        self.fixups.push((self.code.len(), target));
        self.imm32(0);
    }

    /// `jcc rel32`, recorded for later patching.
    fn jcc(&mut self, condition: u8, target: u32) {
        self.byte(0x0F);
        self.byte(0x80 | condition);
        self.fixups.push((self.code.len(), target));
        self.imm32(0);
    }

    fn label(&mut self, il_offset: u32) {
        self.labels.insert(il_offset, self.code.len());
    }

    /// Resolves every recorded jump. A target with no label is a branch to an
    /// offset that is not an instruction boundary, which the verifier should
    /// already have rejected.
    fn finish(mut self) -> Result<Vec<u8>, CompileError> {
        for (at, target) in core::mem::take(&mut self.fixups) {
            let Some(&destination) = self.labels.get(&target) else {
                return Err(CompileError::Unsupported(format!(
                    "branch to IL offset {target:#x}, which is not an instruction boundary"
                )));
            };
            let next = at + 4;
            let relative = destination as i64 - next as i64;
            let relative = i32::try_from(relative).map_err(|_| {
                CompileError::Unsupported("method too large for 32-bit branches".into())
            })?;
            self.code[at..next].copy_from_slice(&relative.to_le_bytes());
        }
        Ok(self.code)
    }
}

// Condition codes, as the low nibble of a `jcc` opcode.
const CC_E: u8 = 0x4;
const CC_NE: u8 = 0x5;
const CC_L: u8 = 0xC;
const CC_GE: u8 = 0xD;
const CC_LE: u8 = 0xE;
const CC_G: u8 = 0xF;
const CC_B: u8 = 0x2;
const CC_AE: u8 = 0x3;
const CC_BE: u8 = 0x6;
const CC_A: u8 = 0x7;

// -- code generation ----------------------------------------------------------

/// The x86-64 side of [`translate`]: register allocation is fixed, so all this
/// does is encode.
///
/// `rax`, `rcx` and `rdx` are the scratch registers, chosen to suit the two
/// instructions that dictate register use on this architecture: `idiv` divides
/// `rdx:rax` and leaves the quotient in `rax`, and a variable shift takes its
/// count in `cl`. Handing the walk `t0 = rax` and `t1 = rcx` makes both fall
/// out without a move.
pub struct X64Emitter {
    asm: Assembler,
    locals: usize,
    /// Bytes of frame reserved below `rbp`.
    frame: i32,
}

impl X64Emitter {
    fn new() -> Self {
        Self { asm: Assembler::new(), locals: 0, frame: 0 }
    }

    fn finish(self) -> Result<Vec<u8>, CompileError> {
        self.asm.finish()
    }
}

impl Backend for X64Emitter {
    fn temp(&self, which: usize) -> u8 {
        match which {
            0 => RAX,
            1 => RCX,
            _ => RDX,
        }
    }

    fn cached_slot_register(&self, depth: usize) -> Option<u8> {
        CACHED.get(depth).copied()
    }

    fn local_offset(&self, index: usize) -> i32 {
        -SAVED_BYTES - 8 * (1 + index as i32)
    }

    fn spill_offset(&self, depth: usize) -> i32 {
        -SAVED_BYTES - 8 * (1 + self.locals as i32 + depth as i32)
    }

    fn prologue(&mut self, locals: usize, max_stack: usize) {
        self.locals = locals;

        self.asm.push(RBP);
        self.asm.mov_rr(RBP, RSP);
        self.asm.push(RBX);
        self.asm.push(R14);
        self.asm.push(R15);

        // Reserve locals and spill slots, keeping rsp 16-byte aligned. The
        // return address plus `push rbp` and three more pushes leave the stack
        // misaligned by 8, so an odd number of reserved words restores it.
        let words = locals + max_stack + 2;
        let words = if words % 2 == 0 { words + 1 } else { words };
        self.frame = (words * 8) as i32;
        // sub rsp, imm32
        self.asm.rex(true, 0, RSP);
        self.asm.byte(0x81);
        self.asm.modrm(0b11, 5, RSP);
        self.asm.imm32(self.frame);

        // The single incoming parameter: rcx on Windows, rdi on System V.
        let incoming = if cfg!(windows) { RCX } else { RDI };
        self.asm.mov_rr(ARGS, incoming);

        // `init_locals` semantics: every local starts at zero.
        if locals > 0 {
            self.asm.mov_ri(RAX, 0);
            for i in 0..locals {
                let off = Backend::local_offset(self, i);
                self.asm.mov_mem_r(RBP, off, RAX);
            }
        }
    }

    fn epilogue(&mut self) {
        // add rsp, frame
        self.asm.rex(true, 0, RSP);
        self.asm.byte(0x81);
        self.asm.modrm(0b11, 0, RSP);
        self.asm.imm32(self.frame);
        self.asm.pop(R15);
        self.asm.pop(R14);
        self.asm.pop(RBX);
        self.asm.pop(RBP);
        self.asm.ret();
    }

    fn mov_reg(&mut self, dst: u8, src: u8) {
        self.asm.mov_rr(dst, src);
    }

    fn load_imm(&mut self, dst: u8, value: i64) {
        self.asm.mov_ri(dst, value);
    }

    fn load_frame(&mut self, dst: u8, offset: i32) {
        self.asm.mov_r_mem(dst, RBP, offset);
    }

    fn store_frame(&mut self, offset: i32, src: u8) {
        self.asm.mov_mem_r(RBP, offset, src);
    }

    fn load_arg(&mut self, dst: u8, index: usize) {
        self.asm.mov_r_mem(dst, ARGS, (index * 8) as i32);
    }

    fn store_arg(&mut self, index: usize, src: u8) {
        self.asm.mov_mem_r(ARGS, (index * 8) as i32, src);
    }

    fn binop(&mut self, op: BinOp, dst: u8, lhs: u8, rhs: u8) {
        // The walk always asks for `dst == lhs`, which is what a two-address
        // architecture wants; anything else would need a move first.
        debug_assert_eq!(dst, lhs, "x86-64 computes in place");

        match op {
            BinOp::Add => self.asm.alu_rr(0x01, dst, rhs),
            BinOp::Sub => self.asm.alu_rr(0x29, dst, rhs),
            BinOp::And => self.asm.alu_rr(0x21, dst, rhs),
            BinOp::Or => self.asm.alu_rr(0x09, dst, rhs),
            BinOp::Xor => self.asm.alu_rr(0x31, dst, rhs),
            BinOp::Mul => {
                // imul dst, rhs
                self.asm.rex(true, dst, rhs);
                self.asm.byte(0x0F);
                self.asm.byte(0xAF);
                self.asm.modrm(0b11, dst, rhs);
            }
            BinOp::Div | BinOp::Rem => {
                // idiv divides rdx:rax, so the dividend has to be in rax and
                // rdx has to hold its sign extension.
                self.asm.mov_rr(RAX, dst);
                // cqo
                self.asm.byte(0x48);
                self.asm.byte(0x99);
                // idiv rhs
                self.asm.rex(true, 0, rhs);
                self.asm.byte(0xF7);
                self.asm.modrm(0b11, 7, rhs);
                let result = if op == BinOp::Rem { RDX } else { RAX };
                self.asm.mov_rr(dst, result);
            }
            BinOp::Shl | BinOp::Shr | BinOp::ShrUn => {
                // A variable shift takes its count in cl and nowhere else.
                self.asm.mov_rr(RCX, rhs);
                let extension = match op {
                    BinOp::Shl => 4,
                    BinOp::Shr => 7, // sar, arithmetic
                    _ => 5,          // shr, logical
                };
                self.asm.rex(true, 0, dst);
                self.asm.byte(0xD3);
                self.asm.modrm(0b11, extension, dst);
            }
        }
    }

    fn unop(&mut self, op: UnOp, dst: u8, src: u8) {
        self.asm.mov_rr(dst, src);
        match op {
            UnOp::Neg => {
                self.asm.rex(true, 0, dst);
                self.asm.byte(0xF7);
                self.asm.modrm(0b11, 3, dst);
            }
            UnOp::Not => {
                self.asm.rex(true, 0, dst);
                self.asm.byte(0xF7);
                self.asm.modrm(0b11, 2, dst);
            }
            UnOp::SignExtend32 => {
                // movsxd dst, dst32
                self.asm.rex(true, dst, dst);
                self.asm.byte(0x63);
                self.asm.modrm(0b11, dst, dst);
            }
        }
    }

    fn compare(&mut self, cond: Cond, dst: u8, lhs: u8, rhs: u8) {
        // cmp lhs, rhs
        self.asm.alu_rr(0x39, lhs, rhs);
        // setcc dst8 — REX is forced so the byte register is the low byte and
        // not `ah`/`ch`/`dh`.
        self.asm.rex_forced(false, 0, dst);
        self.asm.byte(0x0F);
        self.asm.byte(0x90 | condition_code(cond));
        self.asm.modrm(0b11, 0, dst);
        // movzx dst, dst8
        self.asm.rex_forced(true, dst, dst);
        self.asm.byte(0x0F);
        self.asm.byte(0xB6);
        self.asm.modrm(0b11, dst, dst);
    }

    fn branch(&mut self, target: u32) {
        self.asm.jmp(target);
    }

    fn branch_compare(&mut self, cond: Cond, lhs: u8, rhs: u8, target: u32) {
        self.asm.alu_rr(0x39, lhs, rhs);
        self.asm.jcc(condition_code(cond), target);
    }

    fn branch_zero(&mut self, src: u8, non_zero: bool, target: u32) {
        // test src, src
        self.asm.alu_rr(0x85, src, src);
        self.asm.jcc(if non_zero { CC_NE } else { CC_E }, target);
    }

    fn ret(&mut self, src: u8) {
        self.asm.mov_rr(RAX, src);
        self.epilogue();
    }

    fn ret_void(&mut self) {
        self.asm.mov_ri(RAX, 0);
        self.epilogue();
    }

    fn label(&mut self, il_offset: u32) {
        self.asm.label(il_offset);
    }
}

/// The low nibble of a `jcc`/`setcc` opcode for a condition.
fn condition_code(cond: Cond) -> u8 {
    match cond {
        Cond::Equal => CC_E,
        Cond::NotEqual => CC_NE,
        Cond::Less => CC_L,
        Cond::LessOrEqual => CC_LE,
        Cond::Greater => CC_G,
        Cond::GreaterOrEqual => CC_GE,
        Cond::Below => CC_B,
        Cond::BelowOrEqual => CC_BE,
        Cond::Above => CC_A,
        Cond::AboveOrEqual => CC_AE,
    }
}

/// Translates a verified method body into x86-64 machine code.
fn emit(
    instructions: &[Instruction],
    analysis: &MethodAnalysis,
    locals: usize,
    returns_value: bool,
) -> Result<Vec<u8>, CompileError> {
    let mut e = X64Emitter::new();
    translate(&mut e, instructions, analysis, locals, returns_value)?;
    e.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_x86_64_hosts_advertise_this_backend() {
        // The guard is compile-time, so this documents the intent rather than
        // exercising both paths.
        assert_eq!(cfg!(target_arch = "x86_64"), cfg!(target_arch = "x86_64"));
    }

    #[test]
    fn the_register_cache_covers_the_shallowest_slots() {
        assert_eq!(CACHED.len(), 2, "deeper slots spill to the frame");
    }

    #[test]
    fn frame_slots_never_overlap_the_saved_registers() {
        let mut e = X64Emitter::new();
        e.prologue(3, 4);

        // Three registers are pushed after `rbp`, occupying [rbp-8..rbp-24].
        // Anything the body writes must sit strictly below them, or a compiled
        // method returns having destroyed its caller's registers — which is a
        // bug that shows up in the *caller*, far from its cause.
        for i in 0..3 {
            let off = Backend::local_offset(&e, i);
            assert!(off <= -SAVED_BYTES - 8, "local {i} at {off} overlaps the saved registers");
        }
        for depth in CACHED.len()..6 {
            if e.cached_slot_register(depth).is_some() {
                continue;
            }
            let off = e.spill_offset(depth);
            assert!(off <= -SAVED_BYTES - 8, "spill slot {depth} at {off} overlaps");
        }
    }
}

#[cfg(all(test, target_arch = "x86_64", any(windows, unix)))]
mod machine_tests {
    use super::*;
    use crate::verify::analyse;
    use rustclr_core::opcode::decode_all;

    /// Assembles a method body directly, bypassing the loader, and runs it.
    fn run(il: &[u8], locals: usize, args: &[i64]) -> i64 {
        let instructions = decode_all(il).expect("decode");
        let analysis = analyse(&instructions, 8, &[], |ins| {
            if ins.op == Op::Ret { -1 } else { 0 }
        })
        .expect("verify");
        let code = emit(&instructions, &analysis, locals, true).expect("emit");
        let page = CodePage::commit(&code).expect("commit");
        // SAFETY: `emit` produced this page and the signature matches.
        let f: extern "C" fn(*const i64) -> i64 =
            unsafe { core::mem::transmute(page.as_ptr()) };
        f(args.as_ptr())
    }

    #[test]
    fn adds_two_arguments() {
        // ldarg.0; ldarg.1; add; ret
        assert_eq!(run(&[0x02, 0x03, 0x58, 0x2A], 0, &[20, 22]), 42);
    }

    #[test]
    fn arithmetic_matches_the_interpreter() {
        // ldarg.0; ldarg.1; mul; ldarg.0; sub; ret   =>  a*b - a
        assert_eq!(run(&[0x02, 0x03, 0x5A, 0x02, 0x59, 0x2A], 0, &[7, 6]), 35);
        // ldarg.0; ldarg.1; div; ret
        assert_eq!(run(&[0x02, 0x03, 0x5B, 0x2A], 0, &[-17, 5]), -3);
        // ldarg.0; ldarg.1; rem; ret
        assert_eq!(run(&[0x02, 0x03, 0x5D, 0x2A], 0, &[-17, 5]), -2);
    }

    #[test]
    fn constants_use_the_short_encodings() {
        // ldc.i4.8; ldc.i4.s 34; add; ret
        assert_eq!(run(&[0x1E, 0x1F, 34, 0x58, 0x2A], 0, &[]), 42);
        // ldc.i4 1000000; ret
        assert_eq!(run(&[0x20, 0x40, 0x42, 0x0F, 0x00, 0x2A], 0, &[]), 1_000_000);
        // ldc.i4.m1; ret — negative immediates must sign-extend
        assert_eq!(run(&[0x15, 0x2A], 0, &[]), -1);
    }

    #[test]
    fn comparisons_produce_zero_or_one() {
        // ldarg.0; ldarg.1; clt; ret
        assert_eq!(run(&[0x02, 0x03, 0xFE, 0x04, 0x2A], 0, &[3, 9]), 1);
        assert_eq!(run(&[0x02, 0x03, 0xFE, 0x04, 0x2A], 0, &[9, 3]), 0);
        // ldarg.0; ldarg.1; ceq; ret
        assert_eq!(run(&[0x02, 0x03, 0xFE, 0x01, 0x2A], 0, &[5, 5]), 1);
    }

    #[test]
    fn a_loop_runs_to_completion() {
        // Sums 1..=n into local 0.
        //
        //   ldc.i4.0; stloc.0            total = 0
        //   ldc.i4.1; stloc.1            i = 1
        // loop:
        //   ldloc.1; ldarg.0; bgt.s end
        //   ldloc.0; ldloc.1; add; stloc.0
        //   ldloc.1; ldc.i4.1; add; stloc.1
        //   br.s loop
        // end:
        //   ldloc.0; ret
        let il = [
            // 0
            0x16, 0x0A, // ldc.i4.0; stloc.0        total = 0
            0x17, 0x0B, // ldc.i4.1; stloc.1        i = 1
            // 4: loop head
            0x07, 0x02, 0x30, 0x0A, // ldloc.1; ldarg.0; bgt.s -> 18
            // 8
            0x06, 0x07, 0x58, 0x0A, // ldloc.0; ldloc.1; add; stloc.0
            // 12
            0x07, 0x17, 0x58, 0x0B, // ldloc.1; ldc.i4.1; add; stloc.1
            // 16
            0x2B, 0xF2, // br.s -> 4
            // 18
            0x06, 0x2A, // ldloc.0; ret
        ];
        assert_eq!(run(&il, 2, &[10]), 55);
        assert_eq!(run(&il, 2, &[100]), 5050);
        assert_eq!(run(&il, 2, &[0]), 0, "an empty range sums to zero");
    }

    #[test]
    fn deep_stacks_spill_correctly() {
        // Four values on the stack at once, so slots 2 and 3 must spill:
        // ldc.i4.1; ldc.i4.2; ldc.i4.3; ldc.i4.4; add; add; add; ret  => 10
        assert_eq!(run(&[0x17, 0x18, 0x19, 0x1A, 0x58, 0x58, 0x58, 0x2A], 0, &[]), 10);
    }

    #[test]
    fn shifts_and_bitwise_operations_agree_with_csharp() {
        // ldarg.0; ldarg.1; shl; ret
        assert_eq!(run(&[0x02, 0x03, 0x62, 0x2A], 0, &[1, 10]), 1024);
        // ldarg.0; ldarg.1; and; ret
        assert_eq!(run(&[0x02, 0x03, 0x5F, 0x2A], 0, &[0xF0, 0x3C]), 0x30);
        // ldarg.0; not; ret
        assert_eq!(run(&[0x02, 0x66, 0x2A], 0, &[0]), -1);
        // ldarg.0; neg; ret
        assert_eq!(run(&[0x02, 0x65, 0x2A], 0, &[42]), -42);
    }
}
