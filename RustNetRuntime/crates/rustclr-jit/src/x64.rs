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
use crate::verify::MethodAnalysis;
use crate::{analyse, CompileError, CompiledCode, Compiler, Tier};
use rustclr_core::opcode::{decode_all, Instruction, Op, Operand};
use rustclr_core::{Loader, MethodId, MethodKind, TypeRegistry};
use rustclr_metadata::TypeSig;
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
#[derive(Default)]
pub struct X64Backend {
    pub methods_compiled: usize,
    pub bytes_emitted: usize,
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
        if !cfg!(target_arch = "x86_64") {
            return false;
        }
        let info = registry.method(method);
        let MethodKind::Il(body) = &info.kind else { return false };
        if !body.exception_clauses.is_empty() {
            return false;
        }
        // Only integer-shaped signatures: every argument, local and the result
        // must fit an `i64` register.
        if !integer_shaped(&info.signature.return_type) && !info.returns_void() {
            return false;
        }
        if !info.signature.params.iter().all(integer_shaped) {
            return false;
        }
        if info.signature.has_this {
            return false;
        }
        if !body.locals.iter().all(integer_shaped) {
            return false;
        }
        let Ok(instructions) = decode_all(&body.il) else { return false };
        instructions.iter().all(|i| is_supported(i.op))
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
        if !self.can_compile(&loader.registry, method) {
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

/// Whether a signature type occupies one integer register.
fn integer_shaped(sig: &TypeSig) -> bool {
    matches!(
        sig.unwrap_modifiers(),
        TypeSig::Boolean
            | TypeSig::Char
            | TypeSig::I1
            | TypeSig::U1
            | TypeSig::I2
            | TypeSig::U2
            | TypeSig::I4
            | TypeSig::U4
            | TypeSig::I8
            | TypeSig::U8
    )
}

/// Opcodes the baseline backend emits code for.
///
/// Public so `rustnet jit` can say which instruction turned a method down.
pub fn is_supported(op: Op) -> bool {
    use Op::*;
    matches!(
        op,
        Nop | Ldarg0
            | Ldarg1
            | Ldarg2
            | Ldarg3
            | LdargS
            | Ldarg
            | StargS
            | Starg
            | Ldloc0
            | Ldloc1
            | Ldloc2
            | Ldloc3
            | LdlocS
            | Ldloc
            | Stloc0
            | Stloc1
            | Stloc2
            | Stloc3
            | StlocS
            | Stloc
            | LdcI4M1
            | LdcI40
            | LdcI41
            | LdcI42
            | LdcI43
            | LdcI44
            | LdcI45
            | LdcI46
            | LdcI47
            | LdcI48
            | LdcI4S
            | LdcI4
            | LdcI8
            | Add
            | Sub
            | Mul
            | Div
            | Rem
            | And
            | Or
            | Xor
            | Shl
            | Shr
            | ShrUn
            | Neg
            | Not
            | Dup
            | Pop
            | Ceq
            | Cgt
            | CgtUn
            | Clt
            | CltUn
            | Br
            | BrS
            | Brtrue
            | BrtrueS
            | Brfalse
            | BrfalseS
            | Beq
            | BeqS
            | BneUn
            | BneUnS
            | Bge
            | BgeS
            | Bgt
            | BgtS
            | Ble
            | BleS
            | Blt
            | BltS
            | BgeUn
            | BgeUnS
            | BgtUn
            | BgtUnS
            | BleUn
            | BleUnS
            | BltUn
            | BltUnS
            | ConvI4
            | ConvI8
            | ConvI
            | Ret
    )
}

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

/// Where an evaluation-stack slot lives.
enum Slot {
    Register(u8),
    /// Frame offset from `rbp`.
    Spilled(i32),
}

struct Emitter<'a> {
    asm: Assembler,
    analysis: &'a MethodAnalysis,
    locals: usize,
    returns_value: bool,
    /// Bytes of frame reserved below `rbp`.
    frame: i32,
}

impl<'a> Emitter<'a> {
    fn local_offset(&self, index: usize) -> i32 {
        -SAVED_BYTES - 8 * (1 + index as i32)
    }

    fn slot(&self, depth: usize) -> Slot {
        match CACHED.get(depth) {
            Some(&r) => Slot::Register(r),
            None => Slot::Spilled(
                -SAVED_BYTES - 8 * (1 + self.locals as i32 + depth as i32),
            ),
        }
    }

    /// Loads evaluation-stack slot `depth` into `dst`.
    fn load_slot(&mut self, dst: u8, depth: usize) {
        match self.slot(depth) {
            Slot::Register(r) => self.asm.mov_rr(dst, r),
            Slot::Spilled(off) => self.asm.mov_r_mem(dst, RBP, off),
        }
    }

    /// Stores `src` into evaluation-stack slot `depth`.
    fn store_slot(&mut self, depth: usize, src: u8) {
        match self.slot(depth) {
            Slot::Register(r) => self.asm.mov_rr(r, src),
            Slot::Spilled(off) => self.asm.mov_mem_r(RBP, off, src),
        }
    }

    /// The prologue: standard frame, callee-saved registers, argument pointer.
    fn prologue(&mut self) {
        self.asm.push(RBP);
        self.asm.mov_rr(RBP, RSP);
        self.asm.push(RBX);
        self.asm.push(R14);
        self.asm.push(R15);

        // Reserve locals and spill slots, keeping rsp 16-byte aligned. The
        // return address plus `push rbp` and three more pushes leave the stack
        // misaligned by 8, so an odd number of reserved words restores it.
        let words = self.locals + self.analysis.max_stack_observed as usize + 2;
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
        if self.locals > 0 {
            self.asm.mov_ri(RAX, 0);
            for i in 0..self.locals {
                let off = self.local_offset(i);
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

    /// Materialises a comparison as 0 or 1 in `rax`.
    fn compare(&mut self, condition: u8, depth: usize) {
        // The operands are at depth-2 and depth-1.
        self.load_slot(RAX, depth - 2);
        self.load_slot(RCX, depth - 1);
        // cmp rax, rcx
        self.asm.alu_rr(0x39, RAX, RCX);
        // setcc al — REX is forced so the byte register is `al`, not `ah`.
        self.asm.byte(0x0F);
        self.asm.byte(0x90 | condition);
        self.asm.modrm(0b11, 0, RAX);
        // movzx rax, al
        self.asm.rex_forced(true, RAX, RAX);
        self.asm.byte(0x0F);
        self.asm.byte(0xB6);
        self.asm.modrm(0b11, RAX, RAX);
        self.store_slot(depth - 2, RAX);
    }

    /// Signed division or remainder, which on x86 both come from `idiv`.
    fn divide(&mut self, want_remainder: bool, depth: usize) {
        self.load_slot(RAX, depth - 2);
        self.load_slot(RCX, depth - 1);
        // cqo — sign-extend rax into rdx, which idiv divides as rdx:rax.
        self.asm.byte(0x48);
        self.asm.byte(0x99);
        // idiv rcx
        self.asm.rex(true, 0, RCX);
        self.asm.byte(0xF7);
        self.asm.modrm(0b11, 7, RCX);
        self.store_slot(depth - 2, if want_remainder { RDX } else { RAX });
    }
}

/// Translates a verified method body into machine code.
fn emit(
    instructions: &[Instruction],
    analysis: &MethodAnalysis,
    locals: usize,
    returns_value: bool,
) -> Result<Vec<u8>, CompileError> {
    let mut e = Emitter {
        asm: Assembler::new(),
        analysis,
        locals,
        returns_value,
        frame: 0,
    };
    e.prologue();

    for ins in instructions {
        e.asm.label(ins.offset);
        let depth = *analysis.depth_at.get(&ins.offset).unwrap_or(&0);
        if depth < 0 {
            return Err(CompileError::Unsupported(
                "negative evaluation-stack depth".into(),
            ));
        }
        let depth = depth as usize;
        emit_one(&mut e, ins, depth)?;
    }

    // A body that falls off the end without `ret` is invalid IL, but emitting a
    // return keeps the page well-formed rather than running into whatever
    // follows it in memory.
    e.asm.mov_ri(RAX, 0);
    e.epilogue();
    e.asm.finish()
}

fn emit_one(e: &mut Emitter, ins: &Instruction, depth: usize) -> Result<(), CompileError> {
    use Op::*;

    /// The ALU opcode for a two-operand integer instruction.
    fn alu_opcode(op: Op) -> Option<u8> {
        Some(match op {
            Add => 0x01,
            Sub => 0x29,
            And => 0x21,
            Or => 0x09,
            Xor => 0x31,
            _ => return None,
        })
    }

    match ins.op {
        Nop => {}

        Ldarg0 | Ldarg1 | Ldarg2 | Ldarg3 | LdargS | Ldarg => {
            let index = match ins.op {
                Ldarg0 => 0,
                Ldarg1 => 1,
                Ldarg2 => 2,
                Ldarg3 => 3,
                _ => ins.operand.as_var().unwrap_or(0) as usize,
            };
            e.asm.mov_r_mem(RAX, ARGS, (index * 8) as i32);
            e.store_slot(depth, RAX);
        }

        Ldloc0 | Ldloc1 | Ldloc2 | Ldloc3 | LdlocS | Ldloc => {
            let index = match ins.op {
                Ldloc0 => 0,
                Ldloc1 => 1,
                Ldloc2 => 2,
                Ldloc3 => 3,
                _ => ins.operand.as_var().unwrap_or(0) as usize,
            };
            let off = e.local_offset(index);
            e.asm.mov_r_mem(RAX, RBP, off);
            e.store_slot(depth, RAX);
        }

        // `starg` assigns to a parameter. The argument array is the callee's
        // own copy — the caller marshalled it for this call — so writing to it
        // has exactly the local effect C# gives an assignment to a parameter.
        StargS | Starg => {
            let index = ins.operand.as_var().unwrap_or(0) as usize;
            e.load_slot(RAX, depth - 1);
            e.asm.mov_mem_r(ARGS, (index * 8) as i32, RAX);
        }

        Stloc0 | Stloc1 | Stloc2 | Stloc3 | StlocS | Stloc => {
            let index = match ins.op {
                Stloc0 => 0,
                Stloc1 => 1,
                Stloc2 => 2,
                Stloc3 => 3,
                _ => ins.operand.as_var().unwrap_or(0) as usize,
            };
            e.load_slot(RAX, depth - 1);
            let off = e.local_offset(index);
            e.asm.mov_mem_r(RBP, off, RAX);
        }

        LdcI4M1 | LdcI40 | LdcI41 | LdcI42 | LdcI43 | LdcI44 | LdcI45 | LdcI46 | LdcI47
        | LdcI48 | LdcI4S | LdcI4 => {
            let value = match ins.op {
                LdcI4M1 => -1,
                LdcI40 => 0,
                LdcI41 => 1,
                LdcI42 => 2,
                LdcI43 => 3,
                LdcI44 => 4,
                LdcI45 => 5,
                LdcI46 => 6,
                LdcI47 => 7,
                LdcI48 => 8,
                _ => ins.operand.as_i32().unwrap_or(0),
            };
            e.asm.mov_ri(RAX, value as i64);
            e.store_slot(depth, RAX);
        }

        LdcI8 => {
            let Operand::I64(v) = ins.operand else {
                return Err(CompileError::Unsupported("ldc.i8 without an operand".into()));
            };
            e.asm.mov_ri(RAX, v);
            e.store_slot(depth, RAX);
        }

        Add | Sub | And | Or | Xor => {
            let opcode = alu_opcode(ins.op).expect("matched above");
            e.load_slot(RAX, depth - 2);
            e.load_slot(RCX, depth - 1);
            e.asm.alu_rr(opcode, RAX, RCX);
            e.store_slot(depth - 2, RAX);
        }

        Mul => {
            e.load_slot(RAX, depth - 2);
            e.load_slot(RCX, depth - 1);
            // imul rax, rcx
            e.asm.rex(true, RAX, RCX);
            e.asm.byte(0x0F);
            e.asm.byte(0xAF);
            e.asm.modrm(0b11, RAX, RCX);
            e.store_slot(depth - 2, RAX);
        }

        Div => e.divide(false, depth),
        Rem => e.divide(true, depth),

        Shl | Shr | ShrUn => {
            e.load_slot(RAX, depth - 2);
            e.load_slot(RCX, depth - 1);
            // The shift count must be in cl.
            let extension = match ins.op {
                Shl => 4,
                Shr => 7,  // sar, arithmetic
                _ => 5,    // shr, logical
            };
            e.asm.rex(true, 0, RAX);
            e.asm.byte(0xD3);
            e.asm.modrm(0b11, extension, RAX);
            e.store_slot(depth - 2, RAX);
        }

        Neg => {
            e.load_slot(RAX, depth - 1);
            e.asm.rex(true, 0, RAX);
            e.asm.byte(0xF7);
            e.asm.modrm(0b11, 3, RAX);
            e.store_slot(depth - 1, RAX);
        }

        Not => {
            e.load_slot(RAX, depth - 1);
            e.asm.rex(true, 0, RAX);
            e.asm.byte(0xF7);
            e.asm.modrm(0b11, 2, RAX);
            e.store_slot(depth - 1, RAX);
        }

        Dup => {
            e.load_slot(RAX, depth - 1);
            e.store_slot(depth, RAX);
        }

        Pop => {}

        // `conv.i4` truncates to 32 bits and sign-extends back, which is what
        // the evaluation stack's int32 type means here.
        ConvI4 => {
            e.load_slot(RAX, depth - 1);
            // movsxd rax, eax
            e.asm.rex(true, RAX, RAX);
            e.asm.byte(0x63);
            e.asm.modrm(0b11, RAX, RAX);
            e.store_slot(depth - 1, RAX);
        }
        ConvI8 | ConvI => {}

        Ceq => e.compare(CC_E, depth),
        Cgt => e.compare(CC_G, depth),
        CgtUn => e.compare(CC_A, depth),
        Clt => e.compare(CC_L, depth),
        CltUn => e.compare(CC_B, depth),

        Br | BrS => {
            let target = branch_target(ins)?;
            e.asm.jmp(target);
        }

        Brtrue | BrtrueS | Brfalse | BrfalseS => {
            let target = branch_target(ins)?;
            e.load_slot(RAX, depth - 1);
            // test rax, rax
            e.asm.alu_rr(0x85, RAX, RAX);
            let condition = if matches!(ins.op, Brtrue | BrtrueS) { CC_NE } else { CC_E };
            e.asm.jcc(condition, target);
        }

        Beq | BeqS | BneUn | BneUnS | Bge | BgeS | Bgt | BgtS | Ble | BleS | Blt | BltS
        | BgeUn | BgeUnS | BgtUn | BgtUnS | BleUn | BleUnS | BltUn | BltUnS => {
            let target = branch_target(ins)?;
            e.load_slot(RAX, depth - 2);
            e.load_slot(RCX, depth - 1);
            e.asm.alu_rr(0x39, RAX, RCX);
            // The `.un` forms are unsigned comparisons, except `bne.un`, which
            // is plain inequality — there is no ordering to interpret.
            let condition = match ins.op {
                Beq | BeqS => CC_E,
                BneUn | BneUnS => CC_NE,
                Bge | BgeS => CC_GE,
                Bgt | BgtS => CC_G,
                Ble | BleS => CC_LE,
                Blt | BltS => CC_L,
                BgeUn | BgeUnS => CC_AE,
                BgtUn | BgtUnS => CC_A,
                BleUn | BleUnS => CC_BE,
                _ => CC_B,
            };
            e.asm.jcc(condition, target);
        }

        Ret => {
            if e.returns_value {
                e.load_slot(RAX, depth - 1);
            } else {
                e.asm.mov_ri(RAX, 0);
            }
            e.epilogue();
        }

        other => {
            return Err(CompileError::Unsupported(format!("{other:?}")));
        }
    }
    Ok(())
}

fn branch_target(ins: &Instruction) -> Result<u32, CompileError> {
    ins.operand
        .as_target()
        .ok_or_else(|| CompileError::Unsupported("branch without a target".into()))
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
        let analysis = MethodAnalysis { max_stack_observed: 4, ..Default::default() };
        let e = Emitter {
            asm: Assembler::new(),
            analysis: &analysis,
            locals: 3,
            returns_value: true,
            frame: 0,
        };
        // Three registers are pushed after `rbp`, occupying [rbp-8..rbp-24].
        // Anything the body writes must sit strictly below them, or a compiled
        // method returns having destroyed its caller's registers.
        for i in 0..3 {
            assert!(
                e.local_offset(i) <= -SAVED_BYTES - 8,
                "local {i} at {} overlaps the saved registers",
                e.local_offset(i)
            );
        }
        for depth in CACHED.len()..6 {
            let Slot::Spilled(off) = e.slot(depth) else { continue };
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
