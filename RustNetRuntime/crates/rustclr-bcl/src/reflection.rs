//! `System.Type`, `System.Reflection` and `System.Activator`.
//!
//! Until this landed, `GetType()` returned a *string* and `typeof(T)` an opaque
//! token: enough for the identity comparison a record generates, and nothing
//! else. A program could not ask a type its name, walk its members, or build an
//! instance it only knew at run time.
//!
//! # A Type is an object holding a type id
//!
//! The interpreter interns one `System.Type` instance per runtime type, so
//! `typeof(int) == typeof(int)` is reference equality — which .NET guarantees
//! and real code relies on. `ldtoken` resolves a type token the moment it is
//! executed rather than carrying it as a number, because a metadata token means
//! nothing outside the assembly that emitted it and the handle may be consumed
//! somewhere else entirely.
//!
//! `MethodInfo`, `FieldInfo` and `PropertyInfo` are the same shape: an id plus
//! the type they were reached through.
//!
//! # What is not here
//!
//! Custom attributes. Reading them means decoding the `CustomAttribute` table
//! and its blob-encoded arguments, and `GetCustomAttributes` returns an empty
//! array rather than pretending — see `docs/limitations.md`.

use crate::collections::{field, set_field};
use crate::support::*;
use rustclr_core::{
    ClrArray, ClrExceptionKind, ExecResult, ExecutionError, FieldId, Interpreter, MethodId,
    TypeId, TypeKind, Value,
};
use rustclr_gc::Handle;

const REFLECT: &str = "System.Reflection";

/// Leaks a stable key string; the native table holds it for the process life.
fn key(type_name: &str, member: &str) -> &'static str {
    Box::leak(format!("{type_name}::{member}").into_boxed_str())
}

pub fn register(interp: &mut Interpreter) {
    // Every user attribute derives from `System.Attribute`, whose constructor
    // chain has to terminate somewhere.
    interp.register_native("System.Attribute::.ctor()", |_i, _a| Ok(None));
    interp.register_native("System.Attribute::.ctor/0", |_i, _a| Ok(None));

    register_type(interp);
    register_members(interp);
    register_activator(interp);
}

// -- System.Type --------------------------------------------------------------

/// The runtime type a `System.Type` argument describes.
fn described(interp: &mut Interpreter, args: &[Value], index: usize) -> ExecResult<TypeId> {
    let handle = arg(interp, args, index)?
        .as_handle()
        .filter(|h| !h.is_null())
        .ok_or_else(ExecutionError::null_reference)?;
    interp.type_from_object(handle).ok_or_else(|| {
        ExecutionError::exception(
            ClrExceptionKind::Argument,
            "the value is not a System.Type instance",
        )
    })
}

fn register_type(interp: &mut Interpreter) {
    const TYPE: &str = "System.Type";

    // `typeof(T)` is `ldtoken T; call GetTypeFromHandle`, and `ldtoken` has
    // already produced the Type object — so this is the identity function.
    interp.register_native(key(TYPE, "GetTypeFromHandle/1"), |i, a| Ok(Some(arg(i, a, 0)?)));

    interp.register_native(key(TYPE, "get_Name()"), |i, a| {
        let id = described(i, a, 0)?;
        let name = i.loader.registry.ty(id).name.clone();
        Ok(Some(string_value(i, &name)))
    });
    interp.register_native(key(TYPE, "get_FullName()"), |i, a| {
        let id = described(i, a, 0)?;
        let name = i.loader.registry.ty(id).full_name();
        Ok(Some(string_value(i, &name)))
    });
    interp.register_native(key(TYPE, "ToString()"), |i, a| {
        let id = described(i, a, 0)?;
        let name = i.loader.registry.ty(id).full_name();
        Ok(Some(string_value(i, &name)))
    });
    interp.register_native(key(TYPE, "get_Namespace()"), |i, a| {
        let id = described(i, a, 0)?;
        let name = i.loader.registry.ty(id).namespace.clone();
        Ok(Some(string_value(i, &name)))
    });

    interp.register_native(key(TYPE, "get_BaseType()"), |i, a| {
        let id = described(i, a, 0)?;
        match i.loader.registry.ty(id).base {
            Some(base) => {
                let handle = i.type_object(base);
                Ok(Some(Value::Obj(handle)))
            }
            None => Ok(Some(Value::Null)),
        }
    });

    for (member, test) in [
        ("get_IsValueType()", TypeTest::ValueType),
        ("get_IsClass()", TypeTest::Class),
        ("get_IsInterface()", TypeTest::Interface),
        ("get_IsEnum()", TypeTest::Enum),
        ("get_IsArray()", TypeTest::Array),
        ("get_IsPrimitive()", TypeTest::Primitive),
        ("get_IsAbstract()", TypeTest::Abstract),
        ("get_IsSealed()", TypeTest::Sealed),
    ] {
        // Native bindings are plain function pointers, so each test gets its
        // own entry point rather than a captured discriminant.
        let f: rustclr_core::NativeFn = match test {
            TypeTest::ValueType => |i, a| type_flag(i, a, TypeTest::ValueType),
            TypeTest::Class => |i, a| type_flag(i, a, TypeTest::Class),
            TypeTest::Interface => |i, a| type_flag(i, a, TypeTest::Interface),
            TypeTest::Enum => |i, a| type_flag(i, a, TypeTest::Enum),
            TypeTest::Array => |i, a| type_flag(i, a, TypeTest::Array),
            TypeTest::Primitive => |i, a| type_flag(i, a, TypeTest::Primitive),
            TypeTest::Abstract => |i, a| type_flag(i, a, TypeTest::Abstract),
            TypeTest::Sealed => |i, a| type_flag(i, a, TypeTest::Sealed),
        };
        interp.register_native(key(TYPE, member), f);
    }

    interp.register_native(key(TYPE, "IsAssignableFrom/1"), |i, a| {
        let target = described(i, a, 0)?;
        let source = described(i, a, 1)?;
        Ok(Some(Value::I32(assignable(i, target, source) as i32)))
    });
    interp.register_native(key(TYPE, "IsInstanceOfType(object)"), |i, a| {
        let target = described(i, a, 0)?;
        let value = arg(i, a, 1)?;
        let Some(h) = value.as_handle().filter(|h| !h.is_null()) else {
            return Ok(Some(Value::I32(0)));
        };
        let Some(actual) = i.type_of(h) else { return Ok(Some(Value::I32(0))) };
        Ok(Some(Value::I32(assignable(i, target, actual) as i32)))
    });

    // Identity: the interned instance means reference equality is type
    // equality, so these need no special case beyond comparing handles.
    interp.register_native(key(TYPE, "op_Equality/2"), |i, a| {
        Ok(Some(Value::I32(same_type(i, a)? as i32)))
    });
    interp.register_native(key(TYPE, "op_Inequality/2"), |i, a| {
        Ok(Some(Value::I32(!same_type(i, a)? as i32)))
    });
    interp.register_native(key(TYPE, "Equals(object)"), |i, a| {
        Ok(Some(Value::I32(same_type(i, a)? as i32)))
    });
    interp.register_native(key(TYPE, "GetHashCode()"), |i, a| {
        let id = described(i, a, 0)?;
        Ok(Some(Value::I32(id.0 as i32)))
    });

    interp.register_native(key(TYPE, "GetType(string)"), |i, a| {
        let name = arg_string_or_empty(i, a, 0)?;
        match i.loader.registry.find_type_by_name(&name) {
            Some(id) => {
                let handle = i.type_object(id);
                Ok(Some(Value::Obj(handle)))
            }
            None => Ok(Some(Value::Null)),
        }
    });

    // Member enumeration.
    interp.register_native(key(TYPE, "GetMethods()"), |i, a| {
        let id = described(i, a, 0)?;
        let members = declared_methods(i, id, false);
        Ok(Some(member_array(i, "MethodInfo", id, &members)))
    });
    interp.register_native(key(TYPE, "GetMethods/1"), |i, a| {
        let id = described(i, a, 0)?;
        let members = declared_methods(i, id, false);
        Ok(Some(member_array(i, "MethodInfo", id, &members)))
    });
    interp.register_native(key(TYPE, "GetConstructors()"), |i, a| {
        let id = described(i, a, 0)?;
        let members = declared_methods(i, id, true);
        Ok(Some(member_array(i, "ConstructorInfo", id, &members)))
    });
    interp.register_native(key(TYPE, "GetMethod(string)"), |i, a| {
        let id = described(i, a, 0)?;
        let name = arg_string_or_empty(i, a, 1)?;
        match declared_methods(i, id, false).into_iter().find(|m| {
            i.loader.registry.method(*m).name == name
        }) {
            Some(m) => Ok(Some(new_member(i, "MethodInfo", m.0, id))),
            None => Ok(Some(Value::Null)),
        }
    });

    interp.register_native(key(TYPE, "GetFields()"), |i, a| {
        let id = described(i, a, 0)?;
        let members = declared_fields(i, id);
        let ids: Vec<u32> = members.iter().map(|f| f.0).collect();
        Ok(Some(id_array(i, "FieldInfo", id, &ids)))
    });
    interp.register_native(key(TYPE, "GetFields/1"), |i, a| {
        let id = described(i, a, 0)?;
        let members = declared_fields(i, id);
        let ids: Vec<u32> = members.iter().map(|f| f.0).collect();
        Ok(Some(id_array(i, "FieldInfo", id, &ids)))
    });
    interp.register_native(key(TYPE, "GetField(string)"), |i, a| {
        let id = described(i, a, 0)?;
        let name = arg_string_or_empty(i, a, 1)?;
        match declared_fields(i, id)
            .into_iter()
            .find(|f| i.loader.registry.field(*f).name == name)
        {
            Some(f) => Ok(Some(new_member(i, "FieldInfo", f.0, id))),
            None => Ok(Some(Value::Null)),
        }
    });

    for member in ["GetCustomAttributes/1", "GetCustomAttributes/2"] {
        interp.register_native(key(TYPE, member), |i, a| {
            let id = described(i, a, 0)?;
            let ty = i.loader.registry.ty(id);
            let (assembly, token) = (ty.assembly, ty.token);
            let instances = attribute_instances(i, assembly, token)?;
            Ok(Some(filtered_attributes(i, a, instances)))
        });
    }
    interp.register_native(key(TYPE, "IsDefined/2"), |i, a| {
        let id = described(i, a, 0)?;
        let ty = i.loader.registry.ty(id);
        let (assembly, token) = (ty.assembly, ty.token);
        let instances = attribute_instances(i, assembly, token)?;
        Ok(Some(Value::I32(!matching(i, a, &instances).is_empty() as i32)))
    })
}

/// Narrows attribute instances to the requested type, when one was given.
///
/// `GetCustomAttributes(attributeType, inherit)` filters; the one-argument
/// overload takes only `inherit` and returns everything.
fn matching(interp: &mut Interpreter, args: &[Value], instances: &[Value]) -> Vec<Value> {
    let wanted = args.get(1).and_then(|v| v.as_handle()).and_then(|h| {
        if h.is_null() {
            None
        } else {
            interp.type_from_object(h)
        }
    });
    let Some(wanted) = wanted else { return instances.to_vec() };
    instances
        .iter()
        .filter(|v| {
            v.as_handle()
                .and_then(|h| interp.type_of(h))
                .map(|actual| assignable(interp, wanted, actual))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

fn filtered_attributes(
    interp: &mut Interpreter,
    args: &[Value],
    instances: Vec<Value>,
) -> Value {
    let kept = matching(interp, args, &instances);
    let array = interp.alloc_value_array(0);
    if let Some(a) = interp.heap.get_as_mut::<ClrArray>(array) {
        if let Some(v) = a.storage.values_mut() {
            *v = kept;
            a.dimensions = vec![v.len() as u32];
        }
    }
    Value::Obj(array)
}

#[derive(Clone, Copy)]
enum TypeTest {
    ValueType,
    Class,
    Interface,
    Enum,
    Array,
    Primitive,
    Abstract,
    Sealed,
}

fn type_flag(
    interp: &mut Interpreter,
    args: &[Value],
    test: TypeTest,
) -> ExecResult<Option<Value>> {
    let id = described(interp, args, 0)?;
    let ty = interp.loader.registry.ty(id);
    let answer = match test {
        TypeTest::ValueType => ty.kind.is_value_like(),
        TypeTest::Class => matches!(ty.kind, TypeKind::Class | TypeKind::String),
        TypeTest::Interface => ty.kind == TypeKind::Interface,
        TypeTest::Enum => ty.kind == TypeKind::Enum,
        TypeTest::Array => ty.is_array(),
        TypeTest::Primitive => ty.primitive.is_some(),
        TypeTest::Abstract => ty.is_abstract,
        TypeTest::Sealed => ty.is_sealed,
    };
    Ok(Some(Value::I32(answer as i32)))
}

/// Whether a value of type `source` can be assigned to `target`.
fn assignable(interp: &Interpreter, target: TypeId, source: TypeId) -> bool {
    if target == source {
        return true;
    }
    for base in interp.loader.registry.base_chain(source) {
        if base == target {
            return true;
        }
        if interp.loader.registry.ty(base).interfaces.contains(&target) {
            return true;
        }
    }
    false
}

fn same_type(interp: &mut Interpreter, args: &[Value]) -> ExecResult<bool> {
    let left = arg(interp, args, 0)?.as_handle();
    let right = arg(interp, args, 1)?.as_handle();
    Ok(match (left, right) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    })
}

// -- members ------------------------------------------------------------------

/// Methods a type declares itself, optionally only its constructors.
fn declared_methods(interp: &Interpreter, type_id: TypeId, constructors: bool) -> Vec<MethodId> {
    interp
        .loader
        .registry
        .ty(type_id)
        .methods
        .iter()
        .copied()
        .filter(|m| {
            let info = interp.loader.registry.method(*m);
            let is_ctor = info.name == ".ctor" || info.name == ".cctor";
            is_ctor == constructors
        })
        .collect()
}

fn declared_fields(interp: &Interpreter, type_id: TypeId) -> Vec<FieldId> {
    let ty = interp.loader.registry.ty(type_id);
    ty.instance_fields.iter().chain(&ty.static_fields).copied().collect()
}

/// Allocates a member handle: the id of what it describes, plus its type.
fn new_member(interp: &mut Interpreter, type_name: &str, id: u32, declaring: TypeId) -> Value {
    let Some(type_id) = interp
        .loader
        .registry
        .find_type_by_name(&format!("{REFLECT}.{type_name}"))
    else {
        return Value::Null;
    };
    let handle = interp.alloc_object(type_id);
    set_field(interp, handle, 0, Value::I32(id as i32));
    set_field(interp, handle, 1, Value::I32(declaring.0 as i32));
    Value::Obj(handle)
}

fn member_array(
    interp: &mut Interpreter,
    type_name: &str,
    declaring: TypeId,
    members: &[MethodId],
) -> Value {
    let ids: Vec<u32> = members.iter().map(|m| m.0).collect();
    id_array(interp, type_name, declaring, &ids)
}

fn id_array(
    interp: &mut Interpreter,
    type_name: &str,
    declaring: TypeId,
    ids: &[u32],
) -> Value {
    let values: Vec<Value> =
        ids.iter().map(|id| new_member(interp, type_name, *id, declaring)).collect();
    let array = interp.alloc_value_array(0);
    if let Some(a) = interp.heap.get_as_mut::<ClrArray>(array) {
        if let Some(v) = a.storage.values_mut() {
            *v = values;
            a.dimensions = vec![v.len() as u32];
        }
    }
    Value::Obj(array)
}

/// The attributes applied to whatever a member handle describes.
///
/// A `FieldInfo` and a `MethodInfo` both arrive here; the id space is separate
/// per kind, so the declaring type decides which registry to consult.
fn member_attributes(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Vec<Value>> {
    let handle = arg(interp, args, 0)?
        .as_handle()
        .filter(|h| !h.is_null())
        .ok_or_else(ExecutionError::null_reference)?;
    let id = field(interp, handle, 0).as_i32().unwrap_or(0) as u32;
    let Some(kind) = interp.type_of(handle) else { return Ok(Vec::new()) };
    let is_field = interp.loader.registry.ty(kind).full_name() == "System.Reflection.FieldInfo";

    let (assembly, token) = if is_field {
        let info = interp.loader.registry.field(FieldId(id));
        (info.declaring_type, info.token)
    } else {
        let info = interp.loader.registry.method(MethodId(id));
        (info.declaring_type, info.token)
    };
    // A field records its declaring *type*, so the assembly comes from there.
    let assembly = interp.loader.registry.ty(assembly).assembly;
    attribute_instances(interp, assembly, token)
}

/// Whether two member handles describe the same member.
fn same_member(interp: &mut Interpreter, args: &[Value]) -> ExecResult<bool> {
    let left = arg(interp, args, 0)?;
    let right = arg(interp, args, 1)?;
    let (Some(a), Some(b)) = (
        left.as_handle().filter(|h| !h.is_null()),
        right.as_handle().filter(|h| !h.is_null()),
    ) else {
        // One or both are null: equal only when both are.
        return Ok(left.is_null() && right.is_null());
    };
    if a == b {
        return Ok(true);
    }
    Ok(field(interp, a, 0) == field(interp, b, 0)
        && field(interp, a, 1) == field(interp, b, 1))
}

/// The id a member handle carries.
fn member_id(interp: &mut Interpreter, args: &[Value]) -> ExecResult<u32> {
    let handle = arg(interp, args, 0)?
        .as_handle()
        .filter(|h| !h.is_null())
        .ok_or_else(ExecutionError::null_reference)?;
    Ok(field(interp, handle, 0).as_i32().unwrap_or(0) as u32)
}

fn register_members(interp: &mut Interpreter) {
    // `info != null` compiles to `op_Inequality` on the member type, not to a
    // null check, so these have to exist or the comparison fails to resolve.
    // Member handles compare by what they describe: two handles for the same
    // method are equal even though they are separate objects.
    for type_name in [
        "MemberInfo",
        "MethodInfo",
        "ConstructorInfo",
        "MethodBase",
        "FieldInfo",
        "PropertyInfo",
    ] {
        let full: &'static str = Box::leak(format!("{REFLECT}.{type_name}").into_boxed_str());
        interp.register_native(key(full, "op_Equality/2"), |i, a| {
            Ok(Some(Value::I32(same_member(i, a)? as i32)))
        });
        interp.register_native(key(full, "op_Inequality/2"), |i, a| {
            Ok(Some(Value::I32(!same_member(i, a)? as i32)))
        });
        interp.register_native(key(full, "Equals(object)"), |i, a| {
            Ok(Some(Value::I32(same_member(i, a)? as i32)))
        });
        interp.register_native(key(full, "GetHashCode()"), |i, a| {
            Ok(Some(Value::I32(member_id(i, a)? as i32)))
        });
        for member in ["GetCustomAttributes/1", "GetCustomAttributes/2"] {
            interp.register_native(key(full, member), |i, a| {
                let instances = member_attributes(i, a)?;
                Ok(Some(filtered_attributes(i, a, instances)))
            });
        }
        interp.register_native(key(full, "IsDefined/2"), |i, a| {
            let instances = member_attributes(i, a)?;
            Ok(Some(Value::I32(!matching(i, a, &instances).is_empty() as i32)))
        });
    }

    for type_name in ["MethodInfo", "ConstructorInfo", "MemberInfo", "MethodBase"] {
        let full: &'static str = Box::leak(format!("{REFLECT}.{type_name}").into_boxed_str());
        interp.register_native(key(full, "get_Name()"), |i, a| {
            let id = MethodId(member_id(i, a)?);
            let name = i.loader.registry.method(id).name.clone();
            Ok(Some(string_value(i, &name)))
        });
        interp.register_native(key(full, "ToString()"), |i, a| {
            let id = MethodId(member_id(i, a)?);
            let name = i.loader.registry.method(id).qualified_name.clone();
            Ok(Some(string_value(i, &name)))
        });
        interp.register_native(key(full, "get_IsStatic()"), |i, a| {
            let id = MethodId(member_id(i, a)?);
            Ok(Some(Value::I32(i.loader.registry.method(id).is_static() as i32)))
        });
        interp.register_native(key(full, "get_IsVirtual()"), |i, a| {
            let id = MethodId(member_id(i, a)?);
            Ok(Some(Value::I32(i.loader.registry.method(id).is_virtual() as i32)))
        });
        interp.register_native(key(full, "get_DeclaringType()"), |i, a| {
            let id = MethodId(member_id(i, a)?);
            let declaring = i.loader.registry.method(id).declaring_type;
            let handle = i.type_object(declaring);
            Ok(Some(Value::Obj(handle)))
        });
        interp.register_native(key(full, "get_ReturnType()"), |i, a| {
            let id = MethodId(member_id(i, a)?);
            let info = i.loader.registry.method(id);
            let assembly = info.assembly;
            let signature = info.signature.return_type.clone();
            let resolved = i
                .loader
                .resolve_type_sig(i.loader.assembly(assembly), &signature)
                .unwrap_or_else(|| i.loader.core().object);
            let handle = i.type_object(resolved);
            Ok(Some(Value::Obj(handle)))
        });
        interp.register_native(key(full, "Invoke/2"), invoke_method);
        interp.register_native(key(full, "Invoke/3"), invoke_method);
    }

    let field_type: &'static str = Box::leak(format!("{REFLECT}.FieldInfo").into_boxed_str());
    interp.register_native(key(field_type, "get_Name()"), |i, a| {
        let id = FieldId(member_id(i, a)?);
        let name = i.loader.registry.field(id).name.clone();
        Ok(Some(string_value(i, &name)))
    });
    interp.register_native(key(field_type, "get_IsStatic()"), |i, a| {
        let id = FieldId(member_id(i, a)?);
        Ok(Some(Value::I32(i.loader.registry.field(id).is_static as i32)))
    });
    interp.register_native(key(field_type, "get_DeclaringType()"), |i, a| {
        let id = FieldId(member_id(i, a)?);
        let declaring = i.loader.registry.field(id).declaring_type;
        let handle = i.type_object(declaring);
        Ok(Some(Value::Obj(handle)))
    });
    interp.register_native(key(field_type, "GetValue(object)"), |i, a| {
        let id = FieldId(member_id(i, a)?);
        let info = i.loader.registry.field(id);
        if info.is_static {
            return Ok(Some(i.loader.static_value(id).clone()));
        }
        let target = arg(i, a, 1)?
            .as_handle()
            .filter(|h| !h.is_null())
            .ok_or_else(ExecutionError::null_reference)?;
        let slot = field_slot(i, target, id)?;
        Ok(Some(field(i, target, slot)))
    });
    interp.register_native(key(field_type, "SetValue(object,object)"), |i, a| {
        let id = FieldId(member_id(i, a)?);
        // `SetValue` takes `object`, so an `int` arrives boxed. The field slot
        // holds the value, not the box — storing the box would make every
        // later read of that field return zero.
        let value = arg(i, a, 2)?;
        let value = unbox(i, value);
        if i.loader.registry.field(id).is_static {
            *i.loader.static_value_mut(id) = value;
            return Ok(None);
        }
        let target = arg(i, a, 1)?
            .as_handle()
            .filter(|h| !h.is_null())
            .ok_or_else(ExecutionError::null_reference)?;
        let slot = field_slot(i, target, id)?;
        set_field(i, target, slot, value);
        Ok(None)
    });
}

/// The slot a field occupies on a concrete instance.
fn field_slot(interp: &Interpreter, target: Handle, field: FieldId) -> ExecResult<usize> {
    let type_id = interp.type_of(target).ok_or_else(ExecutionError::null_reference)?;
    interp
        .instance_fields(type_id)
        .iter()
        .position(|f| *f == field)
        .ok_or_else(|| {
            ExecutionError::exception(
                ClrExceptionKind::Argument,
                "the field does not belong to this instance",
            )
        })
}

/// `MethodInfo.Invoke(target, args)`.
fn invoke_method(interp: &mut Interpreter, args: &[Value]) -> ExecResult<Option<Value>> {
    let id = MethodId(member_id(interp, args)?);
    let info = interp.loader.registry.method(id);
    let is_static = info.is_static();
    let expected = info.signature.params.len();

    let target = arg(interp, args, 1)?;
    // The argument array is the last parameter in both overloads.
    let supplied = args.last().cloned().unwrap_or(Value::Null);
    let supplied = match supplied.as_handle().filter(|h| !h.is_null()) {
        Some(h) => array_values(interp, h),
        None => Vec::new(),
    };
    if supplied.len() != expected {
        return Err(ExecutionError::exception(
            ClrExceptionKind::Argument,
            format!("the method takes {expected} arguments, {} were supplied", supplied.len()),
        ));
    }

    let mut call_args = Vec::with_capacity(expected + 1);
    if !is_static {
        if target.is_null() {
            return Err(ExecutionError::null_reference());
        }
        call_args.push(target);
    }
    // `Invoke` takes `object[]`, so an `int` argument arrives boxed. The method
    // itself expects the value, not the box — passing the box through would
    // hand a primitive parameter a reference and quietly compute nonsense.
    call_args.extend(supplied.into_iter().map(|v| unbox(interp, v)));
    interp.invoke(id, call_args)
}

/// Unwraps a boxed primitive, leaving anything else alone.
fn unbox(interp: &Interpreter, value: Value) -> Value {
    match &value {
        Value::Obj(h) => match interp.heap.get_as::<rustclr_core::ClrBox>(*h) {
            Some(b) => b.value.clone(),
            None => value,
        },
        _ => value,
    }
}

// -- Activator ----------------------------------------------------------------

fn register_activator(interp: &mut Interpreter) {
    const ACTIVATOR: &str = "System.Activator";

    // `Activator.CreateInstance(type)` and the generic `CreateInstance<T>()`,
    // which after erasure arrives with no argument at all — so the type comes
    // from the binding key the loader chose for the instantiation.
    interp.register_native(key(ACTIVATOR, "CreateInstance/1"), |i, a| {
        let id = described(i, a, 0)?;
        Ok(Some(construct(i, id)?))
    });
    interp.register_native(key(ACTIVATOR, "CreateInstance()"), |_i, _a| {
        Err(ExecutionError::MissingImplementation(
            "Activator.CreateInstance<T>() cannot name T on this runtime: generic type \
             arguments are erased. Pass the type explicitly — CreateInstance(typeof(T)) — \
             or see docs/limitations.md."
                .into(),
        ))
    });
}

/// Allocates an instance and runs its parameterless constructor.
fn construct(interp: &mut Interpreter, type_id: TypeId) -> ExecResult<Value> {
    let ty = interp.loader.registry.ty(type_id);
    if ty.is_abstract || ty.kind == TypeKind::Interface {
        let name = ty.full_name();
        return Err(ExecutionError::exception(
            ClrExceptionKind::InvalidOperation,
            format!("Cannot create an instance of {name} because it is abstract."),
        ));
    }
    if ty.kind.is_value_like() {
        return Ok(interp.zero_of(type_id));
    }

    interp.ensure_cctor(type_id)?;
    let handle = interp.alloc_object(type_id);
    let ctor = declared_methods(interp, type_id, true).into_iter().find(|m| {
        let info = interp.loader.registry.method(*m);
        info.signature.params.is_empty() && !info.is_static()
    });
    if let Some(ctor) = ctor {
        interp.invoke(ctor, vec![Value::Obj(handle)])?;
    }
    Ok(Value::Obj(handle))
}

// -- custom attributes --------------------------------------------------------
//
// An attribute is stored as its constructor plus a blob of encoded arguments
// (ECMA-335 II.23.3). Reading one means decoding the blob against the
// constructor's signature and then *running* that constructor — which is why
// nothing is decoded during loading.

/// A cursor over an attribute's argument blob.
struct Blob<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Blob<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.at..self.at + n)?;
        self.at += n;
        Some(slice)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }

    /// A length-prefixed UTF-8 string; `0xFF` means null.
    fn string(&mut self) -> Option<Option<String>> {
        if *self.bytes.get(self.at)? == 0xFF {
            self.at += 1;
            return Some(None);
        }
        let length = self.compressed()? as usize;
        let bytes = self.take(length)?;
        Some(Some(String::from_utf8_lossy(bytes).into_owned()))
    }

    /// ECMA-335 II.23.2 compressed unsigned integer.
    fn compressed(&mut self) -> Option<u32> {
        let first = self.u8()?;
        if first & 0x80 == 0 {
            return Some(first as u32);
        }
        if first & 0x40 == 0 {
            let second = self.u8()?;
            return Some((((first & 0x3F) as u32) << 8) | second as u32);
        }
        let rest = self.take(3)?;
        Some(
            (((first & 0x1F) as u32) << 24)
                | ((rest[0] as u32) << 16)
                | ((rest[1] as u32) << 8)
                | rest[2] as u32,
        )
    }
}

/// Reads one argument of the given signature type.
///
/// Returns `None` for a shape this runtime does not decode — arrays, `Type`
/// arguments and boxed objects. The caller abandons the whole attribute rather
/// than constructing it with a wrong value in that slot.
fn read_argument(
    interp: &mut Interpreter,
    blob: &mut Blob<'_>,
    signature: &rustclr_core::metadata::TypeSig,
) -> Option<Value> {
    use rustclr_core::metadata::TypeSig as T;
    Some(match signature.unwrap_modifiers() {
        T::Boolean => Value::I32((blob.u8()? != 0) as i32),
        T::I1 => Value::I32(blob.u8()? as i8 as i32),
        T::U1 => Value::I32(blob.u8()? as i32),
        T::Char | T::U2 => Value::I32(blob.u16()? as i32),
        T::I2 => Value::I32(blob.u16()? as i16 as i32),
        T::I4 => Value::I32(i32::from_le_bytes(blob.take(4)?.try_into().ok()?)),
        T::U4 => Value::I32(u32::from_le_bytes(blob.take(4)?.try_into().ok()?) as i32),
        T::I8 | T::U8 => Value::I64(i64::from_le_bytes(blob.take(8)?.try_into().ok()?)),
        T::R4 => Value::F(f32::from_le_bytes(blob.take(4)?.try_into().ok()?) as f64),
        T::R8 => Value::F(f64::from_le_bytes(blob.take(8)?.try_into().ok()?)),
        T::String => match blob.string()? {
            Some(text) => string_value(interp, &text),
            None => Value::Null,
        },
        // An enum argument is encoded as its underlying type. Only the common
        // 32-bit case is decoded; anything else is refused rather than guessed.
        T::ValueType(_) => Value::I32(i32::from_le_bytes(blob.take(4)?.try_into().ok()?)),
        _ => return None,
    })
}

/// Builds the attribute instances applied to a token.
fn attribute_instances(
    interp: &mut Interpreter,
    assembly: rustclr_core::AssemblyId,
    token: rustclr_core::metadata::Token,
) -> ExecResult<Vec<Value>> {
    let applied: Vec<rustclr_core::CustomAttribute> =
        interp.loader.attributes_on(assembly, token).to_vec();
    let mut out = Vec::with_capacity(applied.len());

    for attribute in applied {
        let info = interp.loader.registry.method(attribute.constructor);
        let declaring = info.declaring_type;
        let parameters = info.signature.params.clone();
        let is_managed = matches!(info.kind, rustclr_core::MethodKind::Il(_));

        let mut blob = Blob::new(&attribute.value);
        // The prolog is always 0x0001; anything else is not an attribute blob.
        if blob.u16() != Some(0x0001) {
            continue;
        }

        let mut arguments = Vec::with_capacity(parameters.len());
        let mut decoded = true;
        for parameter in &parameters {
            match read_argument(interp, &mut blob, parameter) {
                Some(v) => arguments.push(v),
                None => {
                    decoded = false;
                    break;
                }
            }
        }
        // An argument shape this runtime cannot read would make the instance
        // wrong. Skipping it reports the same "not found" a caller gets for an
        // attribute that is genuinely absent, which is far better than an
        // instance carrying an invented value.
        if !decoded {
            continue;
        }

        let instance = interp.alloc_object(declaring);
        let mut call_args = Vec::with_capacity(arguments.len() + 1);
        call_args.push(Value::Obj(instance));
        call_args.extend(arguments);
        // A framework attribute has no managed constructor; its arguments are
        // still decoded, there is simply nothing to run.
        if is_managed {
            interp.invoke(attribute.constructor, call_args)?;
        }

        apply_named_arguments(interp, &mut blob, instance);
        out.push(Value::Obj(instance));
    }
    Ok(out)
}

/// Applies the `Name = value` part of an attribute's blob.
fn apply_named_arguments(interp: &mut Interpreter, blob: &mut Blob<'_>, instance: Handle) {
    const FIELD: u8 = 0x53;
    const PROPERTY: u8 = 0x54;

    let Some(count) = blob.u16() else { return };
    let Some(type_id) = interp.type_of(instance) else { return };

    for _ in 0..count {
        let Some(kind) = blob.u8() else { return };
        let Some(element) = blob.u8() else { return };
        let Some(Some(name)) = blob.string() else { return };

        // An unreadable element type means the rest of the blob cannot be
        // located either, so stop rather than misread what follows.
        let Some(signature) = element_signature(element) else { return };
        let Some(value) = read_argument(interp, blob, &signature) else { return };

        match kind {
            FIELD => {
                let field = interp
                    .instance_fields(type_id)
                    .into_iter()
                    .find(|f| interp.loader.registry.field(*f).name == name);
                if let Some(field) = field {
                    if let Ok(slot) = field_slot(interp, instance, field) {
                        set_field(interp, instance, slot, value);
                    }
                }
            }
            PROPERTY => {
                // A property is set through its accessor, so the attribute's
                // own logic runs rather than being bypassed.
                let setter = format!("set_{name}");
                let method = interp
                    .loader
                    .registry
                    .ty(type_id)
                    .methods
                    .iter()
                    .copied()
                    .find(|m| interp.loader.registry.method(*m).name == setter);
                if let Some(method) = method {
                    let _ = interp.invoke(method, vec![Value::Obj(instance), value]);
                }
            }
            _ => return,
        }
    }
}

/// The signature an element-type byte denotes, for named arguments.
fn element_signature(element: u8) -> Option<rustclr_core::metadata::TypeSig> {
    use rustclr_core::metadata::TypeSig as T;
    Some(match element {
        0x02 => T::Boolean,
        0x03 => T::Char,
        0x04 => T::I1,
        0x05 => T::U1,
        0x06 => T::I2,
        0x07 => T::U2,
        0x08 => T::I4,
        0x09 => T::U4,
        0x0A => T::I8,
        0x0B => T::U8,
        0x0C => T::R4,
        0x0D => T::R8,
        0x0E => T::String,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interpreter() -> Interpreter {
        let mut i = Interpreter::with_host(Box::new(rustclr_core::CaptureHost::new()));
        crate::install(&mut i);
        i
    }

    #[test]
    fn a_type_object_is_interned_so_identity_works() {
        let mut i = interpreter();
        let int_type = i.loader.primitive_type(rustclr_core::Primitive::Int32);
        let a = i.type_object(int_type);
        let b = i.type_object(int_type);
        assert_eq!(a, b, "typeof(int) == typeof(int) must be reference equality");
        assert_eq!(i.type_from_object(a), Some(int_type));
    }

    #[test]
    fn a_type_object_round_trips_to_its_name() {
        let mut i = interpreter();
        let string_type = i.loader.core().string;
        let handle = i.type_object(string_type);
        let described = i.type_from_object(handle).expect("round trip");
        assert_eq!(i.loader.registry.ty(described).full_name(), "System.String");
    }

    #[test]
    fn an_ordinary_object_is_not_mistaken_for_a_type() {
        let mut i = interpreter();
        let object_type = i.loader.core().object;
        let plain = i.alloc_object(object_type);
        assert_eq!(i.type_from_object(plain), None);
    }

    #[test]
    fn assignability_follows_the_base_chain() {
        let i = interpreter();
        let object_type = i.loader.core().object;
        let string_type = i.loader.core().string;
        assert!(assignable(&i, object_type, string_type), "string is an object");
        assert!(!assignable(&i, string_type, object_type), "not the other way");
        assert!(assignable(&i, string_type, string_type));
    }
}
