//! Canonical names for types and method signatures.
//!
//! These strings are the key the interpreter uses to bind a `MemberRef` to a
//! native RustBCL implementation, so they must be stable and unambiguous
//! between overloads.

use rustclr_metadata::{MethodSig, TypeSig};

/// Renders a signature type the way the key format expects.
pub fn type_sig_name(sig: &TypeSig) -> String {
    match sig {
        TypeSig::Void => "void".into(),
        TypeSig::Boolean => "bool".into(),
        TypeSig::Char => "char".into(),
        TypeSig::I1 => "sbyte".into(),
        TypeSig::U1 => "byte".into(),
        TypeSig::I2 => "short".into(),
        TypeSig::U2 => "ushort".into(),
        TypeSig::I4 => "int".into(),
        TypeSig::U4 => "uint".into(),
        TypeSig::I8 => "long".into(),
        TypeSig::U8 => "ulong".into(),
        TypeSig::R4 => "float".into(),
        TypeSig::R8 => "double".into(),
        TypeSig::String => "string".into(),
        TypeSig::IntPtr => "nint".into(),
        TypeSig::UIntPtr => "nuint".into(),
        TypeSig::Object => "object".into(),
        TypeSig::TypedByRef => "typedref".into(),
        TypeSig::ValueType(t) | TypeSig::Class(t) => format!("#{}", t.raw()),
        TypeSig::Ptr(inner) => format!("{}*", type_sig_name(inner)),
        TypeSig::ByRef(inner) => format!("{}&", type_sig_name(inner)),
        TypeSig::SzArray(inner) => format!("{}[]", type_sig_name(inner)),
        TypeSig::Array { element, rank, .. } => {
            format!("{}[{}]", type_sig_name(element), ",".repeat((*rank as usize).saturating_sub(1)))
        }
        TypeSig::GenericInst { definition, args, .. } => {
            let inner: Vec<String> = args.iter().map(type_sig_name).collect();
            format!("#{}<{}>", definition.raw(), inner.join(","))
        }
        TypeSig::Var(i) => format!("!{i}"),
        TypeSig::MVar(i) => format!("!!{i}"),
        TypeSig::FnPtr(_) => "fnptr".into(),
        TypeSig::Modified { inner, .. } | TypeSig::Pinned(inner) => type_sig_name(inner),
    }
}

/// The arity-and-shape suffix that distinguishes overloads.
///
/// Types declared in other assemblies appear as `#token`, which is unstable
/// across assemblies, so native binding keys use [`native_key`] instead, which
/// falls back to arity alone when a parameter is not a primitive.
pub fn signature_suffix(sig: &MethodSig) -> String {
    let params: Vec<String> = sig.params.iter().map(type_sig_name).collect();
    format!("({})", params.join(","))
}

/// The key used to bind a method to a native implementation.
///
/// Format: `Namespace.Type::Method/arity`, optionally followed by a
/// primitive-only parameter list when that is enough to pick an overload.
/// Keeping the arity separate means a native table can register a single
/// handler for all overloads of the same shape.
pub fn native_key(declaring_type: &str, method: &str, sig: &MethodSig) -> String {
    format!("{declaring_type}::{method}/{}", sig.params.len())
}

/// A more specific key that also names primitive parameter types, used to pick
/// between overloads of equal arity such as `WriteLine(int)` and
/// `WriteLine(string)`.
pub fn native_key_typed(declaring_type: &str, method: &str, sig: &MethodSig) -> String {
    let params: Vec<String> = sig.params.iter().map(|p| type_sig_name(p.unwrap_modifiers())).collect();
    format!("{declaring_type}::{method}({})", params.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustclr_metadata::SignatureParser;

    fn sig(bytes: &[u8]) -> MethodSig {
        SignatureParser::new(bytes).parse_method().unwrap()
    }

    #[test]
    fn typed_keys_separate_overloads_of_equal_arity() {
        // static void M(string)
        let s = sig(&[0x00, 0x01, 0x01, 0x0E]);
        // static void M(int)
        let i = sig(&[0x00, 0x01, 0x01, 0x08]);

        assert_eq!(native_key("System.Console", "WriteLine", &s), "System.Console::WriteLine/1");
        assert_eq!(native_key("System.Console", "WriteLine", &i), "System.Console::WriteLine/1");
        assert_ne!(
            native_key_typed("System.Console", "WriteLine", &s),
            native_key_typed("System.Console", "WriteLine", &i)
        );
        assert_eq!(
            native_key_typed("System.Console", "WriteLine", &s),
            "System.Console::WriteLine(string)"
        );
    }

    #[test]
    fn array_and_byref_types_render_distinctly() {
        assert_eq!(type_sig_name(&TypeSig::SzArray(Box::new(TypeSig::String))), "string[]");
        assert_eq!(type_sig_name(&TypeSig::ByRef(Box::new(TypeSig::I4))), "int&");
    }
}
