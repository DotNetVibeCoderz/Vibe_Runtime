//! Runtime errors, and the managed exceptions they map to.

use crate::opcode::DecodeError;
use rustclr_gc::Handle;
use rustclr_metadata::{MetadataError, Token};
use core::fmt;

#[allow(unused_imports)]
use crate::prelude::*;

/// A CLR-visible exception kind. Native code raises these; the interpreter
/// turns them into managed exception objects that `catch` blocks can see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClrExceptionKind {
    NullReference,
    IndexOutOfRange,
    InvalidCast,
    DivideByZero,
    Overflow,
    OutOfMemory,
    StackOverflow,
    ArgumentNull,
    Argument,
    ArgumentOutOfRange,
    InvalidOperation,
    NotSupported,
    NotImplemented,
    TypeLoad,
    MissingMethod,
    MissingField,
    EntryPointNotFound,
    DllNotFound,
    Arithmetic,
    Format,
    Io,
    /// An exception type raised by managed code itself.
    Managed(String),
}

impl ClrExceptionKind {
    /// The .NET type name this maps to.
    pub fn type_name(&self) -> &str {
        match self {
            Self::NullReference => "System.NullReferenceException",
            Self::IndexOutOfRange => "System.IndexOutOfRangeException",
            Self::InvalidCast => "System.InvalidCastException",
            Self::DivideByZero => "System.DivideByZeroException",
            Self::Overflow => "System.OverflowException",
            Self::OutOfMemory => "System.OutOfMemoryException",
            Self::StackOverflow => "System.StackOverflowException",
            Self::ArgumentNull => "System.ArgumentNullException",
            Self::Argument => "System.ArgumentException",
            Self::ArgumentOutOfRange => "System.ArgumentOutOfRangeException",
            Self::InvalidOperation => "System.InvalidOperationException",
            Self::NotSupported => "System.NotSupportedException",
            Self::NotImplemented => "System.NotImplementedException",
            Self::TypeLoad => "System.TypeLoadException",
            Self::MissingMethod => "System.MissingMethodException",
            Self::MissingField => "System.MissingFieldException",
            Self::EntryPointNotFound => "System.EntryPointNotFoundException",
            Self::DllNotFound => "System.DllNotFoundException",
            Self::Arithmetic => "System.ArithmeticException",
            Self::Format => "System.FormatException",
            Self::Io => "System.IO.IOException",
            Self::Managed(name) => name,
        }
    }
}

/// Everything the execution engine can fail with.
#[derive(Debug, Clone)]
pub enum ExecutionError {
    /// A managed exception is in flight. `object` is the exception instance
    /// once one has been allocated.
    Exception {
        kind: ClrExceptionKind,
        message: String,
        object: Handle,
    },
    /// The IL stream is malformed.
    InvalidProgram(String),
    /// A metadata token could not be resolved.
    UnresolvedToken { token: Token, context: String },
    /// A type could not be loaded.
    TypeLoad(String),
    /// A method exists in metadata but has no implementation available.
    MissingImplementation(String),
    /// The evaluation stack under- or over-flowed.
    StackImbalance { at: String, detail: String },
    /// Call depth exceeded the configured limit.
    CallDepthExceeded(usize),
    /// The interpreter ran more instructions than the configured budget.
    InstructionBudgetExceeded(u64),
    /// A feature this runtime does not implement yet.
    Unsupported(String),
    /// Failure while reading an assembly.
    Metadata(MetadataError),
    /// Failure while decoding IL.
    Decode(DecodeError),
}

impl ExecutionError {
    pub fn exception(kind: ClrExceptionKind, message: impl Into<String>) -> Self {
        Self::Exception {
            kind,
            message: message.into(),
            object: Handle::NULL,
        }
    }

    pub fn null_reference() -> Self {
        Self::exception(
            ClrExceptionKind::NullReference,
            "Object reference not set to an instance of an object.",
        )
    }

    pub fn index_out_of_range(index: i64, length: usize) -> Self {
        Self::exception(
            ClrExceptionKind::IndexOutOfRange,
            format!("Index {index} is outside the bounds of the array (length {length})."),
        )
    }

    pub fn divide_by_zero() -> Self {
        Self::exception(ClrExceptionKind::DivideByZero, "Attempted to divide by zero.")
    }

    pub fn overflow() -> Self {
        Self::exception(
            ClrExceptionKind::Overflow,
            "Arithmetic operation resulted in an overflow.",
        )
    }

    pub fn invalid_cast(from: &str, to: &str) -> Self {
        Self::exception(
            ClrExceptionKind::InvalidCast,
            format!("Unable to cast object of type '{from}' to type '{to}'."),
        )
    }

    /// True when this represents a managed exception a `catch` could handle.
    pub fn is_managed_exception(&self) -> bool {
        matches!(self, Self::Exception { .. })
    }

    pub fn exception_type_name(&self) -> Option<&str> {
        match self {
            Self::Exception { kind, .. } => Some(kind.type_name()),
            _ => None,
        }
    }
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exception { kind, message, .. } => {
                write!(f, "{}: {}", kind.type_name(), message)
            }
            Self::InvalidProgram(m) => write!(f, "invalid program: {m}"),
            Self::UnresolvedToken { token, context } => {
                write!(f, "could not resolve token {token} ({context})")
            }
            Self::TypeLoad(m) => write!(f, "type load failed: {m}"),
            Self::MissingImplementation(m) => write!(f, "no implementation for {m}"),
            Self::StackImbalance { at, detail } => {
                write!(f, "evaluation stack imbalance in {at}: {detail}")
            }
            Self::CallDepthExceeded(n) => write!(f, "call depth exceeded {n} frames"),
            Self::InstructionBudgetExceeded(n) => {
                write!(f, "instruction budget of {n} exceeded")
            }
            Self::Unsupported(m) => write!(f, "unsupported: {m}"),
            Self::Metadata(e) => write!(f, "metadata error: {e}"),
            Self::Decode(e) => write!(f, "{e}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ExecutionError {}

impl From<MetadataError> for ExecutionError {
    fn from(e: MetadataError) -> Self {
        Self::Metadata(e)
    }
}

impl From<DecodeError> for ExecutionError {
    fn from(e: DecodeError) -> Self {
        Self::Decode(e)
    }
}

pub type ExecResult<T> = Result<T, ExecutionError>;
