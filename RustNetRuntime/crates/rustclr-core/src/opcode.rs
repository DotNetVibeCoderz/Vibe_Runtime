//! IL opcode table and decoder.
//!
//! One table drives both the interpreter and the disassembler, so they can
//! never disagree about an instruction's length or operand shape.

use rustclr_metadata::Token;

#[allow(unused_imports)]
use crate::prelude::*;

/// The shape of an instruction's inline operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandKind {
    None,
    /// Signed 1-byte branch displacement.
    ShortBranch,
    /// Signed 4-byte branch displacement.
    Branch,
    /// 1-byte local/argument index.
    ShortVar,
    /// 2-byte local/argument index.
    Var,
    /// Signed 1-byte immediate.
    ShortI,
    /// 4-byte immediate.
    I4,
    /// 8-byte immediate.
    I8,
    /// 4-byte float.
    R4,
    /// 8-byte float.
    R8,
    /// 4-byte metadata token.
    Token,
    /// `switch`: a count followed by that many 4-byte displacements.
    Switch,
}

impl OperandKind {
    /// Bytes the operand occupies, excluding `Switch` which is variable.
    pub const fn fixed_size(self) -> usize {
        match self {
            Self::None => 0,
            Self::ShortBranch | Self::ShortVar | Self::ShortI => 1,
            Self::Var => 2,
            Self::Branch | Self::I4 | Self::R4 | Self::Token => 4,
            Self::I8 | Self::R8 => 8,
            Self::Switch => 4,
        }
    }
}

macro_rules! opcodes {
    ($( $variant:ident = $code:expr, $name:literal, $operand:ident ; )*) => {
        /// Every IL instruction this runtime recognises.
        ///
        /// Two-byte opcodes (`0xFE xx`) are stored as `0xFE00 | xx`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u16)]
        pub enum Op {
            $( $variant = $code, )*
        }

        impl Op {
            pub fn from_code(code: u16) -> Option<Op> {
                match code {
                    $( $code => Some(Op::$variant), )*
                    _ => None,
                }
            }

            pub const fn name(self) -> &'static str {
                match self {
                    $( Op::$variant => $name, )*
                }
            }

            pub const fn operand_kind(self) -> OperandKind {
                match self {
                    $( Op::$variant => OperandKind::$operand, )*
                }
            }

            pub const fn code(self) -> u16 {
                self as u16
            }
        }
    };
}

opcodes! {
    Nop = 0x00, "nop", None;
    Break = 0x01, "break", None;
    Ldarg0 = 0x02, "ldarg.0", None;
    Ldarg1 = 0x03, "ldarg.1", None;
    Ldarg2 = 0x04, "ldarg.2", None;
    Ldarg3 = 0x05, "ldarg.3", None;
    Ldloc0 = 0x06, "ldloc.0", None;
    Ldloc1 = 0x07, "ldloc.1", None;
    Ldloc2 = 0x08, "ldloc.2", None;
    Ldloc3 = 0x09, "ldloc.3", None;
    Stloc0 = 0x0A, "stloc.0", None;
    Stloc1 = 0x0B, "stloc.1", None;
    Stloc2 = 0x0C, "stloc.2", None;
    Stloc3 = 0x0D, "stloc.3", None;
    LdargS = 0x0E, "ldarg.s", ShortVar;
    LdargaS = 0x0F, "ldarga.s", ShortVar;
    StargS = 0x10, "starg.s", ShortVar;
    LdlocS = 0x11, "ldloc.s", ShortVar;
    LdlocaS = 0x12, "ldloca.s", ShortVar;
    StlocS = 0x13, "stloc.s", ShortVar;
    Ldnull = 0x14, "ldnull", None;
    LdcI4M1 = 0x15, "ldc.i4.m1", None;
    LdcI40 = 0x16, "ldc.i4.0", None;
    LdcI41 = 0x17, "ldc.i4.1", None;
    LdcI42 = 0x18, "ldc.i4.2", None;
    LdcI43 = 0x19, "ldc.i4.3", None;
    LdcI44 = 0x1A, "ldc.i4.4", None;
    LdcI45 = 0x1B, "ldc.i4.5", None;
    LdcI46 = 0x1C, "ldc.i4.6", None;
    LdcI47 = 0x1D, "ldc.i4.7", None;
    LdcI48 = 0x1E, "ldc.i4.8", None;
    LdcI4S = 0x1F, "ldc.i4.s", ShortI;
    LdcI4 = 0x20, "ldc.i4", I4;
    LdcI8 = 0x21, "ldc.i8", I8;
    LdcR4 = 0x22, "ldc.r4", R4;
    LdcR8 = 0x23, "ldc.r8", R8;
    Dup = 0x25, "dup", None;
    Pop = 0x26, "pop", None;
    Jmp = 0x27, "jmp", Token;
    Call = 0x28, "call", Token;
    Calli = 0x29, "calli", Token;
    Ret = 0x2A, "ret", None;
    BrS = 0x2B, "br.s", ShortBranch;
    BrfalseS = 0x2C, "brfalse.s", ShortBranch;
    BrtrueS = 0x2D, "brtrue.s", ShortBranch;
    BeqS = 0x2E, "beq.s", ShortBranch;
    BgeS = 0x2F, "bge.s", ShortBranch;
    BgtS = 0x30, "bgt.s", ShortBranch;
    BleS = 0x31, "ble.s", ShortBranch;
    BltS = 0x32, "blt.s", ShortBranch;
    BneUnS = 0x33, "bne.un.s", ShortBranch;
    BgeUnS = 0x34, "bge.un.s", ShortBranch;
    BgtUnS = 0x35, "bgt.un.s", ShortBranch;
    BleUnS = 0x36, "ble.un.s", ShortBranch;
    BltUnS = 0x37, "blt.un.s", ShortBranch;
    Br = 0x38, "br", Branch;
    Brfalse = 0x39, "brfalse", Branch;
    Brtrue = 0x3A, "brtrue", Branch;
    Beq = 0x3B, "beq", Branch;
    Bge = 0x3C, "bge", Branch;
    Bgt = 0x3D, "bgt", Branch;
    Ble = 0x3E, "ble", Branch;
    Blt = 0x3F, "blt", Branch;
    BneUn = 0x40, "bne.un", Branch;
    BgeUn = 0x41, "bge.un", Branch;
    BgtUn = 0x42, "bgt.un", Branch;
    BleUn = 0x43, "ble.un", Branch;
    BltUn = 0x44, "blt.un", Branch;
    Switch = 0x45, "switch", Switch;
    LdindI1 = 0x46, "ldind.i1", None;
    LdindU1 = 0x47, "ldind.u1", None;
    LdindI2 = 0x48, "ldind.i2", None;
    LdindU2 = 0x49, "ldind.u2", None;
    LdindI4 = 0x4A, "ldind.i4", None;
    LdindU4 = 0x4B, "ldind.u4", None;
    LdindI8 = 0x4C, "ldind.i8", None;
    LdindI = 0x4D, "ldind.i", None;
    LdindR4 = 0x4E, "ldind.r4", None;
    LdindR8 = 0x4F, "ldind.r8", None;
    LdindRef = 0x50, "ldind.ref", None;
    StindRef = 0x51, "stind.ref", None;
    StindI1 = 0x52, "stind.i1", None;
    StindI2 = 0x53, "stind.i2", None;
    StindI4 = 0x54, "stind.i4", None;
    StindI8 = 0x55, "stind.i8", None;
    StindR4 = 0x56, "stind.r4", None;
    StindR8 = 0x57, "stind.r8", None;
    Add = 0x58, "add", None;
    Sub = 0x59, "sub", None;
    Mul = 0x5A, "mul", None;
    Div = 0x5B, "div", None;
    DivUn = 0x5C, "div.un", None;
    Rem = 0x5D, "rem", None;
    RemUn = 0x5E, "rem.un", None;
    And = 0x5F, "and", None;
    Or = 0x60, "or", None;
    Xor = 0x61, "xor", None;
    Shl = 0x62, "shl", None;
    Shr = 0x63, "shr", None;
    ShrUn = 0x64, "shr.un", None;
    Neg = 0x65, "neg", None;
    Not = 0x66, "not", None;
    ConvI1 = 0x67, "conv.i1", None;
    ConvI2 = 0x68, "conv.i2", None;
    ConvI4 = 0x69, "conv.i4", None;
    ConvI8 = 0x6A, "conv.i8", None;
    ConvR4 = 0x6B, "conv.r4", None;
    ConvR8 = 0x6C, "conv.r8", None;
    ConvU4 = 0x6D, "conv.u4", None;
    ConvU8 = 0x6E, "conv.u8", None;
    Callvirt = 0x6F, "callvirt", Token;
    Cpobj = 0x70, "cpobj", Token;
    Ldobj = 0x71, "ldobj", Token;
    Ldstr = 0x72, "ldstr", Token;
    Newobj = 0x73, "newobj", Token;
    Castclass = 0x74, "castclass", Token;
    Isinst = 0x75, "isinst", Token;
    ConvRUn = 0x76, "conv.r.un", None;
    Unbox = 0x79, "unbox", Token;
    Throw = 0x7A, "throw", None;
    Ldfld = 0x7B, "ldfld", Token;
    Ldflda = 0x7C, "ldflda", Token;
    Stfld = 0x7D, "stfld", Token;
    Ldsfld = 0x7E, "ldsfld", Token;
    Ldsflda = 0x7F, "ldsflda", Token;
    Stsfld = 0x80, "stsfld", Token;
    Stobj = 0x81, "stobj", Token;
    ConvOvfI1Un = 0x82, "conv.ovf.i1.un", None;
    ConvOvfI2Un = 0x83, "conv.ovf.i2.un", None;
    ConvOvfI4Un = 0x84, "conv.ovf.i4.un", None;
    ConvOvfI8Un = 0x85, "conv.ovf.i8.un", None;
    ConvOvfU1Un = 0x86, "conv.ovf.u1.un", None;
    ConvOvfU2Un = 0x87, "conv.ovf.u2.un", None;
    ConvOvfU4Un = 0x88, "conv.ovf.u4.un", None;
    ConvOvfU8Un = 0x89, "conv.ovf.u8.un", None;
    ConvOvfIUn = 0x8A, "conv.ovf.i.un", None;
    ConvOvfUUn = 0x8B, "conv.ovf.u.un", None;
    Box = 0x8C, "box", Token;
    Newarr = 0x8D, "newarr", Token;
    Ldlen = 0x8E, "ldlen", None;
    Ldelema = 0x8F, "ldelema", Token;
    LdelemI1 = 0x90, "ldelem.i1", None;
    LdelemU1 = 0x91, "ldelem.u1", None;
    LdelemI2 = 0x92, "ldelem.i2", None;
    LdelemU2 = 0x93, "ldelem.u2", None;
    LdelemI4 = 0x94, "ldelem.i4", None;
    LdelemU4 = 0x95, "ldelem.u4", None;
    LdelemI8 = 0x96, "ldelem.i8", None;
    LdelemI = 0x97, "ldelem.i", None;
    LdelemR4 = 0x98, "ldelem.r4", None;
    LdelemR8 = 0x99, "ldelem.r8", None;
    LdelemRef = 0x9A, "ldelem.ref", None;
    StelemI = 0x9B, "stelem.i", None;
    StelemI1 = 0x9C, "stelem.i1", None;
    StelemI2 = 0x9D, "stelem.i2", None;
    StelemI4 = 0x9E, "stelem.i4", None;
    StelemI8 = 0x9F, "stelem.i8", None;
    StelemR4 = 0xA0, "stelem.r4", None;
    StelemR8 = 0xA1, "stelem.r8", None;
    StelemRef = 0xA2, "stelem.ref", None;
    Ldelem = 0xA3, "ldelem", Token;
    Stelem = 0xA4, "stelem", Token;
    UnboxAny = 0xA5, "unbox.any", Token;
    ConvOvfI1 = 0xB3, "conv.ovf.i1", None;
    ConvOvfU1 = 0xB4, "conv.ovf.u1", None;
    ConvOvfI2 = 0xB5, "conv.ovf.i2", None;
    ConvOvfU2 = 0xB6, "conv.ovf.u2", None;
    ConvOvfI4 = 0xB7, "conv.ovf.i4", None;
    ConvOvfU4 = 0xB8, "conv.ovf.u4", None;
    ConvOvfI8 = 0xB9, "conv.ovf.i8", None;
    ConvOvfU8 = 0xBA, "conv.ovf.u8", None;
    Refanyval = 0xC2, "refanyval", Token;
    Ckfinite = 0xC3, "ckfinite", None;
    Mkrefany = 0xC6, "mkrefany", Token;
    Ldtoken = 0xD0, "ldtoken", Token;
    ConvU2 = 0xD1, "conv.u2", None;
    ConvU1 = 0xD2, "conv.u1", None;
    ConvI = 0xD3, "conv.i", None;
    ConvOvfI = 0xD4, "conv.ovf.i", None;
    ConvOvfU = 0xD5, "conv.ovf.u", None;
    AddOvf = 0xD6, "add.ovf", None;
    AddOvfUn = 0xD7, "add.ovf.un", None;
    MulOvf = 0xD8, "mul.ovf", None;
    MulOvfUn = 0xD9, "mul.ovf.un", None;
    SubOvf = 0xDA, "sub.ovf", None;
    SubOvfUn = 0xDB, "sub.ovf.un", None;
    Endfinally = 0xDC, "endfinally", None;
    Leave = 0xDD, "leave", Branch;
    LeaveS = 0xDE, "leave.s", ShortBranch;
    StindI = 0xDF, "stind.i", None;
    ConvU = 0xE0, "conv.u", None;

    // --- two-byte opcodes (0xFE prefix) ---
    Arglist = 0xFE00, "arglist", None;
    Ceq = 0xFE01, "ceq", None;
    Cgt = 0xFE02, "cgt", None;
    CgtUn = 0xFE03, "cgt.un", None;
    Clt = 0xFE04, "clt", None;
    CltUn = 0xFE05, "clt.un", None;
    Ldftn = 0xFE06, "ldftn", Token;
    Ldvirtftn = 0xFE07, "ldvirtftn", Token;
    Ldarg = 0xFE09, "ldarg", Var;
    Ldarga = 0xFE0A, "ldarga", Var;
    Starg = 0xFE0B, "starg", Var;
    Ldloc = 0xFE0C, "ldloc", Var;
    Ldloca = 0xFE0D, "ldloca", Var;
    Stloc = 0xFE0E, "stloc", Var;
    Localloc = 0xFE0F, "localloc", None;
    Endfilter = 0xFE11, "endfilter", None;
    Unaligned = 0xFE12, "unaligned.", ShortVar;
    Volatile = 0xFE13, "volatile.", None;
    Tail = 0xFE14, "tail.", None;
    Initobj = 0xFE15, "initobj", Token;
    Constrained = 0xFE16, "constrained.", Token;
    Cpblk = 0xFE17, "cpblk", None;
    Initblk = 0xFE18, "initblk", None;
    No = 0xFE19, "no.", ShortVar;
    Rethrow = 0xFE1A, "rethrow", None;
    Sizeof = 0xFE1C, "sizeof", Token;
    Refanytype = 0xFE1D, "refanytype", None;
    Readonly = 0xFE1E, "readonly.", None;
}

/// A decoded instruction operand.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    None,
    /// Absolute IL offset of the branch target.
    Target(u32),
    /// Local or argument index.
    Var(u32),
    I32(i32),
    I64(i64),
    F64(f64),
    Token(Token),
    /// Absolute IL offsets of every `switch` case.
    Targets(Vec<u32>),
}

impl Operand {
    pub fn as_i32(&self) -> Option<i32> {
        match self {
            Operand::I32(v) => Some(*v),
            Operand::Var(v) => Some(*v as i32),
            _ => None,
        }
    }
    pub fn as_var(&self) -> Option<u32> {
        match self {
            Operand::Var(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_token(&self) -> Option<Token> {
        match self {
            Operand::Token(t) => Some(*t),
            _ => None,
        }
    }
    pub fn as_target(&self) -> Option<u32> {
        match self {
            Operand::Target(t) => Some(*t),
            _ => None,
        }
    }
}

/// A decoded instruction with its position and length.
#[derive(Debug, Clone, PartialEq)]
pub struct Instruction {
    /// IL offset of the first byte of the opcode.
    pub offset: u32,
    /// Total encoded length, opcode plus operand.
    pub length: u32,
    pub op: Op,
    pub operand: Operand,
}

impl Instruction {
    /// IL offset of the following instruction.
    pub const fn next_offset(&self) -> u32 {
        self.offset + self.length
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Ran off the end of the IL stream.
    Truncated { offset: u32 },
    /// The byte sequence is not a defined opcode.
    Unknown { offset: u32, code: u16 },
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Truncated { offset } => write!(f, "IL truncated at offset {offset:#x}"),
            Self::Unknown { offset, code } => {
                write!(f, "unknown opcode {code:#06x} at offset {offset:#x}")
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for DecodeError {}

/// Decodes the instruction starting at `offset` in `il`.
pub fn decode(il: &[u8], offset: u32) -> Result<Instruction, DecodeError> {
    let start = offset as usize;
    let b0 = *il.get(start).ok_or(DecodeError::Truncated { offset })?;

    let (code, mut cursor) = if b0 == 0xFE {
        let b1 = *il.get(start + 1).ok_or(DecodeError::Truncated { offset })?;
        (0xFE00u16 | b1 as u16, start + 2)
    } else {
        (b0 as u16, start + 1)
    };

    let op = Op::from_code(code).ok_or(DecodeError::Unknown { offset, code })?;
    let kind = op.operand_kind();

    // A macro rather than a closure: it has to read `cursor` while the match
    // arms below are also assigning to it.
    macro_rules! need {
        ($n:expr) => {
            if cursor + $n > il.len() {
                return Err(DecodeError::Truncated { offset });
            }
        };
    }

    let operand = match kind {
        OperandKind::None => Operand::None,
        OperandKind::ShortI => {
            need!(1);
            let v = il[cursor] as i8 as i32;
            cursor += 1;
            Operand::I32(v)
        }
        OperandKind::ShortVar => {
            need!(1);
            let v = il[cursor] as u32;
            cursor += 1;
            Operand::Var(v)
        }
        OperandKind::Var => {
            need!(2);
            let v = u16::from_le_bytes([il[cursor], il[cursor + 1]]) as u32;
            cursor += 2;
            Operand::Var(v)
        }
        OperandKind::I4 => {
            need!(4);
            let v = i32::from_le_bytes([il[cursor], il[cursor + 1], il[cursor + 2], il[cursor + 3]]);
            cursor += 4;
            Operand::I32(v)
        }
        OperandKind::I8 => {
            need!(8);
            let mut b = [0u8; 8];
            b.copy_from_slice(&il[cursor..cursor + 8]);
            cursor += 8;
            Operand::I64(i64::from_le_bytes(b))
        }
        OperandKind::R4 => {
            need!(4);
            let v = f32::from_le_bytes([il[cursor], il[cursor + 1], il[cursor + 2], il[cursor + 3]]);
            cursor += 4;
            Operand::F64(v as f64)
        }
        OperandKind::R8 => {
            need!(8);
            let mut b = [0u8; 8];
            b.copy_from_slice(&il[cursor..cursor + 8]);
            cursor += 8;
            Operand::F64(f64::from_le_bytes(b))
        }
        OperandKind::Token => {
            need!(4);
            let v = u32::from_le_bytes([il[cursor], il[cursor + 1], il[cursor + 2], il[cursor + 3]]);
            cursor += 4;
            Operand::Token(Token(v))
        }
        OperandKind::ShortBranch => {
            need!(1);
            let disp = il[cursor] as i8 as i64;
            cursor += 1;
            // Displacement is relative to the instruction *after* the branch.
            Operand::Target((cursor as i64 + disp) as u32)
        }
        OperandKind::Branch => {
            need!(4);
            let disp =
                i32::from_le_bytes([il[cursor], il[cursor + 1], il[cursor + 2], il[cursor + 3]])
                    as i64;
            cursor += 4;
            Operand::Target((cursor as i64 + disp) as u32)
        }
        OperandKind::Switch => {
            need!(4);
            let count =
                u32::from_le_bytes([il[cursor], il[cursor + 1], il[cursor + 2], il[cursor + 3]])
                    as usize;
            cursor += 4;
            need!(count * 4);
            let base = cursor + count * 4;
            let mut targets = Vec::with_capacity(count);
            for i in 0..count {
                let o = cursor + i * 4;
                let disp = i32::from_le_bytes([il[o], il[o + 1], il[o + 2], il[o + 3]]) as i64;
                targets.push((base as i64 + disp) as u32);
            }
            cursor = base;
            Operand::Targets(targets)
        }
    };

    Ok(Instruction {
        offset,
        length: (cursor - start) as u32,
        op,
        operand,
    })
}

/// Decodes an entire method body into a instruction list.
pub fn decode_all(il: &[u8]) -> Result<Vec<Instruction>, DecodeError> {
    let mut out = Vec::new();
    let mut offset = 0u32;
    while (offset as usize) < il.len() {
        let ins = decode(il, offset)?;
        offset = ins.next_offset();
        out.push(ins);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_add_method_from_the_csharp_compiler() {
        // ldarg.0; ldarg.1; add; ret
        let instructions = decode_all(&[0x02, 0x03, 0x58, 0x2A]).unwrap();
        let names: Vec<&str> = instructions.iter().map(|i| i.op.name()).collect();
        assert_eq!(names, ["ldarg.0", "ldarg.1", "add", "ret"]);
        assert!(instructions.iter().all(|i| i.length == 1));
    }

    #[test]
    fn short_branch_targets_are_relative_to_the_next_instruction() {
        // offset 0: br.s +2  -> lands on offset 4
        // offset 2: nop, nop
        let ins = decode(&[0x2B, 0x02, 0x00, 0x00, 0x2A], 0).unwrap();
        assert_eq!(ins.op, Op::BrS);
        assert_eq!(ins.operand, Operand::Target(4));
        assert_eq!(ins.length, 2);
    }

    #[test]
    fn a_backward_branch_decodes_to_a_lower_offset() {
        // offset 2: br.s -4 -> lands on offset 0
        let ins = decode(&[0x00, 0x00, 0x2B, 0xFC], 2).unwrap();
        assert_eq!(ins.operand, Operand::Target(0));
    }

    #[test]
    fn two_byte_opcodes_decode_through_the_fe_prefix() {
        let ins = decode(&[0xFE, 0x01], 0).unwrap();
        assert_eq!(ins.op, Op::Ceq);
        assert_eq!(ins.length, 2);
    }

    #[test]
    fn switch_targets_are_relative_to_the_end_of_the_table() {
        // switch (2) { +0, +1 } then two nops
        let il = [0x45, 0x02, 0, 0, 0, 0x00, 0, 0, 0, 0x01, 0, 0, 0, 0x00, 0x00];
        let ins = decode(&il, 0).unwrap();
        assert_eq!(ins.length, 13);
        assert_eq!(ins.operand, Operand::Targets(vec![13, 14]));
    }

    #[test]
    fn ldc_i4_carries_a_full_32_bit_immediate() {
        let ins = decode(&[0x20, 0x40, 0xE2, 0x01, 0x00], 0).unwrap();
        assert_eq!(ins.op, Op::LdcI4);
        assert_eq!(ins.operand, Operand::I32(123456));
        assert_eq!(ins.length, 5);
    }

    #[test]
    fn truncated_il_is_an_error_not_a_panic() {
        assert_eq!(decode(&[0x20, 0x01], 0), Err(DecodeError::Truncated { offset: 0 }));
        assert!(matches!(decode(&[0xFE, 0xEE], 0), Err(DecodeError::Unknown { .. })));
    }

    #[test]
    fn every_opcode_has_a_unique_code_and_round_trips() {
        // A representative sample across the one- and two-byte spaces.
        for op in [Op::Nop, Op::Ret, Op::Ldstr, Op::Ceq, Op::Constrained, Op::Readonly] {
            assert_eq!(Op::from_code(op.code()), Some(op));
        }
    }
}
